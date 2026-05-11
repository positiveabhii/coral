//! Adapters from neutral source-model projections into engine types.

#![allow(
    dead_code,
    reason = "Source-model projection adapter is spike plumbing that is not wired into runtime registration yet."
)]

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use coral_spec::types::projection::{ProjectionFilter, ProjectionScalarType, TableProjection};
use coral_spec::types::source::{
    Binding, HeaderValue, HttpMethod, JsonPath, OperationId, Surface, SurfaceKind,
};
use datafusion::arrow::array::{ArrayRef, Int64Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::TableProvider;
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::{Expr, Operator, TableProviderFilterPushDown, TableType};
use datafusion::physical_plan::ExecutionPlan;
use serde_json::Value;

use crate::backends::shared::filter_expr::literal_to_string;
use crate::backends::shared::json_exec::{Converter, Fetcher, JsonExec, RowFetcher};
use crate::backends::shared::json_path::get_path_value;

#[derive(Debug, Clone)]
pub(crate) struct ProjectionTableShape {
    pub(crate) schema: SchemaRef,
    pub(crate) output_columns: Vec<String>,
    pub(crate) filter_only_columns: Vec<String>,
    pub(crate) required_filters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceModelOperationInputs {
    pub(crate) operation: OperationId,
    pub(crate) values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceModelRestRequest {
    pub(crate) operation: OperationId,
    pub(crate) method: HttpMethod,
    pub(crate) url: String,
    pub(crate) query_params: Vec<(String, String)>,
    pub(crate) headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceModelTableProvider {
    source_schema: String,
    table_name: String,
    operation: OperationId,
    shape: ProjectionTableShape,
    filters: Vec<ProjectionFilter>,
    execution: SourceModelExecution,
}

#[derive(Debug, Clone)]
enum SourceModelExecution {
    Empty,
    Rest(Box<SourceModelRestExecution>),
}

#[derive(Debug, Clone)]
struct SourceModelRestExecution {
    surface: Surface,
    binding: Binding,
    source_inputs: BTreeMap<String, String>,
    client: reqwest::Client,
}

impl SourceModelTableProvider {
    pub(crate) fn new(source_schema: impl Into<String>, projection: &TableProjection) -> Self {
        let source_schema = source_schema.into();
        Self {
            table_name: table_name_from_projection_name(&source_schema, projection.name()),
            source_schema,
            operation: projection.operation().clone(),
            shape: table_shape_from_projection(projection),
            filters: projection.filters().to_vec(),
            execution: SourceModelExecution::Empty,
        }
    }

    pub(crate) fn new_rest(
        source_schema: impl Into<String>,
        projection: &TableProjection,
        surface: Surface,
        binding: Binding,
        source_inputs: BTreeMap<String, String>,
    ) -> Self {
        let mut provider = Self::new(source_schema, projection);
        provider.execution = SourceModelExecution::Rest(Box::new(SourceModelRestExecution {
            surface,
            binding,
            source_inputs,
            client: reqwest::Client::new(),
        }));
        provider
    }

    fn json_exec(
        &self,
        operation_inputs: SourceModelOperationInputs,
        request: Option<SourceModelRestRequest>,
        items_path: Vec<String>,
        client: Option<reqwest::Client>,
        projection: Option<&Vec<usize>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let fetcher: Fetcher = match (request, client) {
            (Some(request), Some(client)) => Arc::new(SourceModelRestFetchPlan {
                client,
                request,
                items_path,
            }),
            _ => Arc::new(EmptySourceModelFetchPlan),
        };

        let schema = self.shape.schema.clone();
        let shape = self.shape.clone();
        let operation_values = operation_inputs.values;
        let converter: Converter = Arc::new(move |items: &[Value]| {
            convert_source_model_items(schema.clone(), &shape, &operation_values, items)
        });

        Ok(Arc::new(JsonExec::new(
            &self.source_schema,
            &self.table_name,
            self.shape.schema.clone(),
            fetcher,
            converter,
            projection.cloned(),
        )?))
    }

    fn rest_json_exec(
        &self,
        runtime: &SourceModelRestExecution,
        operation_inputs: SourceModelOperationInputs,
        projection: Option<&Vec<usize>>,
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let request = build_rest_request(
            &runtime.surface,
            &runtime.binding,
            &runtime.source_inputs,
            &operation_inputs,
            limit,
        )?;
        let http = runtime.binding.protocol().as_http();
        let items_path = json_path_segments(http.response().items_path())?;

        self.json_exec(
            operation_inputs,
            Some(request),
            items_path,
            Some(runtime.client.clone()),
            projection,
        )
    }

    fn empty_json_exec(
        &self,
        operation_inputs: SourceModelOperationInputs,
        projection: Option<&Vec<usize>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.json_exec(operation_inputs, None, Vec::new(), None, projection)
    }

    pub(crate) fn rest_request_from_filters(
        &self,
        surface: &Surface,
        binding: &Binding,
        source_inputs: &BTreeMap<String, String>,
        filters: &[Expr],
        sql_limit: Option<usize>,
    ) -> Result<SourceModelRestRequest> {
        let operation_inputs = self.operation_inputs_from_filters(filters)?;
        build_rest_request(
            surface,
            binding,
            source_inputs,
            &operation_inputs,
            sql_limit,
        )
    }

    pub(crate) fn operation_inputs_from_filters(
        &self,
        filters: &[Expr],
    ) -> Result<SourceModelOperationInputs> {
        let values = extract_operation_input_values(filters, &self.filters);

        for required in self.filters.iter().filter(|filter| filter.is_required()) {
            if !values.contains_key(required.name()) {
                return Err(DataFusionError::Execution(format!(
                    "{}.{} scan is missing required operation input/column '{}'",
                    self.source_schema,
                    self.table_name,
                    required.name()
                )));
            }
        }

        Ok(SourceModelOperationInputs {
            operation: self.operation.clone(),
            values,
        })
    }
}

#[async_trait]
impl TableProvider for SourceModelTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.shape.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        let allowed = self
            .filters
            .iter()
            .map(ProjectionFilter::name)
            .collect::<HashSet<_>>();

        Ok(filters
            .iter()
            .map(|filter| classify_projection_filter(filter, &allowed))
            .collect())
    }

    async fn scan(
        &self,
        _state: &dyn datafusion::catalog::Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let operation_inputs = self.operation_inputs_from_filters(filters)?;
        match &self.execution {
            SourceModelExecution::Empty => self.empty_json_exec(operation_inputs, projection),
            SourceModelExecution::Rest(runtime) => {
                self.rest_json_exec(runtime, operation_inputs, projection, limit)
            }
        }
    }
}

#[derive(Debug)]
struct EmptySourceModelFetchPlan;

#[async_trait]
impl RowFetcher for EmptySourceModelFetchPlan {
    async fn fetch(&self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

#[derive(Debug)]
struct SourceModelRestFetchPlan {
    client: reqwest::Client,
    request: SourceModelRestRequest,
    items_path: Vec<String>,
}

#[async_trait]
impl RowFetcher for SourceModelRestFetchPlan {
    async fn fetch(&self) -> Result<Vec<Value>> {
        let method = match self.request.method {
            HttpMethod::Get => reqwest::Method::GET,
        };
        let mut request = self.client.request(method, &self.request.url);
        for (name, value) in &self.request.headers {
            request = request.header(name, value);
        }

        let response = request.send().await.map_err(|error| {
            DataFusionError::Execution(format!(
                "REST request for operation '{}' failed: {error}",
                self.request.operation.as_str()
            ))
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            DataFusionError::Execution(format!(
                "REST response for operation '{}' could not be read: {error}",
                self.request.operation.as_str()
            ))
        })?;

        if !status.is_success() {
            return Err(DataFusionError::Execution(format!(
                "REST request for operation '{}' returned HTTP {status}: {body}",
                self.request.operation.as_str()
            )));
        }

        let payload = serde_json::from_str::<Value>(&body).map_err(|error| {
            DataFusionError::Execution(format!(
                "REST response for operation '{}' was not valid JSON: {error}",
                self.request.operation.as_str()
            ))
        })?;
        let items = get_path_value(&payload, &self.items_path).ok_or_else(|| {
            DataFusionError::Execution(format!(
                "REST response for operation '{}' did not contain items at '{}'",
                self.request.operation.as_str(),
                display_json_path(&self.items_path)
            ))
        })?;
        let items = items.as_array().ok_or_else(|| {
            DataFusionError::Execution(format!(
                "REST response items at '{}' for operation '{}' were not an array",
                display_json_path(&self.items_path),
                self.request.operation.as_str()
            ))
        })?;

        Ok(items.clone())
    }
}

pub(crate) fn table_shape_from_projection(projection: &TableProjection) -> ProjectionTableShape {
    let output_columns = projection
        .columns()
        .iter()
        .map(|column| column.name().to_string())
        .collect::<Vec<_>>();
    let output_column_set = output_columns.iter().cloned().collect::<BTreeSet<_>>();

    let mut fields = projection
        .columns()
        .iter()
        .map(|column| {
            Field::new(
                column.name(),
                data_type_for_projection_type(column.ty()),
                column.nullable(),
            )
        })
        .collect::<Vec<_>>();

    let mut filter_only_columns = Vec::new();
    for filter in projection.filters() {
        if output_column_set.contains(filter.name()) {
            continue;
        }

        filter_only_columns.push(filter.name().to_string());
        fields.push(field_for_filter(filter));
    }

    let required_filters = projection
        .required_filter_names()
        .into_iter()
        .map(str::to_string)
        .collect();

    ProjectionTableShape {
        schema: Arc::new(Schema::new(fields)),
        output_columns,
        filter_only_columns,
        required_filters,
    }
}

fn table_name_from_projection_name(source_schema: &str, projection_name: &str) -> String {
    projection_name
        .strip_prefix(source_schema)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(projection_name)
        .to_string()
}

fn extract_operation_input_values(
    exprs: &[Expr],
    filters: &[ProjectionFilter],
) -> BTreeMap<String, String> {
    let allowed = filters
        .iter()
        .map(ProjectionFilter::name)
        .collect::<HashSet<_>>();
    let mut values = BTreeMap::new();

    for expr in exprs {
        collect_operation_input_values(expr, &allowed, &mut values);
    }

    values
}

fn collect_operation_input_values(
    expr: &Expr,
    allowed: &HashSet<&str>,
    values: &mut BTreeMap<String, String>,
) {
    match expr {
        Expr::BinaryExpr(binary) if binary.op == Operator::And => {
            collect_operation_input_values(binary.left.as_ref(), allowed, values);
            collect_operation_input_values(binary.right.as_ref(), allowed, values);
        }
        Expr::BinaryExpr(binary) if binary.op == Operator::Eq => {
            if let Some((name, value)) = extract_projection_filter_equality(
                binary.left.as_ref(),
                binary.right.as_ref(),
                allowed,
            ) {
                values.insert(name, value);
                return;
            }

            if let Some((name, value)) = extract_projection_filter_equality(
                binary.right.as_ref(),
                binary.left.as_ref(),
                allowed,
            ) {
                values.insert(name, value);
            }
        }
        _ => {}
    }
}

fn classify_projection_filter(expr: &Expr, allowed: &HashSet<&str>) -> TableProviderFilterPushDown {
    match expr {
        Expr::BinaryExpr(binary) if binary.op == Operator::And => {
            let left = classify_projection_filter(binary.left.as_ref(), allowed);
            let right = classify_projection_filter(binary.right.as_ref(), allowed);
            if matches!(left, TableProviderFilterPushDown::Exact)
                && matches!(right, TableProviderFilterPushDown::Exact)
            {
                TableProviderFilterPushDown::Exact
            } else {
                TableProviderFilterPushDown::Unsupported
            }
        }
        Expr::BinaryExpr(binary)
            if binary.op == Operator::Eq
                && (extract_projection_filter_equality(
                    binary.left.as_ref(),
                    binary.right.as_ref(),
                    allowed,
                )
                .is_some()
                    || extract_projection_filter_equality(
                        binary.right.as_ref(),
                        binary.left.as_ref(),
                        allowed,
                    )
                    .is_some()) =>
        {
            TableProviderFilterPushDown::Exact
        }
        _ => TableProviderFilterPushDown::Unsupported,
    }
}

fn extract_projection_filter_equality(
    left: &Expr,
    right: &Expr,
    allowed: &HashSet<&str>,
) -> Option<(String, String)> {
    let Expr::Column(column) = left else {
        return None;
    };

    let name = column.name();
    if !allowed.contains(name) {
        return None;
    }

    Some((name.to_string(), literal_to_string(right)?))
}

fn build_rest_request(
    surface: &Surface,
    binding: &Binding,
    source_inputs: &BTreeMap<String, String>,
    operation_inputs: &SourceModelOperationInputs,
    sql_limit: Option<usize>,
) -> Result<SourceModelRestRequest> {
    if binding.surface() != surface.id() {
        return Err(DataFusionError::Execution(format!(
            "REST binding '{}' references surface '{}' but received surface '{}'",
            binding.id().as_str(),
            binding.surface().as_str(),
            surface.id().as_str()
        )));
    }
    if surface.kind() != &SurfaceKind::Rest {
        return Err(DataFusionError::Execution(format!(
            "REST binding '{}' cannot use non-REST surface '{}'",
            binding.id().as_str(),
            surface.id().as_str()
        )));
    }
    if binding.operation() != &operation_inputs.operation {
        return Err(DataFusionError::Execution(format!(
            "REST binding for operation '{}' cannot execute inputs for operation '{}'",
            binding.operation().as_str(),
            operation_inputs.operation.as_str()
        )));
    }

    let http = binding.protocol().as_http();
    let rendered_path = expand_path_template(http.path(), &operation_inputs.values)?;
    let mut query_params = Vec::new();
    for query in http.query() {
        if let Some(value) = operation_inputs.values.get(query.input().as_str()) {
            query_params.push((query.name().to_string(), value.clone()));
        }
    }
    if let Some(pagination) = http.pagination() {
        let page_size = pagination.page_size();
        let value = sql_limit
            .map_or(page_size.default(), |limit| {
                limit.try_into().unwrap_or(u32::MAX)
            })
            .min(page_size.max())
            .max(1);
        query_params.push((page_size.query_param().to_string(), value.to_string()));
    }

    let url = build_url(surface.base_url(), &rendered_path, &query_params)?;
    let headers = build_rest_headers(surface, source_inputs)?;

    Ok(SourceModelRestRequest {
        operation: operation_inputs.operation.clone(),
        method: http.method(),
        url,
        query_params,
        headers,
    })
}

fn expand_path_template(
    template: &str,
    operation_inputs: &BTreeMap<String, String>,
) -> Result<String> {
    let mut rendered = String::new();
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        rendered.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err(DataFusionError::Execution(format!(
                "REST path template '{template}' has an unclosed input placeholder"
            )));
        };
        let name = &after_start[..end];
        if name.is_empty() || name.contains('{') {
            return Err(DataFusionError::Execution(format!(
                "REST path template '{template}' has an invalid input placeholder"
            )));
        }
        let value = operation_inputs.get(name).ok_or_else(|| {
            DataFusionError::Execution(format!(
                "REST path template '{template}' is missing required operation input '{name}'"
            ))
        })?;
        rendered.push_str(&urlencoding::encode(value));
        rest = &after_start[end + 1..];
    }

    if rest.contains('}') {
        return Err(DataFusionError::Execution(format!(
            "REST path template '{template}' has an unmatched closing brace"
        )));
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn build_url(base_url: &str, path: &str, query_params: &[(String, String)]) -> Result<String> {
    let trimmed_path = path.trim();
    if reqwest::Url::parse(trimmed_path).is_ok() || trimmed_path.starts_with("//") {
        return Err(DataFusionError::Execution(
            "REST request path must be relative; absolute URLs are not allowed".to_string(),
        ));
    }

    let base = base_url.trim_end_matches('/');
    let joined = if trimmed_path.starts_with('/') {
        format!("{base}{trimmed_path}")
    } else {
        format!("{base}/{trimmed_path}")
    };
    let mut url = reqwest::Url::parse(&joined).map_err(|error| {
        DataFusionError::Execution(format!("REST request URL '{joined}' is invalid: {error}"))
    })?;
    url.query_pairs_mut()
        .extend_pairs(query_params.iter().map(|(name, value)| (&**name, &**value)));
    Ok(url.to_string())
}

fn build_rest_headers(
    surface: &Surface,
    source_inputs: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();

    for header in surface.headers() {
        let value = match header.value() {
            HeaderValue::Literal(value) => value.clone(),
            HeaderValue::Input(input) => {
                source_inputs.get(input.as_str()).cloned().ok_or_else(|| {
                    DataFusionError::Execution(format!(
                        "REST header '{}' is missing source input '{}'",
                        header.name(),
                        input.as_str()
                    ))
                })?
            }
        };
        headers.insert(header.name().to_string(), value);
    }

    let (input, header, prefix) = surface.auth().bearer_token_parts();
    let token = source_inputs.get(input.as_str()).ok_or_else(|| {
        DataFusionError::Execution(format!(
            "REST bearer auth header '{header}' is missing source input '{}'",
            input.as_str()
        ))
    })?;
    headers.insert(header.to_string(), format!("{prefix}{token}"));

    Ok(headers)
}

fn json_path_segments(path: &JsonPath) -> Result<Vec<String>> {
    let path = path.as_str();
    if path == "$" {
        return Ok(Vec::new());
    }

    let rest = path.strip_prefix("$.").ok_or_else(|| {
        DataFusionError::Execution(format!(
            "REST response items path '{path}' must start with '$' or '$.'"
        ))
    })?;
    if rest.is_empty() || rest.contains('[') || rest.contains(']') || rest.contains("..") {
        return Err(DataFusionError::Execution(format!(
            "REST response items path '{path}' is not supported by the source-model spike"
        )));
    }

    Ok(rest.split('.').map(str::to_string).collect())
}

fn display_json_path(segments: &[String]) -> String {
    if segments.is_empty() {
        "$".to_string()
    } else {
        format!("$.{}", segments.join("."))
    }
}

fn convert_source_model_items(
    schema: SchemaRef,
    shape: &ProjectionTableShape,
    operation_values: &BTreeMap<String, String>,
    items: &[Value],
) -> Result<RecordBatch> {
    let filter_only = shape
        .filter_only_columns
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let arrays = schema
        .fields()
        .iter()
        .map(|field| source_model_array_for_field(field, &filter_only, operation_values, items))
        .collect::<Result<Vec<_>>>()?;

    RecordBatch::try_new(schema, arrays).map_err(|error| {
        DataFusionError::ArrowError(Box::new(error), Some("source-model conversion".to_string()))
    })
}

fn source_model_array_for_field(
    field: &Field,
    filter_only: &HashSet<&str>,
    operation_values: &BTreeMap<String, String>,
    items: &[Value],
) -> Result<ArrayRef> {
    match field.data_type() {
        DataType::Utf8 => {
            let values = items.iter().map(|item| {
                source_model_field_value(field.name(), filter_only, operation_values, item)
                    .and_then(json_value_to_string)
            });
            Ok(Arc::new(values.collect::<StringArray>()) as ArrayRef)
        }
        DataType::Int64 => {
            let values = items.iter().map(|item| {
                source_model_field_value(field.name(), filter_only, operation_values, item)
                    .and_then(json_value_to_i64)
            });
            Ok(Arc::new(values.collect::<Int64Array>()) as ArrayRef)
        }
        other => Err(DataFusionError::Execution(format!(
            "source-model column '{}' uses unsupported Arrow type {other:?}",
            field.name()
        ))),
    }
}

fn source_model_field_value(
    name: &str,
    filter_only: &HashSet<&str>,
    operation_values: &BTreeMap<String, String>,
    item: &Value,
) -> Option<Value> {
    if filter_only.contains(name) {
        return operation_values.get(name).cloned().map(Value::String);
    }

    let path = name
        .split("__")
        .map(str::to_string)
        .collect::<Vec<String>>();
    get_path_value(item, &path).cloned()
}

fn json_value_to_string(value: Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        other => Some(other.to_string()),
    }
}

fn json_value_to_i64(value: Value) -> Option<i64> {
    match value {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn field_for_filter(filter: &ProjectionFilter) -> Field {
    Field::new(
        filter.name(),
        data_type_for_projection_type(filter.ty()),
        true,
    )
}

fn data_type_for_projection_type(ty: &ProjectionScalarType) -> DataType {
    match ty {
        ProjectionScalarType::String => DataType::Utf8,
        ProjectionScalarType::Integer => DataType::Int64,
    }
}

#[cfg(test)]
mod tests {
    use coral_spec::types::projection::github_issues_projection;
    use coral_spec::types::source::{github_issue_list_rest_binding, github_rest_surface};
    use datafusion::arrow::array::{Array, Int64Array, StringArray};
    use datafusion::datasource::TableProvider;
    use datafusion::execution::TaskContext;
    use datafusion::logical_expr::{Operator, TableProviderFilterPushDown, binary_expr, col, lit};
    use datafusion::prelude::SessionContext;
    use futures::TryStreamExt;
    use serde_json::json;

    use super::*;

    fn github_issues_provider() -> SourceModelTableProvider {
        SourceModelTableProvider::new("github", &github_issues_projection())
    }

    fn github_issues_rest_provider() -> SourceModelTableProvider {
        SourceModelTableProvider::new_rest(
            "github",
            &github_issues_projection(),
            github_rest_surface(),
            github_issue_list_rest_binding(),
            BTreeMap::from([("GITHUB_TOKEN".to_string(), "ghp_test".to_string())]),
        )
    }

    #[test]
    fn builds_arrow_schema_from_github_issues_projection() {
        let projection = github_issues_projection();
        let shape = table_shape_from_projection(&projection);
        let fields = shape.schema.fields();

        assert_eq!(
            fields
                .iter()
                .map(|field| (field.name().as_str(), field.data_type()))
                .collect::<Vec<_>>(),
            vec![
                ("number", &DataType::Int64),
                ("title", &DataType::Utf8),
                ("state", &DataType::Utf8),
                ("created_at", &DataType::Utf8),
                ("user__login", &DataType::Utf8),
                ("owner", &DataType::Utf8),
                ("repo", &DataType::Utf8),
            ]
        );
    }

    #[test]
    fn preserves_projection_scan_metadata() {
        let projection = github_issues_projection();
        let shape = table_shape_from_projection(&projection);

        assert_eq!(
            shape.output_columns,
            vec!["number", "title", "state", "created_at", "user__login"]
        );
        assert_eq!(shape.filter_only_columns, vec!["owner", "repo"]);
        assert_eq!(shape.required_filters, vec!["owner", "repo"]);
    }

    #[test]
    fn extracts_required_and_optional_equality_filters_as_operation_inputs() {
        let provider = github_issues_provider();
        let owner_filter = col("owner").eq(lit("withcoral"));
        let repo_filter = col("repo").eq(lit("coral"));
        let state_filter = col("state").eq(lit("closed"));
        let inputs = provider
            .operation_inputs_from_filters(&[owner_filter, repo_filter, state_filter])
            .expect("filters should satisfy required operation inputs");

        assert_eq!(inputs.operation.as_str(), "github.issue.list");
        assert_eq!(
            inputs.values,
            BTreeMap::from([
                ("owner".to_string(), "withcoral".to_string()),
                ("repo".to_string(), "coral".to_string()),
                ("state".to_string(), "closed".to_string()),
            ])
        );
    }

    #[test]
    fn extracts_operation_inputs_from_conjunctions() {
        let provider = github_issues_provider();
        let filters = col("owner")
            .eq(lit("withcoral"))
            .and(col("repo").eq(lit("coral")))
            .and(col("state").eq(lit("closed")));
        let inputs = provider
            .operation_inputs_from_filters(&[filters])
            .expect("conjunctive filters should satisfy required operation inputs");

        assert_eq!(
            inputs.values.get("owner").map(String::as_str),
            Some("withcoral")
        );
        assert_eq!(inputs.values.get("repo").map(String::as_str), Some("coral"));
        assert_eq!(
            inputs.values.get("state").map(String::as_str),
            Some("closed")
        );
    }

    #[test]
    fn rejects_scan_inputs_missing_required_filters() {
        let provider = github_issues_provider();
        let error = provider
            .operation_inputs_from_filters(&[col("owner").eq(lit("withcoral"))])
            .expect_err("missing repo should fail");
        let message = error.to_string();

        assert!(message.contains("github.issues"));
        assert!(message.contains("repo"));
        assert!(message.contains("operation input/column"));
    }

    #[test]
    fn unsupported_filters_are_not_treated_as_operation_inputs() {
        let provider = github_issues_provider();
        let unsupported_state = binary_expr(col("state"), Operator::NotEq, lit("closed"));
        let pushdown = provider
            .supports_filters_pushdown(&[&unsupported_state])
            .expect("pushdown classification should succeed");

        assert_eq!(pushdown, vec![TableProviderFilterPushDown::Unsupported]);

        let inputs = provider
            .operation_inputs_from_filters(&[
                col("owner").eq(lit("withcoral")),
                col("repo").eq(lit("coral")),
                unsupported_state,
            ])
            .expect("unsupported optional filter should remain residual");

        assert!(!inputs.values.contains_key("state"));
    }

    #[test]
    fn equality_filters_are_exact_pushdown() {
        let provider = github_issues_provider();
        let owner_filter = col("owner").eq(lit("withcoral"));
        let repo_filter = lit("coral").eq(col("repo"));
        let pushdown = provider
            .supports_filters_pushdown(&[&owner_filter, &repo_filter])
            .expect("pushdown classification should succeed");

        assert_eq!(
            pushdown,
            vec![
                TableProviderFilterPushDown::Exact,
                TableProviderFilterPushDown::Exact,
            ]
        );
    }

    #[tokio::test]
    async fn rest_provider_scan_produces_json_exec() {
        let provider = github_issues_rest_provider();
        let ctx = SessionContext::new();
        let state = ctx.state();
        let plan = provider
            .scan(
                &state,
                None,
                &[
                    col("owner").eq(lit("withcoral")),
                    col("repo").eq(lit("coral")),
                ],
                Some(10),
            )
            .await
            .expect("scan should produce a source-model execution plan");

        assert_eq!(plan.name(), "JsonExec");
        assert_eq!(plan.schema(), provider.schema());
    }

    #[tokio::test]
    async fn scan_respects_datafusion_projection() {
        let provider = github_issues_provider();
        let ctx = SessionContext::new();
        let state = ctx.state();
        let projection = vec![1, 4];
        let plan = provider
            .scan(
                &state,
                Some(&projection),
                &[
                    col("owner").eq(lit("withcoral")),
                    col("repo").eq(lit("coral")),
                ],
                None,
            )
            .await
            .expect("scan should produce a projected execution plan");

        assert_eq!(
            plan.schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            vec!["title", "user__login"]
        );

        let batches = plan
            .execute(0, Arc::new(TaskContext::default()))
            .expect("projected plan should execute")
            .try_collect::<Vec<_>>()
            .await
            .expect("projected stream should collect");

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_columns(), 2);
        assert_eq!(batches[0].num_rows(), 0);
    }

    #[tokio::test]
    async fn scan_rejects_missing_required_inputs() {
        let provider = github_issues_rest_provider();
        let ctx = SessionContext::new();
        let state = ctx.state();
        let error = provider
            .scan(&state, None, &[col("owner").eq(lit("withcoral"))], Some(10))
            .await
            .expect_err("missing repo should fail during scan");
        let message = error.to_string();

        assert!(message.contains("github.issues"));
        assert!(message.contains("repo"));
        assert!(message.contains("operation input/column"));
    }

    #[test]
    fn converts_rest_items_using_projection_columns_and_filter_values() {
        let provider = github_issues_provider();
        let operation_values = BTreeMap::from([
            ("owner".to_string(), "withcoral".to_string()),
            ("repo".to_string(), "coral".to_string()),
        ]);
        let batch = convert_source_model_items(
            provider.schema(),
            &provider.shape,
            &operation_values,
            &[json!({
                "number": 42,
                "title": "Spike issue",
                "state": "open",
                "created_at": "2026-05-11T10:00:00Z",
                "user": {
                    "login": "simonwhitaker"
                }
            })],
        )
        .expect("items should convert");

        assert_eq!(batch.num_rows(), 1);
        assert_eq!(
            batch
                .column_by_name("number")
                .expect("number column")
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("number should be int64")
                .value(0),
            42
        );
        assert_eq!(
            batch
                .column_by_name("user__login")
                .expect("user login column")
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("user login should be string")
                .value(0),
            "simonwhitaker"
        );
        assert_eq!(
            batch
                .column_by_name("owner")
                .expect("owner column")
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("owner should be string")
                .value(0),
            "withcoral"
        );
    }

    #[test]
    fn converts_missing_optional_json_fields_to_nulls() {
        let provider = github_issues_provider();
        let operation_values = BTreeMap::from([
            ("owner".to_string(), "withcoral".to_string()),
            ("repo".to_string(), "coral".to_string()),
        ]);
        let batch = convert_source_model_items(
            provider.schema(),
            &provider.shape,
            &operation_values,
            &[
                json!({
                    "number": 42,
                    "title": "Missing user",
                    "state": "open",
                    "created_at": "2026-05-11T10:00:00Z"
                }),
                json!({
                    "number": 43,
                    "title": "Null user login",
                    "state": "closed",
                    "created_at": "2026-05-11T11:00:00Z",
                    "user": {
                        "login": null
                    }
                }),
            ],
        )
        .expect("items with missing optional fields should convert");

        let user_login = batch
            .column_by_name("user__login")
            .expect("user login column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("user login should be string");

        assert!(user_login.is_null(0));
        assert!(user_login.is_null(1));
    }

    #[test]
    fn builds_github_issues_rest_request_from_operation_inputs() {
        let provider = github_issues_provider();
        let surface = github_rest_surface();
        let binding = github_issue_list_rest_binding();
        let source_inputs = BTreeMap::from([("GITHUB_TOKEN".to_string(), "ghp_test".to_string())]);
        let request = provider
            .rest_request_from_filters(
                &surface,
                &binding,
                &source_inputs,
                &[
                    col("owner").eq(lit("withcoral")),
                    col("repo").eq(lit("coral")),
                    col("state").eq(lit("closed")),
                ],
                None,
            )
            .expect("request should build from required inputs");

        assert_eq!(request.operation.as_str(), "github.issue.list");
        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(
            request.url,
            "https://api.github.com/repos/withcoral/coral/issues?state=closed&per_page=100"
        );
        assert_eq!(
            request.query_params,
            vec![
                ("state".to_string(), "closed".to_string()),
                ("per_page".to_string(), "100".to_string()),
            ]
        );
        assert_eq!(
            request.headers.get("Accept").map(String::as_str),
            Some("application/vnd.github+json")
        );
        assert_eq!(
            request
                .headers
                .get("X-GitHub-Api-Version")
                .map(String::as_str),
            Some("2022-11-28")
        );
        assert_eq!(
            request.headers.get("Authorization").map(String::as_str),
            Some("Bearer ghp_test")
        );
    }

    #[test]
    fn rest_request_uses_sql_limit_as_page_size_cap() {
        let provider = github_issues_provider();
        let surface = github_rest_surface();
        let binding = github_issue_list_rest_binding();
        let source_inputs = BTreeMap::from([("GITHUB_TOKEN".to_string(), "ghp_test".to_string())]);
        let request = provider
            .rest_request_from_filters(
                &surface,
                &binding,
                &source_inputs,
                &[
                    col("owner").eq(lit("withcoral")),
                    col("repo").eq(lit("coral")),
                ],
                Some(25),
            )
            .expect("request should build from required inputs");

        assert_eq!(
            request.url,
            "https://api.github.com/repos/withcoral/coral/issues?per_page=25"
        );
        assert_eq!(
            request.query_params,
            vec![("per_page".to_string(), "25".to_string())]
        );
    }

    #[test]
    fn missing_required_path_inputs_do_not_build_rest_requests() {
        let provider = github_issues_provider();
        let surface = github_rest_surface();
        let binding = github_issue_list_rest_binding();
        let source_inputs = BTreeMap::from([("GITHUB_TOKEN".to_string(), "ghp_test".to_string())]);
        let error = provider
            .rest_request_from_filters(
                &surface,
                &binding,
                &source_inputs,
                &[col("owner").eq(lit("withcoral"))],
                None,
            )
            .expect_err("missing repo should fail before request execution");
        let message = error.to_string();

        assert!(message.contains("github.issues"));
        assert!(message.contains("repo"));
    }

    #[test]
    fn path_template_expansion_encodes_operation_inputs() {
        let rendered = expand_path_template(
            "/repos/{owner}/{repo}/issues",
            &BTreeMap::from([
                ("owner".to_string(), "with coral".to_string()),
                ("repo".to_string(), "coral/core".to_string()),
            ]),
        )
        .expect("path should render");

        assert_eq!(rendered, "/repos/with%20coral/coral%2Fcore/issues");
    }
}
