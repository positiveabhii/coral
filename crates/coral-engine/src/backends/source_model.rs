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
    Binding, HeaderValue, HttpMethod, OperationId, Surface, SurfaceKind,
};
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
