//! Query-time loading, validation, and execution over installed sources.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use coral_engine::{
    CoralQuery, CoreError, QueryExecution, QueryPlan, QueryRuntimeConfig, QueryRuntimeContext,
    QuerySource, SourceValidationReport, StatusCode, TableInfo,
};
use coral_spec::{ManifestInputKind, ManifestInputSpec};
use opentelemetry::{
    KeyValue,
    trace::{Status as OtelStatus, TraceContextExt as _},
};
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::bootstrap::AppError;
use crate::plan::{AnalyticalPlan, PlanError, PlanErrorCode, PlanId, PlanWarning};
use crate::query::extensions::{EngineExtensionsProvider, engine_extensions_for_providers};
use crate::sources::SourceName;
use crate::sources::catalog::resolve_installed_manifest;
use crate::sources::model::InstalledSource;
use crate::state::{AppStateLayout, ConfigStore, SecretStore};
use crate::workspaces::WorkspaceName;

#[derive(Debug)]
pub(crate) enum QueryManagerError {
    App(AppError),
    Core(CoreError),
}

pub(crate) struct ValidatedSource {
    pub(crate) source: InstalledSource,
    pub(crate) report: SourceValidationReport,
}

pub(crate) struct PlannedQueryExecution {
    pub(crate) plan: AnalyticalPlan,
    pub(crate) result: Result<QueryExecution, QueryManagerError>,
}

struct LoadedQuerySources {
    sources: Vec<QuerySource>,
    source_names: Vec<SourceName>,
    warnings: Vec<PlanWarning>,
}

#[derive(Clone)]
pub(crate) struct QueryManager {
    config_store: ConfigStore,
    secret_store: SecretStore,
    runtime_context: QueryRuntimeContext,
    layout: AppStateLayout,
    engine_extensions_providers: Vec<Arc<dyn EngineExtensionsProvider>>,
}

impl QueryManager {
    pub(crate) fn new(
        config_store: ConfigStore,
        secret_store: SecretStore,
        runtime_context: QueryRuntimeContext,
        layout: AppStateLayout,
        engine_extensions_providers: Vec<Arc<dyn EngineExtensionsProvider>>,
    ) -> Self {
        Self {
            config_store,
            secret_store,
            runtime_context,
            layout,
            engine_extensions_providers,
        }
    }

    pub(crate) async fn list_tables(
        &self,
        workspace_name: &WorkspaceName,
        schema_filter: Option<&str>,
        table_filter: Option<&str>,
    ) -> Result<Vec<TableInfo>, QueryManagerError> {
        let sources = self
            .load_query_sources(workspace_name)
            .map_err(QueryManagerError::App)?;
        let runtime = self.runtime_config(&sources);
        CoralQuery::list_tables(&sources, runtime, schema_filter, table_filter)
            .await
            .map_err(QueryManagerError::Core)
    }

    pub(crate) async fn execute_sql_plan(
        &self,
        workspace_name: &WorkspaceName,
        sql: &str,
    ) -> PlannedQueryExecution {
        let mut plan = AnalyticalPlan::from_sql(workspace_name, sql);
        plan.mark_execution_started(Utc::now());
        let plan_id = plan.plan_id.clone();
        let operation = run_query_operation(
            QueryOperation::ExecuteSql,
            workspace_name,
            sql,
            Some(&plan_id),
            async {
                let LoadedQuerySources {
                    sources,
                    source_names,
                    warnings,
                } = self
                    .load_query_sources_with_report(workspace_name)
                    .map_err(QueryManagerError::App)?;
                plan.record_source_load(source_names, warnings);
                let runtime = self.runtime_config(&sources);
                CoralQuery::execute_sql(&sources, runtime, sql)
                    .await
                    .map_err(QueryManagerError::Core)
            },
            |execution| Some(u64::try_from(execution.row_count()).unwrap_or(u64::MAX)),
        )
        .await;

        match &operation.result {
            Ok(execution) => plan.record_success(
                u64::try_from(execution.row_count()).unwrap_or(u64::MAX),
                operation.trace_id.clone(),
                Utc::now(),
            ),
            Err(error) => {
                plan.record_error(
                    query_error_type(error),
                    operation.trace_id.clone(),
                    Utc::now(),
                );
            }
        }

        PlannedQueryExecution {
            plan,
            result: operation.result,
        }
    }

    pub(crate) async fn explain_sql(
        &self,
        workspace_name: &WorkspaceName,
        sql: &str,
    ) -> Result<QueryPlan, QueryManagerError> {
        run_query_operation(
            QueryOperation::ExplainSql,
            workspace_name,
            sql,
            None,
            async {
                let sources = self
                    .load_query_sources(workspace_name)
                    .map_err(QueryManagerError::App)?;
                let runtime = self.runtime_config(&sources);
                CoralQuery::explain_sql(&sources, runtime, sql)
                    .await
                    .map_err(QueryManagerError::Core)
            },
            |_| None,
        )
        .await
        .result
    }

    pub(crate) async fn validate_source(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<ValidatedSource, QueryManagerError> {
        let source = self
            .config_store
            .get_source(workspace_name, source_name)
            .map_err(QueryManagerError::App)?;
        let (query_source, version) = self
            .load_query_source(workspace_name, &source)
            .map_err(QueryManagerError::App)?;
        let runtime = self.runtime_config(std::slice::from_ref(&query_source));
        let report = CoralQuery::validate_source(
            &query_source,
            runtime,
            query_source.source_spec().test_queries(),
        )
        .await
        .map_err(QueryManagerError::Core)?;
        let mut source = source;
        source.version = Some(version);

        Ok(ValidatedSource { source, report })
    }

    fn load_query_sources(
        &self,
        workspace_name: &WorkspaceName,
    ) -> Result<Vec<QuerySource>, AppError> {
        self.load_query_sources_with_report(workspace_name)
            .map(|loaded| loaded.sources)
    }

    fn load_query_sources_with_report(
        &self,
        workspace_name: &WorkspaceName,
    ) -> Result<LoadedQuerySources, AppError> {
        let catalog = self.config_store.load_catalog()?;
        let mut sources = Vec::new();
        let mut source_names = Vec::new();
        let mut warnings = Vec::new();
        for source in catalog.workspace_sources(workspace_name) {
            match self.load_query_source(workspace_name, &source) {
                Ok((query_source, _version)) => {
                    source_names.push(source.name.clone());
                    sources.push(query_source);
                }
                Err(error) => {
                    tracing::warn!(
                        source = %source.name,
                        detail = %error,
                        "skipping source during query-source load"
                    );
                    warnings.push(PlanWarning::source_skipped(
                        source.name.clone(),
                        error.to_string(),
                    ));
                }
            }
        }
        Ok(LoadedQuerySources {
            sources,
            source_names,
            warnings,
        })
    }

    fn load_query_source(
        &self,
        workspace_name: &WorkspaceName,
        source: &InstalledSource,
    ) -> Result<(QuerySource, String), AppError> {
        let installed = resolve_installed_manifest(workspace_name, source, &self.layout)?;
        let source_spec = installed.source_spec;
        validate_required_variables(source, source_spec.declared_inputs())?;
        let stored_secrets = self
            .secret_store
            .read_source_secrets_for(workspace_name, &source.name)?;
        let mut resolved_secrets = BTreeMap::new();
        let missing_secrets: Vec<String> = source_spec
            .required_secret_names()
            .into_iter()
            .filter(|name| !stored_secrets.contains_key(name))
            .collect();
        if let Some((first, rest)) = missing_secrets.split_first() {
            let detail = if rest.is_empty() {
                format!("secret '{first}'")
            } else {
                format!("secret '{first}' and {} other(s)", rest.len())
            };
            return Err(AppError::FailedPrecondition(format!(
                "source '{}' is missing {detail}",
                source.name
            )));
        }
        for secret_name in source_spec.required_secret_names() {
            let value = stored_secrets.get(&secret_name).cloned().ok_or_else(|| {
                AppError::FailedPrecondition(format!(
                    "source '{}' is missing secret '{secret_name}'",
                    source.name
                ))
            })?;
            resolved_secrets.insert(secret_name, value);
        }
        Ok((
            QuerySource::new(source_spec, source.variables.clone(), resolved_secrets),
            installed.candidate.version,
        ))
    }

    fn runtime_config(&self, selected_sources: &[QuerySource]) -> QueryRuntimeConfig {
        QueryRuntimeConfig::new(
            self.runtime_context.clone(),
            engine_extensions_for_providers(&self.engine_extensions_providers, selected_sources),
        )
    }
}

#[derive(Clone, Copy)]
enum QueryOperation {
    ExecuteSql,
    ExplainSql,
}

impl QueryOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ExecuteSql => "execute_sql",
            Self::ExplainSql => "explain_sql",
        }
    }
}

struct QueryOperationResult<T> {
    result: Result<T, QueryManagerError>,
    trace_id: Option<String>,
}

async fn run_query_operation<T, Fut, RowCount>(
    operation: QueryOperation,
    workspace_name: &WorkspaceName,
    sql: &str,
    plan_id: Option<&PlanId>,
    query: Fut,
    row_count: RowCount,
) -> QueryOperationResult<T>
where
    Fut: Future<Output = Result<T, QueryManagerError>>,
    RowCount: FnOnce(&T) -> Option<u64>,
{
    let started_at = Instant::now();
    let query_span = create_query_span(operation, workspace_name, sql, plan_id);
    let result = query.instrument(query_span.clone()).await;

    let metrics = crate::telemetry::metrics::metrics();
    let status = crate::telemetry::metrics::status_attr(result.is_ok());
    let attributes = [status, KeyValue::new("operation", operation.as_str())];
    metrics.count.add(1, &attributes);
    metrics
        .duration
        .record(started_at.elapsed().as_secs_f64(), &attributes);

    if let Ok(value) = &result {
        query_span.record("status", "ok");
        query_span.set_status(OtelStatus::Ok);
        if let Some(row_count) = row_count(value) {
            query_span.record("row_count", row_count);
            metrics.rows.record(row_count, &attributes);
        }
    } else if let Err(error) = &result {
        let error_kind = query_error_kind(error);
        let error_type = query_error_type(error);
        let error_message = query_error_message(error);
        query_span.record("status", "error");
        query_span.record("error.kind", error_kind);
        query_span.record("error.type", error_type.as_str());
        query_span.record("exception.message", error_message.as_str());
        query_span.set_status(OtelStatus::error(error_message));
    }

    QueryOperationResult {
        result,
        trace_id: span_trace_id(&query_span),
    }
}

fn create_query_span(
    operation: QueryOperation,
    workspace_name: &WorkspaceName,
    sql: &str,
    plan_id: Option<&PlanId>,
) -> tracing::Span {
    let operation = operation.as_str();
    let span = tracing::info_span!(
        "coral.query",
        otel.name = "coral.query",
        operation = operation,
        workspace = %workspace_name.as_str(),
        sql = %sql,
        plan_id = tracing::field::Empty,
        row_count = tracing::field::Empty,
        status = tracing::field::Empty,
        error.kind = tracing::field::Empty,
        error.type = tracing::field::Empty,
        exception.message = tracing::field::Empty,
    );
    if let Some(plan_id) = plan_id {
        span.record("plan_id", plan_id.as_str());
    }
    span
}

fn span_trace_id(span: &tracing::Span) -> Option<String> {
    let context = span.context();
    let otel_span = context.span();
    let span_context = otel_span.span_context();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

fn query_error_kind(error: &QueryManagerError) -> &'static str {
    match error {
        QueryManagerError::App(_) => "app",
        QueryManagerError::Core(_) => "core",
    }
}

fn query_error_type(error: &QueryManagerError) -> PlanError {
    match error {
        QueryManagerError::App(error) => PlanError::Code(app_error_type(error)),
        QueryManagerError::Core(error) => core_error_type(error),
    }
}

fn query_error_message(error: &QueryManagerError) -> String {
    match error {
        QueryManagerError::App(error) => error.to_string(),
        QueryManagerError::Core(CoreError::QueryFailure(error)) => error.summary().to_string(),
        QueryManagerError::Core(error) => error.to_string(),
    }
}

fn app_error_type(error: &AppError) -> PlanErrorCode {
    match error {
        AppError::SourceNotFound(_) => PlanErrorCode::NotFound,
        AppError::InvalidInput(_) => PlanErrorCode::InvalidArgument,
        AppError::FailedPrecondition(_) | AppError::Credentials(_) | AppError::MissingConfigDir => {
            PlanErrorCode::FailedPrecondition
        }
        AppError::Io(_)
        | AppError::Yaml(_)
        | AppError::TomlDecode(_)
        | AppError::TomlEncode(_)
        | AppError::Json(_)
        | AppError::Transport(_)
        | AppError::TaskJoin(_) => PlanErrorCode::Internal,
    }
}

fn core_error_type(error: &CoreError) -> PlanError {
    match error {
        CoreError::QueryFailure(error) => PlanError::QueryFailure(error.reason().to_string()),
        error => PlanError::Code(status_code_error_type(error.status_code())),
    }
}

fn status_code_error_type(status: StatusCode) -> PlanErrorCode {
    match status {
        StatusCode::InvalidArgument => PlanErrorCode::InvalidArgument,
        StatusCode::NotFound => PlanErrorCode::NotFound,
        StatusCode::FailedPrecondition => PlanErrorCode::FailedPrecondition,
        StatusCode::Unavailable => PlanErrorCode::Unavailable,
        StatusCode::Unimplemented => PlanErrorCode::Unimplemented,
        StatusCode::Internal => PlanErrorCode::Internal,
    }
}

fn validate_required_variables(
    source: &InstalledSource,
    inputs: &[ManifestInputSpec],
) -> Result<(), AppError> {
    let missing: Vec<_> = inputs
        .iter()
        .filter(|input| {
            input.kind == ManifestInputKind::Variable
                && input.required
                && !source.variables.contains_key(&input.key)
        })
        .collect();
    if let Some((first, rest)) = missing.split_first() {
        let detail = if rest.is_empty() {
            format!("variable '{}'", first.key)
        } else {
            format!("variable '{}' and {} other(s)", first.key, rest.len())
        };
        return Err(AppError::FailedPrecondition(format!(
            "source '{}' is missing {detail}",
            source.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::{PlannedQueryExecution, QueryManager};
    use crate::plan::{PlanError, PlanExecutionStatus, PlanId, PlanWarning};
    use crate::query::extensions::EngineExtensionsProvider;
    use crate::sources::SourceName;
    use crate::sources::manager::{
        ImportSourceCommand, SourceBinding, SourceBindings, SourceManager,
    };
    use crate::state::{AppStateLayout, ConfigStore, SecretStore};
    use crate::workspaces::WorkspaceName;
    use coral_engine::QueryRuntimeContext;

    struct QueryManagerHarness {
        _temp: TempDir,
        layout: AppStateLayout,
        workspace_name: WorkspaceName,
        source_manager: SourceManager,
        query_manager: QueryManager,
    }

    impl QueryManagerHarness {
        fn new() -> Self {
            let temp = TempDir::new().expect("temp dir");
            let layout =
                AppStateLayout::discover(Some(temp.path().join("coral-config"))).expect("layout");
            layout.ensure().expect("ensure layout");
            let config_store = ConfigStore::new(layout.clone());
            let secret_store = SecretStore::new(layout.clone());
            let source_manager =
                SourceManager::new(config_store.clone(), secret_store.clone(), layout.clone());
            let query_manager = QueryManager::new(
                config_store,
                secret_store,
                QueryRuntimeContext::default(),
                layout.clone(),
                Vec::<Arc<dyn EngineExtensionsProvider>>::new(),
            );

            Self {
                _temp: temp,
                layout,
                workspace_name: WorkspaceName::default(),
                source_manager,
                query_manager,
            }
        }

        fn import_local_messages_source(&self) {
            self.source_manager
                .import_source(
                    &self.workspace_name,
                    &ImportSourceCommand {
                        manifest_yaml: local_messages_manifest(self.layout.config_file()),
                        bindings: SourceBindings::default(),
                    },
                )
                .expect("import local messages source");
        }

        fn import_secured_messages_source(&self) {
            self.source_manager
                .import_source(
                    &self.workspace_name,
                    &ImportSourceCommand {
                        manifest_yaml: secured_messages_manifest(),
                        bindings: SourceBindings {
                            variables: Vec::new(),
                            secrets: vec![SourceBinding {
                                key: "API_TOKEN".to_string(),
                                value: "secret-token".to_string(),
                            }],
                        },
                    },
                )
                .expect("import secured messages source");
        }

        async fn execute_plan(&self, sql: &str) -> PlannedQueryExecution {
            self.query_manager
                .execute_sql_plan(&self.workspace_name, sql)
                .await
        }
    }

    #[tokio::test]
    async fn execute_sql_plan_records_zero_row_success_evidence() {
        let harness = QueryManagerHarness::new();
        harness.import_local_messages_source();

        let sql = "SELECT * FROM local_messages.messages WHERE text = 'missing'";
        let planned = harness.execute_plan(sql).await;
        let execution = planned.result.as_ref().expect("query should succeed");

        assert_eq!(execution.row_count(), 0);
        assert!(planned.plan.plan_id.as_str().starts_with("plan_"));
        assert_eq!(planned.plan.workspace.as_str(), "default");
        assert_eq!(
            planned.plan.workspace_sources_loaded,
            vec![SourceName::parse("local_messages").expect("source name")]
        );
        assert_eq!(planned.plan.execution.status, PlanExecutionStatus::Ok);
        assert_eq!(planned.plan.execution.row_count, Some(0));
        assert!(planned.plan.execution.started_at.is_some());
        assert!(planned.plan.execution.completed_at.is_some());
    }

    #[tokio::test]
    async fn execute_sql_plan_records_sql_error_evidence() {
        let harness = QueryManagerHarness::new();
        harness.import_local_messages_source();

        let planned = harness
            .execute_plan("SELECT * FROM missing_schema.missing_table")
            .await;

        planned.result.expect_err("query should fail");
        assert_eq!(planned.plan.execution.status, PlanExecutionStatus::Error);
        assert!(planned.plan.execution.row_count.is_none());
        assert!(
            matches!(
                planned.plan.execution.error_type,
                Some(PlanError::QueryFailure(_))
            ),
            "SQL errors should carry a stable error type"
        );
        assert_eq!(
            planned.plan.workspace_sources_loaded,
            vec![SourceName::parse("local_messages").expect("source name")]
        );
    }

    #[tokio::test]
    async fn execute_sql_plan_keeps_shell_when_source_load_is_skipped() {
        let harness = QueryManagerHarness::new();
        harness.import_secured_messages_source();
        let source_name = SourceName::parse("secured_messages").expect("source name");
        fs::remove_file(
            harness
                .layout
                .secret_file(&harness.workspace_name, &source_name),
        )
        .expect("remove persisted secret");

        let planned = harness
            .execute_plan("SELECT * FROM secured_messages.messages")
            .await;

        planned
            .result
            .expect_err("query should fail without source");
        assert_eq!(planned.plan.execution.status, PlanExecutionStatus::Error);
        assert!(planned.plan.workspace_sources_loaded.is_empty());
        let warning = planned
            .plan
            .execution
            .warnings
            .first()
            .expect("source load warning");
        let PlanWarning::SourceSkipped { source, message } = warning;
        assert_eq!(source.as_str(), "secured_messages");
        assert!(
            message.contains("missing secret 'API_TOKEN'"),
            "unexpected warning: {message}"
        );
    }

    #[test]
    fn query_span_records_plan_id_attribute() {
        use opentelemetry::Value as OtelValue;
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use tracing_subscriber::layer::SubscriberExt as _;

        let memory = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(memory.clone())
            .build();
        let tracer = provider.tracer("query-manager-test");
        let layer = tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_level(true);
        let subscriber = tracing_subscriber::Registry::default().with(layer);
        let workspace_name = WorkspaceName::default();
        let plan_id = PlanId::new();

        tracing::subscriber::with_default(subscriber, || {
            let span = super::create_query_span(
                super::QueryOperation::ExecuteSql,
                &workspace_name,
                "SELECT 1",
                Some(&plan_id),
            );
            let _guard = span.enter();
        });
        provider.force_flush().expect("flush spans");

        let spans = memory.get_finished_spans().expect("finished spans");
        let span = spans.first().expect("query span");
        let plan_id_attribute = span
            .attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "plan_id")
            .expect("plan_id attribute");

        match &plan_id_attribute.value {
            OtelValue::String(value) => assert_eq!(value.as_ref(), plan_id.as_str()),
            other => panic!("unexpected plan_id value: {other:?}"),
        }

        provider.shutdown().expect("provider shutdown");
    }

    fn local_messages_manifest(config_file: &std::path::Path) -> String {
        let data_dir = config_file
            .parent()
            .expect("config parent")
            .join("fixture-data");
        fs::create_dir_all(&data_dir).expect("create fixture data dir");
        fs::write(
            data_dir.join("messages.jsonl"),
            r#"{"type":"user","sessionId":"s1","text":"hello"}
{"type":"assistant","sessionId":"s1","text":"world"}
"#,
        )
        .expect("write fixture data");

        format!(
            r#"
name: local_messages
version: 0.1.0
dsl_version: 3
backend: jsonl
tables:
  - name: messages
    description: Fixture messages
    source:
      location: "file://{}/"
      glob: "**/*.jsonl"
    columns:
      - name: type
        type: Utf8
      - name: sessionId
        type: Utf8
      - name: text
        type: Utf8
"#,
            data_dir.display()
        )
    }

    fn secured_messages_manifest() -> String {
        r#"
name: secured_messages
version: 0.1.0
dsl_version: 3
backend: http
inputs:
  API_BASE:
    kind: variable
    default: https://example.com
  API_TOKEN:
    kind: secret
base_url: "{{input.API_BASE}}"
auth:
  type: HeaderAuth
  headers:
    - name: Authorization
      from: template
      template: Bearer {{input.API_TOKEN}}
tables:
  - name: messages
    description: Secured messages
    request:
      method: GET
      path: /messages
    response: {}
    columns:
      - name: id
        type: Utf8
"#
        .to_string()
    }
}
