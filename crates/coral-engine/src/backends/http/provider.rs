//! `DataFusion` table provider for manifest-driven HTTP-backed tables.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::datasource::TableProvider;
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::TaskContext;
use datafusion::logical_expr::{
    Expr, Operator, TableProviderFilterPushDown, TableType, dml::InsertOp,
};
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, Partitioning, PlanProperties, SendableRecordBatchStream,
};
use datafusion::scalar::ScalarValue;
use futures::stream;
use serde_json::{Value, json};

use crate::backends::http::HttpSourceClient;
use crate::backends::http::ProviderQueryError;
use crate::backends::http::target::{HttpFetchTarget, HttpWriteTarget};
use crate::backends::http::write_exec::HttpWriteExec;
use crate::backends::schema_from_columns;
use crate::backends::shared::filter_expr::{extract_filter_values, literal_to_string};
use crate::backends::shared::json_exec::{JsonExec, RowFetcher};
use crate::backends::shared::mapping::convert_items;
use coral_spec::FilterMode;
use coral_spec::backends::http::{HttpRelationSpec, HttpRelationWriteOperationSpec};

/// Table provider that exposes one manifest-defined HTTP table to `DataFusion`.
pub(crate) struct HttpSourceTableProvider {
    backend: HttpSourceClient,
    source_schema: String,
    table: Arc<HttpRelationSpec>,
    target: Option<HttpFetchTarget>,
    schema: SchemaRef,
}

impl std::fmt::Debug for HttpSourceTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSourceTableProvider")
            .field("source_schema", &self.source_schema)
            .field("table", &self.table.name())
            .finish_non_exhaustive()
    }
}

impl HttpSourceTableProvider {
    /// Build a table provider for an `HTTP`-backed source table.
    ///
    /// # Errors
    ///
    /// Returns a `DataFusionError` if the table schema declared in the manifest
    /// is invalid.
    pub(crate) fn new(
        backend: HttpSourceClient,
        source_schema: String,
        table: HttpRelationSpec,
    ) -> Result<Self> {
        let schema = schema_from_columns(table.columns(), &source_schema, table.name())?;
        let target = table
            .read()
            .map(|read| HttpFetchTarget::from_resolved_table_request(&table, read.request.clone()));
        Ok(Self {
            backend,
            source_schema,
            table: Arc::new(table),
            target,
            schema,
        })
    }
}

#[derive(Debug)]
struct HttpFetchPlan {
    backend: HttpSourceClient,
    target: Arc<HttpFetchTarget>,
    filter_values: Arc<HashMap<String, String>>,
    arg_values: Arc<HashMap<String, String>>,
    limit: Option<usize>,
}

#[derive(Debug)]
struct UnsupportedReadExec {
    schema: SchemaRef,
    props: Arc<PlanProperties>,
    message: String,
}

impl UnsupportedReadExec {
    fn new(schema: SchemaRef, message: String) -> Self {
        let props = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(schema.clone()),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Self {
            schema,
            props,
            message,
        }
    }
}

pub(crate) struct HttpJsonExecRequest<'a> {
    pub(crate) backend: HttpSourceClient,
    pub(crate) source_schema: &'a str,
    pub(crate) target: HttpFetchTarget,
    pub(crate) schema: SchemaRef,
    pub(crate) filter_values: HashMap<String, String>,
    pub(crate) arg_values: HashMap<String, String>,
    pub(crate) projection: Option<&'a Vec<usize>>,
    pub(crate) limit: Option<usize>,
}

#[async_trait]
impl RowFetcher for HttpFetchPlan {
    async fn fetch(&self) -> Result<Vec<Value>> {
        self.backend
            .fetch(
                self.target.as_ref(),
                &self.filter_values,
                &self.arg_values,
                self.limit,
            )
            .await
    }
}

impl DisplayAs for UnsupportedReadExec {
    fn fmt_as(&self, _format: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "UnsupportedReadExec")
    }
}

impl ExecutionPlan for UnsupportedReadExec {
    fn name(&self) -> &'static str {
        "UnsupportedReadExec"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.props
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> Result<datafusion::common::Statistics> {
        Ok(datafusion::common::Statistics::new_unknown(&self.schema()))
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if !children.is_empty() {
            return Err(DataFusionError::Plan(format!(
                "UnsupportedReadExec expects no children, got {}",
                children.len()
            )));
        }
        Ok(self)
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let schema = self.schema.clone();
        let stream_schema = schema.clone();
        let message = self.message.clone();
        let stream = stream::once(async move { Err(DataFusionError::Plan(message)) });
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            stream_schema,
            stream,
        )))
    }
}

pub(crate) fn http_json_exec(request: HttpJsonExecRequest<'_>) -> Result<Arc<dyn ExecutionPlan>> {
    let HttpJsonExecRequest {
        backend,
        source_schema,
        target,
        schema,
        filter_values,
        arg_values,
        projection,
        limit,
    } = request;
    let target = Arc::new(target);
    let filter_values = Arc::new(filter_values);
    let arg_values = Arc::new(arg_values);
    let fetcher = Arc::new(HttpFetchPlan {
        backend,
        target: target.clone(),
        filter_values: filter_values.clone(),
        arg_values,
        limit,
    });

    let converter = {
        let target = target.clone();
        let schema = schema.clone();
        let filter_values = filter_values.clone();
        Arc::new(move |items: &[Value]| {
            convert_items(target.columns(), schema.clone(), &filter_values, items)
        })
    };

    let exec = JsonExec::new(
        source_schema,
        target.name(),
        schema,
        fetcher,
        converter,
        projection.cloned(),
    )?;

    Ok(Arc::new(exec))
}

#[async_trait]
impl TableProvider for HttpSourceTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        let allowed: HashSet<&str> = self
            .table
            .filters()
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        let filter_modes: HashMap<&str, FilterMode> = self
            .table
            .filters()
            .iter()
            .map(|f| (f.name.as_str(), f.mode))
            .collect();

        Ok(filters
            .iter()
            .map(|expr| classify_filter(expr, &allowed, &filter_modes))
            .collect())
    }

    async fn scan(
        &self,
        _state: &dyn datafusion::catalog::Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let filter_values = extract_filter_values(filters, self.table.filters());

        for required in self.table.filters().iter().filter(|f| f.required) {
            if !filter_values.contains_key(&required.name) {
                return Err(DataFusionError::External(Box::new(
                    ProviderQueryError::MissingRequiredFilter {
                        schema: self.source_schema.clone(),
                        table: self.table.name().to_string(),
                        column: required.name.clone(),
                    },
                )));
            }
        }

        let filter_value_keys: HashSet<String> = filter_values.keys().cloned().collect();
        let Some(active_request) = self.table.resolve_read_request(&filter_value_keys) else {
            return Ok(Arc::new(UnsupportedReadExec::new(
                self.schema.clone(),
                format!(
                    "{}.{} does not declare a read projection",
                    self.source_schema,
                    self.table.name()
                ),
            )));
        };
        let Some(target) = self.target.as_ref() else {
            return Ok(Arc::new(UnsupportedReadExec::new(
                self.schema.clone(),
                format!(
                    "{}.{} does not declare a read projection",
                    self.source_schema,
                    self.table.name()
                ),
            )));
        };
        let target = target.with_resolved_request(active_request.clone());

        http_json_exec(HttpJsonExecRequest {
            backend: self.backend.clone(),
            source_schema: &self.source_schema,
            target,
            schema: self.schema.clone(),
            filter_values,
            arg_values: HashMap::new(),
            projection,
            limit,
        })
    }

    async fn insert_into(
        &self,
        _state: &dyn datafusion::catalog::Session,
        input: Arc<dyn ExecutionPlan>,
        insert_op: InsertOp,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        if insert_op != InsertOp::Append {
            return Err(DataFusionError::Plan(format!(
                "{}.{} only supports append INSERT; got {insert_op}",
                self.source_schema,
                self.table.name()
            )));
        }
        let operation = self.write_operation(self.table.insert.as_ref(), "INSERT")?;
        let target = HttpWriteTarget::from_relation_write(&self.table, &operation);
        Ok(Arc::new(HttpWriteExec::insert(
            self.backend.clone(),
            self.source_schema.clone(),
            self.table.name().to_string(),
            operation,
            target,
            input,
        )))
    }

    async fn update(
        &self,
        _state: &dyn datafusion::catalog::Session,
        assignments: Vec<(String, Expr)>,
        filters: Vec<Expr>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let operation = self.write_operation(self.table.update.as_ref(), "UPDATE")?;
        let key_values = extract_write_key_values(
            &self.source_schema,
            self.table.name(),
            &filters,
            &operation.key_columns,
            "UPDATE",
        )?;
        let values = assignment_values(
            &self.source_schema,
            self.table.name(),
            &operation,
            assignments,
        )?;
        let target = HttpWriteTarget::from_relation_write(&self.table, &operation);
        Ok(Arc::new(HttpWriteExec::single(
            self.backend.clone(),
            self.source_schema.clone(),
            self.table.name().to_string(),
            operation,
            target,
            key_values,
            values,
        )))
    }

    async fn delete_from(
        &self,
        _state: &dyn datafusion::catalog::Session,
        filters: Vec<Expr>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let operation = self.write_operation(self.table.delete.as_ref(), "DELETE")?;
        let key_values = extract_write_key_values(
            &self.source_schema,
            self.table.name(),
            &filters,
            &operation.key_columns,
            "DELETE",
        )?;
        let target = HttpWriteTarget::from_relation_write(&self.table, &operation);
        Ok(Arc::new(HttpWriteExec::single(
            self.backend.clone(),
            self.source_schema.clone(),
            self.table.name().to_string(),
            operation,
            target,
            key_values,
            HashMap::new(),
        )))
    }

    async fn truncate(
        &self,
        _state: &dyn datafusion::catalog::Session,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let operation = self.write_operation(self.table.truncate.as_ref(), "TRUNCATE")?;
        let target = HttpWriteTarget::from_relation_write(&self.table, &operation);
        Ok(Arc::new(HttpWriteExec::single(
            self.backend.clone(),
            self.source_schema.clone(),
            self.table.name().to_string(),
            operation,
            target,
            HashMap::new(),
            HashMap::new(),
        )))
    }
}

impl HttpSourceTableProvider {
    fn write_operation(
        &self,
        operation: Option<&HttpRelationWriteOperationSpec>,
        sql_op: &str,
    ) -> Result<HttpRelationWriteOperationSpec> {
        operation.cloned().ok_or_else(|| {
            DataFusionError::Plan(format!(
                "{}.{} does not support {sql_op}",
                self.source_schema,
                self.table.name()
            ))
        })
    }
}

fn extract_write_key_values(
    schema: &str,
    relation: &str,
    filters: &[Expr],
    required_keys: &[String],
    operation: &str,
) -> Result<HashMap<String, String>> {
    if filters.is_empty() {
        return Err(DataFusionError::Plan(format!(
            "{operation} on {schema}.{relation} requires direct equality filters for key columns: {}",
            required_keys.join(", ")
        )));
    }
    let allowed = required_keys
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut values = HashMap::new();
    for filter in filters {
        collect_write_key_filter(schema, relation, filter, &allowed, &mut values, operation)?;
    }
    for key in required_keys {
        if !values.contains_key(key) {
            return Err(DataFusionError::Plan(format!(
                "{operation} on {schema}.{relation} missing required key filter '{key}'"
            )));
        }
    }
    Ok(values)
}

fn collect_write_key_filter(
    schema: &str,
    relation: &str,
    expr: &Expr,
    allowed: &HashSet<&str>,
    values: &mut HashMap<String, String>,
    operation: &str,
) -> Result<()> {
    if let Expr::BinaryExpr(binary) = expr
        && binary.op == Operator::And
    {
        collect_write_key_filter(
            schema,
            relation,
            binary.left.as_ref(),
            allowed,
            values,
            operation,
        )?;
        collect_write_key_filter(
            schema,
            relation,
            binary.right.as_ref(),
            allowed,
            values,
            operation,
        )?;
        return Ok(());
    }
    if let Some((column, value)) = extract_write_key_equality(expr, allowed) {
        match values.insert(column.clone(), value.clone()) {
            Some(previous) if previous != value => {
                return Err(DataFusionError::Plan(format!(
                    "{operation} on {schema}.{relation} has conflicting filters for key column '{column}'"
                )));
            }
            _ => return Ok(()),
        }
    }
    Err(DataFusionError::Plan(format!(
        "{operation} on {schema}.{relation} has unsupported write predicate '{expr:?}'; only direct key equality is supported"
    )))
}

fn extract_write_key_equality(expr: &Expr, allowed: &HashSet<&str>) -> Option<(String, String)> {
    match expr {
        Expr::BinaryExpr(binary) if binary.op == Operator::Eq => {
            extract_write_key_equality_sides(binary.left.as_ref(), binary.right.as_ref(), allowed)
                .or_else(|| {
                    extract_write_key_equality_sides(
                        binary.right.as_ref(),
                        binary.left.as_ref(),
                        allowed,
                    )
                })
        }
        Expr::InList(in_list) if !in_list.negated && in_list.list.len() == 1 => {
            let Expr::Column(col) = in_list.expr.as_ref() else {
                return None;
            };
            let column = col.name();
            if !allowed.contains(column) {
                return None;
            }
            let value = literal_to_string(in_list.list.first()?)?;
            Some((column.to_string(), value))
        }
        _ => None,
    }
}

fn extract_write_key_equality_sides(
    left: &Expr,
    right: &Expr,
    allowed: &HashSet<&str>,
) -> Option<(String, String)> {
    let Expr::Column(col) = left else {
        return None;
    };
    let column = col.name();
    if !allowed.contains(column) {
        return None;
    }
    let value = literal_to_string(right)?;
    Some((column.to_string(), value))
}

fn assignment_values(
    schema: &str,
    relation: &str,
    operation: &HttpRelationWriteOperationSpec,
    assignments: Vec<(String, Expr)>,
) -> Result<HashMap<String, Value>> {
    if assignments.is_empty() {
        return Err(DataFusionError::Plan(format!(
            "UPDATE on {schema}.{relation} must assign at least one writable column"
        )));
    }
    let writable = operation
        .input_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let keys = operation
        .key_columns
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut values = HashMap::new();
    for (column, expr) in assignments {
        if keys.contains(column.as_str()) {
            return Err(DataFusionError::Plan(format!(
                "UPDATE on {schema}.{relation} cannot assign target key column '{column}'"
            )));
        }
        if !writable.contains(column.as_str()) {
            return Err(DataFusionError::Plan(format!(
                "UPDATE on {schema}.{relation} cannot assign non-writable column '{column}'"
            )));
        }
        let value = literal_expr_to_json(&expr).ok_or_else(|| {
            DataFusionError::Plan(format!(
                "UPDATE on {schema}.{relation} assignment for '{column}' must be a literal"
            ))
        })?;
        values.insert(column, value);
    }
    Ok(values)
}

fn literal_expr_to_json(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Literal(value, _) => scalar_to_json(value),
        Expr::Cast(cast) => literal_expr_to_json(cast.expr.as_ref()),
        Expr::TryCast(cast) => literal_expr_to_json(cast.expr.as_ref()),
        _ => None,
    }
}

fn scalar_to_json(value: &ScalarValue) -> Option<Value> {
    match value {
        ScalarValue::Utf8(value) | ScalarValue::LargeUtf8(value) => {
            Some(value.clone().map_or(Value::Null, Value::String))
        }
        ScalarValue::Int64(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        ScalarValue::Int32(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        ScalarValue::UInt64(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        ScalarValue::UInt32(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        ScalarValue::Float64(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        ScalarValue::Float32(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        ScalarValue::Boolean(value) => Some(value.map_or(Value::Null, |value| json!(value))),
        _ => None,
    }
}

fn classify_filter(
    expr: &Expr,
    allowed: &HashSet<&str>,
    filter_modes: &HashMap<&str, FilterMode>,
) -> TableProviderFilterPushDown {
    if let Expr::Column(col) = expr
        && allowed.contains(col.name())
    {
        return TableProviderFilterPushDown::Exact;
    }
    if let Expr::Not(inner) = expr
        && let Expr::Column(col) = inner.as_ref()
        && allowed.contains(col.name())
    {
        return TableProviderFilterPushDown::Exact;
    }
    if let Expr::IsTrue(inner) | Expr::IsFalse(inner) = expr
        && let Expr::Column(col) = inner.as_ref()
        && allowed.contains(col.name())
    {
        return TableProviderFilterPushDown::Exact;
    }
    if let Expr::BinaryExpr(binary) = expr
        && binary.op == Operator::Eq
        && let Expr::Column(col) = binary.left.as_ref()
        && allowed.contains(col.name())
        && literal_to_string(binary.right.as_ref()).is_some()
    {
        return TableProviderFilterPushDown::Exact;
    }
    if let Expr::Like(like) = expr
        && !like.negated
        && let Expr::Column(col) = like.expr.as_ref()
        && allowed.contains(col.name())
        && literal_to_string(like.pattern.as_ref()).is_some()
    {
        let mode = filter_modes.get(col.name()).copied().unwrap_or_default();
        if matches!(mode, FilterMode::Search | FilterMode::Contains) {
            // Inexact: the API receives the stripped search term (performance
            // win) but DataFusion keeps a residual filter to enforce exact
            // LIKE/ILIKE semantics client-side (correctness win).
            return TableProviderFilterPushDown::Inexact;
        }
    }
    TableProviderFilterPushDown::Unsupported
}

#[cfg(test)]
mod tests {
    use super::classify_filter;
    use coral_spec::FilterMode;
    use datafusion::common::Column;
    use datafusion::logical_expr::{
        Expr, Operator, TableProviderFilterPushDown, binary_expr, expr::Like, lit,
    };
    use std::collections::{HashMap, HashSet};
    use std::ops::Not;

    fn allowed<'a>(names: &'a [&'a str]) -> HashSet<&'a str> {
        names.iter().copied().collect()
    }

    fn modes<'a>(entries: &'a [(&'a str, FilterMode)]) -> HashMap<&'a str, FilterMode> {
        entries.iter().copied().collect()
    }

    fn like_expr(col_name: &str, pattern: &str) -> Expr {
        Expr::Like(Like::new(
            false,
            Box::new(col(col_name)),
            Box::new(lit(pattern)),
            None,
            false,
        ))
    }

    fn col(name: &str) -> Expr {
        Expr::Column(Column::from_name(name))
    }

    #[test]
    fn like_ignored_for_equality_mode_filter() {
        let pushdown = classify_filter(
            &like_expr("status", "%open%"),
            &allowed(&["status"]),
            &modes(&[("status", FilterMode::Equality)]),
        );
        assert_eq!(pushdown, TableProviderFilterPushDown::Unsupported);
    }

    #[test]
    fn strips_wildcards_from_like_pattern() {
        let pushdown = classify_filter(
            &like_expr("q", "%deploy runbook%"),
            &allowed(&["q"]),
            &modes(&[("q", FilterMode::Search)]),
        );
        assert_eq!(pushdown, TableProviderFilterPushDown::Inexact);
    }

    #[test]
    fn search_filter_also_accepts_equality() {
        let pushdown = classify_filter(
            &binary_expr(col("query"), Operator::Eq, lit("deploy")),
            &allowed(&["query"]),
            &modes(&[("query", FilterMode::Search)]),
        );
        assert_eq!(pushdown, TableProviderFilterPushDown::Exact);
    }

    #[test]
    fn extracts_like_value_for_search_mode_filter() {
        let pushdown = classify_filter(
            &like_expr("query", "%deploy%"),
            &allowed(&["query"]),
            &modes(&[("query", FilterMode::Search)]),
        );
        assert_eq!(pushdown, TableProviderFilterPushDown::Inexact);
    }

    #[test]
    fn boolean_column_filter_pushes_down_exactly() {
        let pushdown = classify_filter(&col("descending"), &allowed(&["descending"]), &modes(&[]));
        assert_eq!(pushdown, TableProviderFilterPushDown::Exact);
    }

    #[test]
    fn negated_boolean_column_filter_pushes_down_exactly() {
        let pushdown = classify_filter(
            &col("descending").not(),
            &allowed(&["descending"]),
            &modes(&[]),
        );
        assert_eq!(pushdown, TableProviderFilterPushDown::Exact);
    }

    #[test]
    fn boolean_is_true_and_is_false_push_down_exactly() {
        for expr in [
            Expr::IsTrue(Box::new(col("descending"))),
            Expr::IsFalse(Box::new(col("descending"))),
        ] {
            let pushdown = classify_filter(&expr, &allowed(&["descending"]), &modes(&[]));
            assert_eq!(pushdown, TableProviderFilterPushDown::Exact);
        }
    }

    #[test]
    fn null_inclusive_boolean_is_predicates_are_not_pushed_down() {
        for expr in [
            Expr::IsNotTrue(Box::new(col("descending"))),
            Expr::IsNotFalse(Box::new(col("descending"))),
        ] {
            let pushdown = classify_filter(&expr, &allowed(&["descending"]), &modes(&[]));
            assert_eq!(pushdown, TableProviderFilterPushDown::Unsupported);
        }
    }
}
