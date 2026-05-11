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
use coral_spec::types::source::OperationId;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::TableProvider;
use datafusion::datasource::empty::EmptyTable;
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::{Expr, Operator, TableProviderFilterPushDown, TableType};
use datafusion::physical_plan::ExecutionPlan;

use crate::backends::shared::filter_expr::literal_to_string;

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

#[derive(Debug, Clone)]
pub(crate) struct SourceModelTableProvider {
    source_schema: String,
    table_name: String,
    operation: OperationId,
    shape: ProjectionTableShape,
    filters: Vec<ProjectionFilter>,
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
        }
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
        state: &dyn datafusion::catalog::Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let _operation_inputs = self.operation_inputs_from_filters(filters)?;
        EmptyTable::new(self.shape.schema.clone())
            .scan(state, projection, &[], limit)
            .await
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
    use datafusion::datasource::TableProvider;
    use datafusion::logical_expr::{Operator, TableProviderFilterPushDown, binary_expr, col, lit};

    use super::*;

    fn github_issues_provider() -> SourceModelTableProvider {
        SourceModelTableProvider::new("github", &github_issues_projection())
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
}
