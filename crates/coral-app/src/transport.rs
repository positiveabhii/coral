//! Shared gRPC transport helpers for app-owned services.

use std::future::Future;

use coral_api::{
    CORAL_ERROR_DOMAIN, grpc_response_status_code,
    v1::{
        AnalyticalPlan as AnalyticalPlanProto, Column, CostEstimate as CostEstimateProto,
        CostEstimateStatus as CostEstimateStatusProto,
        LocalDefaultPolicyCheck as LocalDefaultPolicyCheckProto, MemoryStatus as MemoryStatusProto,
        PlanExecution as PlanExecutionProto, PlanExecutionStatus as PlanExecutionStatusProto,
        PlanIntent as PlanIntentProto, PlanStep as PlanStepProto, PlanWarning as PlanWarningProto,
        PolicyCheck as PolicyCheckProto, PolicyCheckStatus as PolicyCheckStatusProto,
        QueryTestFailure, QueryTestResult, QueryTestSuccess,
        RelationalSqlStep as RelationalSqlStepProto, Source,
        SourceSkippedWarning as SourceSkippedWarningProto, SqlIntent as SqlIntentProto, Table,
        TableSummary, ValidateSourceResponse, Workspace, plan_intent, plan_step, plan_warning,
        policy_check, query_test_result,
    },
};
use opentelemetry::propagation::Extractor;
use opentelemetry::trace::Status as OtelStatus;
use tonic::codegen::{Service, http};
use tonic::{Code, Request, Status};
use tonic_types::{ErrorDetail, StatusExt as _};
use tracing::{Instrument as _, field};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::bootstrap::{AppError, app_status, core_status};
use crate::plan as app_plan;
use crate::query::manager::QueryManagerError;
use crate::workspaces::WorkspaceName;

struct MetadataExtractor<'a>(&'a tonic::metadata::MetadataMap);

impl Extractor for MetadataExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        use tonic::metadata::KeyRef;
        self.0
            .keys()
            .filter_map(|k| match k {
                KeyRef::Ascii(key) => Some(key.as_str()),
                KeyRef::Binary(_) => None,
            })
            .collect()
    }
}

/// Extracts a W3C trace context from incoming gRPC request metadata.
pub(crate) fn extract_trace_context(
    metadata: &tonic::metadata::MetadataMap,
) -> opentelemetry::Context {
    opentelemetry::global::get_text_map_propagator(|p| p.extract(&MetadataExtractor(metadata)))
}

/// Wraps a generated tonic service and stores the inbound gRPC path on the request.
///
/// Tonic preserves `http::Request` extensions when it decodes the protobuf
/// message into a `tonic::Request`, but generated server wrappers do not insert
/// `tonic::GrpcMethod` the way generated clients do. This keeps the method
/// data at the transport boundary and lets handlers read it from the request.
#[derive(Clone)]
pub(crate) struct GrpcMethodAnnotatedService<S> {
    inner: S,
}

impl<S> GrpcMethodAnnotatedService<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S, B> Service<http::Request<B>> for GrpcMethodAnnotatedService<S>
where
    S: Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut request: http::Request<B>) -> Self::Future {
        if let Some(method) = GrpcServerMethod::from_path(request.uri().path()) {
            request.extensions_mut().insert(method);
        }
        self.inner.call(request)
    }
}

impl<S> tonic::server::NamedService for GrpcMethodAnnotatedService<S>
where
    S: tonic::server::NamedService,
{
    const NAME: &'static str = S::NAME;
}

/// Creates a span parented to the trace context extracted from a gRPC request.
pub(crate) fn grpc_span<T>(request: &Request<T>) -> tracing::Span {
    let parent_cx = extract_trace_context(request.metadata());
    let metadata = grpc_method(request);
    let span_name = format!("{}/{}", metadata.service, metadata.method);
    let span = tracing::info_span!(
        "grpc",
        error.type = tracing::field::Empty,
        exception.message = tracing::field::Empty,
        otel.kind = "server",
        otel.name = span_name.as_str(),
        rpc.system = "grpc",
        rpc.system.name = "grpc",
        rpc.service = metadata.service.as_str(),
        rpc.method = metadata.method.as_str(),
        rpc.response.status_code = tracing::field::Empty,
        grpc.method = metadata.method.as_str(),
        grpc.status_code = tracing::field::Empty,
        grpc.code = tracing::field::Empty,
        status = tracing::field::Empty,
    );
    drop(span.set_parent(parent_cx));
    span
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GrpcServerMethod {
    service: String,
    method: String,
}

impl GrpcServerMethod {
    fn from_path(path: &str) -> Option<Self> {
        let trimmed = path.strip_prefix('/').unwrap_or(path);
        let (service, method) = trimmed.split_once('/')?;
        if service.is_empty() || method.is_empty() || method.contains('/') {
            return None;
        }
        Some(Self {
            service: service.to_string(),
            method: method.to_string(),
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
struct GrpcMethodMetadata {
    service: String,
    method: String,
}

impl GrpcMethodMetadata {
    fn new(service: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            method: method.into(),
        }
    }
}

fn grpc_method<T>(request: &Request<T>) -> GrpcMethodMetadata {
    if let Some(method) = request.extensions().get::<tonic::GrpcMethod<'static>>() {
        return GrpcMethodMetadata::new(method.service(), method.method());
    }
    request.extensions().get::<GrpcServerMethod>().map_or_else(
        || GrpcMethodMetadata::new("coral.v1.UnknownService", "Unknown"),
        |method| GrpcMethodMetadata::new(method.service.as_str(), method.method.as_str()),
    )
}

pub(crate) async fn instrument_grpc<T, F>(span: tracing::Span, future: F) -> Result<T, Status>
where
    F: Future<Output = Result<T, Status>>,
{
    let result = future.instrument(span.clone()).await;
    match &result {
        Ok(_) => record_grpc_status(&span, Code::Ok, None),
        Err(status) => record_grpc_status(&span, status.code(), Some(status)),
    }
    result
}

fn record_grpc_status(span: &tracing::Span, code: Code, status: Option<&Status>) {
    let response_status_code = grpc_response_status_code(code);
    span.record("grpc.status_code", code as i64);
    span.record("grpc.code", response_status_code);
    span.record("rpc.response.status_code", response_status_code);
    if code == Code::Ok {
        span.record("status", "ok");
        span.set_status(OtelStatus::Ok);
    } else {
        let error = status.map_or_else(
            || GrpcErrorTelemetry {
                error_type: response_status_code.to_string(),
                message: response_status_code.to_string(),
            },
            decode_grpc_error,
        );
        span.record("status", "error");
        span.record("error.type", error.error_type.as_str());
        span.record("exception.message", field::display(error.message.as_str()));
        span.set_status(OtelStatus::error(error.message));
    }
}

struct GrpcErrorTelemetry {
    error_type: String,
    message: String,
}

fn decode_grpc_error(status: &Status) -> GrpcErrorTelemetry {
    for detail in status.get_error_details_vec() {
        if let ErrorDetail::ErrorInfo(info) = detail
            && info.domain == CORAL_ERROR_DOMAIN
        {
            return GrpcErrorTelemetry {
                error_type: info.reason,
                message: status.message().to_string(),
            };
        }
    }

    GrpcErrorTelemetry {
        error_type: grpc_response_status_code(status.code()).to_string(),
        message: status.message().to_string(),
    }
}

pub(crate) fn query_status(error: QueryManagerError) -> Status {
    match error {
        QueryManagerError::App(error) => app_status(error),
        QueryManagerError::Core(error) => core_status(error),
    }
}

pub(crate) fn workspace_name_from_proto(
    workspace: Option<&Workspace>,
) -> Result<WorkspaceName, Status> {
    let workspace = workspace
        .ok_or_else(|| app_status(AppError::InvalidInput("missing workspace".to_string())))?;
    WorkspaceName::parse(&workspace.name).map_err(app_status)
}

pub(crate) fn workspace_to_proto(workspace_name: &WorkspaceName) -> Workspace {
    Workspace {
        name: workspace_name.as_str().to_string(),
    }
}

pub(crate) fn analytical_plan_to_proto(plan: &app_plan::AnalyticalPlan) -> AnalyticalPlanProto {
    AnalyticalPlanProto {
        plan_id: plan.plan_id.as_str().to_string(),
        workspace: plan.workspace.as_str().to_string(),
        intent: Some(plan_intent_to_proto(&plan.query)),
        workspace_sources_loaded: plan
            .workspace_sources_loaded
            .iter()
            .map(|source| source.as_str().to_string())
            .collect(),
        referenced_tables: Vec::new(),
        steps: vec![plan_step_to_proto(&plan.query)],
        policy_checks: vec![local_default_policy_check_to_proto()],
        cost_estimate: Some(unknown_cost_estimate_to_proto()),
        execution: Some(plan_execution_to_proto(&plan.execution)),
        memory_status: MemoryStatusProto::NotApplicable as i32,
    }
}

fn plan_intent_to_proto(query: &app_plan::PlannedQuery) -> PlanIntentProto {
    match query {
        app_plan::PlannedQuery::Sql(sql) => PlanIntentProto {
            intent: Some(plan_intent::Intent::Sql(SqlIntentProto {
                sql: sql.clone(),
            })),
        },
    }
}

fn plan_step_to_proto(query: &app_plan::PlannedQuery) -> PlanStepProto {
    match query {
        app_plan::PlannedQuery::Sql(sql) => PlanStepProto {
            step: Some(plan_step::Step::RelationalSql(RelationalSqlStepProto {
                sql: sql.clone(),
            })),
        },
    }
}

fn local_default_policy_check_to_proto() -> PolicyCheckProto {
    PolicyCheckProto {
        check: Some(policy_check::Check::LocalDefault(
            LocalDefaultPolicyCheckProto {
                status: PolicyCheckStatusProto::NotEvaluated as i32,
            },
        )),
    }
}

fn unknown_cost_estimate_to_proto() -> CostEstimateProto {
    CostEstimateProto {
        status: CostEstimateStatusProto::Unknown as i32,
    }
}

fn plan_execution_to_proto(execution: &app_plan::PlanExecution) -> PlanExecutionProto {
    PlanExecutionProto {
        status: match execution.status {
            app_plan::PlanExecutionStatus::NotStarted => PlanExecutionStatusProto::NotStarted,
            app_plan::PlanExecutionStatus::Running => PlanExecutionStatusProto::Running,
            app_plan::PlanExecutionStatus::Ok => PlanExecutionStatusProto::Ok,
            app_plan::PlanExecutionStatus::Error => PlanExecutionStatusProto::Error,
        } as i32,
        trace_id: execution
            .trace_id
            .as_deref()
            .unwrap_or_default()
            .to_string(),
        row_count: execution.row_count.unwrap_or_default(),
        row_count_recorded: execution.row_count.is_some(),
        error_type: execution
            .error_type
            .as_ref()
            .map(app_plan::PlanError::as_str)
            .unwrap_or_default()
            .to_string(),
        warnings: execution
            .warnings
            .iter()
            .map(plan_warning_to_proto)
            .collect(),
        started_at: execution
            .started_at
            .map(|timestamp| timestamp.to_rfc3339())
            .unwrap_or_default(),
        completed_at: execution
            .completed_at
            .map(|timestamp| timestamp.to_rfc3339())
            .unwrap_or_default(),
    }
}

fn plan_warning_to_proto(warning: &app_plan::PlanWarning) -> PlanWarningProto {
    match warning {
        app_plan::PlanWarning::SourceSkipped { source, message } => PlanWarningProto {
            warning: Some(plan_warning::Warning::SourceSkipped(
                SourceSkippedWarningProto {
                    source: source.as_str().to_string(),
                    message: message.clone(),
                },
            )),
        },
    }
}

pub(crate) fn table_to_proto(
    workspace_name: &WorkspaceName,
    table: coral_engine::TableInfo,
) -> Table {
    table_to_proto_with_columns(workspace_name, table)
}

pub(crate) fn table_summary_to_proto(
    workspace_name: &WorkspaceName,
    table: coral_engine::TableInfo,
) -> TableSummary {
    TableSummary {
        workspace: Some(workspace_to_proto(workspace_name)),
        schema_name: table.schema_name,
        name: table.table_name,
        description: table.description,
        required_filters: table.required_filters,
        guide: table.guide,
    }
}

fn table_to_proto_with_columns(
    workspace_name: &WorkspaceName,
    table: coral_engine::TableInfo,
) -> Table {
    let columns = table
        .columns
        .into_iter()
        .map(|column| Column {
            name: column.name,
            data_type: column.data_type,
            nullable: column.nullable,
            is_virtual: column.is_virtual,
            is_required_filter: column.is_required_filter,
            description: column.description,
            ordinal_position: column.ordinal_position,
        })
        .collect();

    Table {
        workspace: Some(workspace_to_proto(workspace_name)),
        schema_name: table.schema_name,
        name: table.table_name,
        description: table.description,
        columns,
        required_filters: table.required_filters,
        guide: table.guide,
    }
}

pub(crate) fn query_test_result_to_proto(
    result: &coral_engine::QueryTestResult,
) -> QueryTestResult {
    let outcome = match result.result() {
        Ok(success) => Some(query_test_result::Outcome::Success(QueryTestSuccess {
            row_count: success.row_count(),
        })),
        Err(failure) => Some(query_test_result::Outcome::Failure(QueryTestFailure {
            error_message: failure.error_message().to_string(),
        })),
    };
    QueryTestResult {
        sql: result.sql().to_string(),
        outcome,
    }
}

pub(crate) fn validate_source_response_to_proto(
    source: Source,
    workspace_name: &WorkspaceName,
    report: coral_engine::SourceValidationReport,
) -> ValidateSourceResponse {
    let coral_engine::SourceValidationReport {
        tables,
        query_tests,
    } = report;
    ValidateSourceResponse {
        source: Some(source),
        tables: tables
            .into_iter()
            .map(|table| table_to_proto(workspace_name, table))
            .collect(),
        query_tests: query_tests.iter().map(query_test_result_to_proto).collect(),
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "proto shape assertions intentionally fail loudly in tests"
    )]

    use chrono::TimeZone as _;
    use coral_api::{
        grpc_response_status_code,
        v1::{
            CostEstimateStatus, MemoryStatus, PlanExecutionStatus, PolicyCheckStatus,
            QueryTestFailure, Workspace, plan_intent, plan_step, plan_warning, policy_check,
            query_test_result,
        },
    };
    use tonic::{Code, Request};

    use super::{
        GrpcMethodMetadata, GrpcServerMethod, analytical_plan_to_proto, grpc_method, query_status,
        query_test_result_to_proto, table_summary_to_proto, table_to_proto,
        workspace_name_from_proto, workspace_to_proto,
    };
    use crate::bootstrap::AppError;
    use crate::plan::{AnalyticalPlan, PlanWarning};
    use crate::query::manager::QueryManagerError;
    use crate::sources::SourceName;
    use crate::workspaces::WorkspaceName;
    use coral_engine::{
        ColumnInfo, CoreError, QueryTestResult as EngineQueryTestResult, TableInfo,
    };

    #[test]
    fn query_status_maps_app_errors() {
        let status = query_status(QueryManagerError::App(AppError::SourceNotFound(
            "users".to_string(),
        )));

        assert_eq!(status.code(), Code::NotFound);
        assert_eq!(status.message(), "source 'users' not found");
    }

    #[test]
    fn query_status_maps_core_errors() {
        let status = query_status(QueryManagerError::Core(CoreError::Unavailable(
            "backend down".to_string(),
        )));

        assert_eq!(status.code(), Code::Unavailable);
        assert_eq!(status.message(), "unavailable: backend down");
    }

    #[test]
    fn grpc_response_status_codes_use_otel_names() {
        assert_eq!(grpc_response_status_code(Code::Ok), "OK");
        assert_eq!(
            grpc_response_status_code(Code::InvalidArgument),
            "INVALID_ARGUMENT"
        );
        assert_eq!(grpc_response_status_code(Code::Unavailable), "UNAVAILABLE");
    }

    #[test]
    fn grpc_server_method_derives_from_uri_path() {
        assert_eq!(
            GrpcServerMethod::from_path("/coral.v1.QueryService/ExecuteSql"),
            Some(GrpcServerMethod {
                service: "coral.v1.QueryService".to_string(),
                method: "ExecuteSql".to_string(),
            })
        );
        assert_eq!(GrpcServerMethod::from_path("/missing-method"), None);
        assert_eq!(
            GrpcServerMethod::from_path("/coral.v1.QueryService/Extra/Path"),
            None
        );
    }

    #[test]
    fn grpc_method_reads_server_method_from_request_extensions() {
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(GrpcServerMethod::from_path("/coral.v1.QueryService/ExecuteSql").unwrap());

        assert_eq!(
            grpc_method(&request),
            GrpcMethodMetadata::new("coral.v1.QueryService", "ExecuteSql")
        );
    }

    #[test]
    fn workspace_name_from_proto_rejects_missing_workspace() {
        let status = workspace_name_from_proto(None).expect_err("workspace should be required");

        assert_eq!(status.code(), Code::InvalidArgument);
        assert_eq!(status.message(), "invalid input: missing workspace");
    }

    #[test]
    fn workspace_name_from_proto_parses_valid_workspace() {
        let workspace = Workspace {
            name: "default".to_string(),
        };

        let workspace_name =
            workspace_name_from_proto(Some(&workspace)).expect("workspace should parse");

        assert_eq!(workspace_name.as_str(), "default");
    }

    #[test]
    fn analytical_plan_to_proto_preserves_execution_evidence() {
        let workspace_name = WorkspaceName::parse("default").expect("workspace");
        let started_at = chrono::Utc
            .with_ymd_and_hms(2026, 5, 14, 10, 0, 0)
            .single()
            .expect("started at");
        let completed_at = chrono::Utc
            .with_ymd_and_hms(2026, 5, 14, 10, 0, 1)
            .single()
            .expect("completed at");
        let mut plan = AnalyticalPlan::from_sql(&workspace_name, "SELECT 1");
        plan.record_source_load(
            vec![SourceName::parse("local_messages").expect("source name")],
            vec![PlanWarning::source_skipped(
                SourceName::parse("secured_messages").expect("source name"),
                "missing secret 'API_TOKEN'",
            )],
        );
        plan.mark_execution_started(started_at);
        plan.record_success(0, Some("trace-123".to_string()), completed_at);

        let proto = analytical_plan_to_proto(&plan);

        assert_eq!(proto.plan_id, plan.plan_id.as_str());
        assert_eq!(proto.workspace, "default");
        assert!(proto.referenced_tables.is_empty());
        let intent = proto.intent.expect("intent");
        assert!(matches!(
            intent.intent,
            Some(plan_intent::Intent::Sql(sql)) if sql.sql == "SELECT 1"
        ));
        assert_eq!(proto.workspace_sources_loaded, vec!["local_messages"]);
        assert!(matches!(
            proto.steps.first().expect("step").step.as_ref(),
            Some(plan_step::Step::RelationalSql(sql)) if sql.sql == "SELECT 1"
        ));
        assert!(matches!(
            proto.policy_checks.first().expect("policy").check.as_ref(),
            Some(policy_check::Check::LocalDefault(check))
                if check.status == PolicyCheckStatus::NotEvaluated as i32
        ));
        assert_eq!(
            proto.cost_estimate.expect("cost estimate").status,
            CostEstimateStatus::Unknown as i32
        );
        assert_eq!(proto.memory_status, MemoryStatus::NotApplicable as i32);
        let execution = proto.execution.expect("execution");
        assert_eq!(execution.status, PlanExecutionStatus::Ok as i32);
        assert_eq!(execution.trace_id, "trace-123");
        assert_eq!(execution.row_count, 0);
        assert!(execution.row_count_recorded);
        assert_eq!(execution.started_at, started_at.to_rfc3339());
        assert_eq!(execution.completed_at, completed_at.to_rfc3339());
        let warning = execution.warnings.first().expect("warning");
        assert!(matches!(
            warning.warning.as_ref(),
            Some(plan_warning::Warning::SourceSkipped(warning))
                if warning.source == "secured_messages" && warning.message.contains("API_TOKEN")
        ));
    }

    #[test]
    fn table_to_proto_preserves_table_metadata() {
        let workspace_name = WorkspaceName::parse("default").expect("workspace");
        let table = TableInfo {
            schema_name: "demo".to_string(),
            table_name: "users".to_string(),
            description: "User records".to_string(),
            guide: "Filter by org_id.".to_string(),
            columns: vec![ColumnInfo {
                name: "id".to_string(),
                data_type: "Int64".to_string(),
                nullable: false,
                is_virtual: false,
                is_required_filter: true,
                description: "User id".to_string(),
                ordinal_position: 0,
            }],
            required_filters: vec!["org_id".to_string()],
        };

        let proto = table_to_proto(&workspace_name, table);

        assert_eq!(proto.workspace, Some(workspace_to_proto(&workspace_name)));
        assert_eq!(proto.schema_name, "demo");
        assert_eq!(proto.name, "users");
        assert_eq!(proto.description, "User records");
        assert_eq!(proto.guide, "Filter by org_id.");
        assert_eq!(proto.columns.len(), 1);
        assert_eq!(proto.columns[0].name, "id");
        assert_eq!(proto.columns[0].data_type, "Int64");
        assert!(!proto.columns[0].nullable);
        assert!(!proto.columns[0].is_virtual);
        assert!(proto.columns[0].is_required_filter);
        assert_eq!(proto.columns[0].description, "User id");
        assert_eq!(proto.columns[0].ordinal_position, 0);
        assert_eq!(proto.required_filters, vec!["org_id"]);
    }

    #[test]
    fn table_summary_to_proto_preserves_table_metadata_without_columns() {
        let workspace_name = WorkspaceName::parse("default").expect("workspace");
        let table = TableInfo {
            schema_name: "demo".to_string(),
            table_name: "users".to_string(),
            description: "User records".to_string(),
            guide: "Filter by org_id.".to_string(),
            columns: vec![ColumnInfo {
                name: "id".to_string(),
                data_type: "Int64".to_string(),
                nullable: false,
                is_virtual: false,
                is_required_filter: true,
                description: "User id".to_string(),
                ordinal_position: 0,
            }],
            required_filters: vec!["org_id".to_string()],
        };

        let proto = table_summary_to_proto(&workspace_name, table);

        assert_eq!(proto.workspace, Some(workspace_to_proto(&workspace_name)));
        assert_eq!(proto.schema_name, "demo");
        assert_eq!(proto.name, "users");
        assert_eq!(proto.description, "User records");
        assert_eq!(proto.guide, "Filter by org_id.");
        assert_eq!(proto.required_filters, vec!["org_id"]);
    }

    #[test]
    fn query_test_result_to_proto_preserves_result_metadata() {
        let proto = query_test_result_to_proto(&EngineQueryTestResult::failure(
            "SELECT 1",
            "failed precondition: boom",
        ));

        assert_eq!(proto.sql, "SELECT 1");
        assert!(matches!(
            proto.outcome,
            Some(query_test_result::Outcome::Failure(QueryTestFailure { error_message }))
                if error_message == "failed precondition: boom"
        ));
    }
}
