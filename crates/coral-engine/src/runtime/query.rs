//! Concrete `DataFusion` runtime assembly for the data plane.

use std::sync::Arc;

use arrow::array::{Array, Int64Array, UInt64Array};
use coral_spec::WriteEffect;
use datafusion::dataframe::DataFrame;
use datafusion::datasource::DefaultTableSource;
use datafusion::execution::SessionStateBuilder;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::logical_expr::{LogicalPlan, Projection, TableScan};
use datafusion::physical_plan::{collect as collect_physical_plan, displayable};
use datafusion::prelude::{SQLOptions, SessionConfig, SessionContext};
use datafusion_tracing::{InstrumentationOptions, RuleInstrumentationOptions};

use crate::backends::compile_query_source;
use crate::backends::http::function::HttpSourceFunctionCallTableProvider;
use crate::runtime::catalog;
use crate::runtime::error::{
    datafusion_to_core, datafusion_to_core_with_sql, query_result_observer_error_to_core,
};
use crate::runtime::json::register_json_support;
use crate::runtime::pattern_validator::register_pattern_validator;
use crate::runtime::registry::{
    CompiledQuerySource, SourceRegistrationCandidate, SourceRegistrationFailure, register_sources,
};
use crate::runtime::source_functions::SourceFunctionRegistry;
use crate::{
    CoreError, QueryExecution, QueryPlan, QueryResultObserver, QueryResultObserverError,
    QueryRuntimeConfig, QuerySource, RelationInfo, SqlExecutionSummary,
};

pub(crate) struct QueryRuntimeAdapter {
    ctx: Arc<SessionContext>,
    tables: Vec<RelationInfo>,
    failures: Vec<SourceRegistrationFailure>,
    query_result_observers: Vec<Arc<dyn QueryResultObserver>>,
}

pub(crate) async fn build_runtime(
    sources: &[QuerySource],
    runtime: QueryRuntimeConfig,
) -> Result<QueryRuntimeAdapter, CoreError> {
    let session_config = SessionConfig::new().with_information_schema(true);
    let runtime_env = Arc::new(
        RuntimeEnvBuilder::new()
            .with_object_list_cache_limit(0)
            .build()
            .map_err(|err| datafusion_to_core(&err, &[]))?,
    );
    let exec_options = InstrumentationOptions::builder()
        .record_metrics(true)
        .build();
    let instrument_rule = datafusion_tracing::instrument_with_trace_spans!(
        target: "coral_engine::datafusion",
        options: exec_options
    );
    let session_state = SessionStateBuilder::new()
        .with_config(session_config)
        .with_runtime_env(runtime_env)
        .with_default_features()
        .with_physical_optimizer_rule(instrument_rule)
        .build();
    let session_state = datafusion_tracing::instrument_rules_with_trace_spans!(
        target: "coral_engine::datafusion",
        options: RuleInstrumentationOptions::full(),
        state: session_state
    );
    let mut ctx = SessionContext::new_with_state(session_state);
    register_json_support(&mut ctx).map_err(|err| datafusion_to_core(&err, &[]))?;
    register_pattern_validator(&mut ctx).map_err(|err| datafusion_to_core(&err, &[]))?;
    let ctx = Arc::new(ctx);

    let QueryRuntimeConfig {
        context: runtime_context,
        mut extensions,
    } = runtime;
    let mut source_candidates = Vec::new();
    for source in sources {
        match compile_query_source(source, &runtime_context, &extensions.request_authenticators) {
            Ok(compiled) => {
                source_candidates.push(SourceRegistrationCandidate::Compiled(
                    CompiledQuerySource {
                        source: source.clone(),
                        compiled,
                    },
                ));
            }
            Err(error) => source_candidates.push(SourceRegistrationCandidate::CompileFailed {
                source: source.clone(),
                error,
            }),
        }
    }
    let registration = register_sources(
        &ctx,
        source_candidates,
        extensions.source_decorators.as_mut_slice(),
    )
    .await?;
    catalog::register(&ctx, &registration.active_sources)
        .map_err(|err| datafusion_to_core(&err, &[]))?;
    let tables = catalog::collect_tables(&registration.active_sources);
    let source_functions = SourceFunctionRegistry::new(
        registration
            .active_sources
            .iter()
            .flat_map(|source| source.table_functions.iter()),
    );
    if !source_functions.is_empty() {
        ctx.register_relation_planner(Arc::new(source_functions))
            .map_err(|err| datafusion_to_core(&err, &tables))?;
    }
    for failure in &registration.failures {
        tracing::warn!(
            source = %failure.schema_name,
            detail = %failure.detail,
            "skipping source during runtime build"
        );
    }

    Ok(QueryRuntimeAdapter {
        ctx,
        tables,
        failures: registration.failures,
        query_result_observers: extensions.query_result_observers,
    })
}

impl QueryRuntimeAdapter {
    pub(crate) fn list_relations(
        &self,
        source_filter: Option<&str>,
        relation_filter: Option<&str>,
    ) -> Vec<RelationInfo> {
        self.tables
            .iter()
            .filter(|relation| source_filter.is_none_or(|value| relation.schema_name == value))
            .filter(|relation| relation_filter.is_none_or(|value| relation.relation_name == value))
            .cloned()
            .collect()
    }

    pub(crate) fn registration_failure(
        &self,
        source_name: &str,
    ) -> Option<&SourceRegistrationFailure> {
        self.failures
            .iter()
            .find(|failure| failure.schema_name == source_name)
    }

    pub(crate) async fn execute_sql(&self, sql: &str) -> Result<QueryExecution, CoreError> {
        let df = self.sql_dataframe(sql).await?;
        let (session_state, logical_plan) = df.into_parts();
        validate_coral_sql_plan(&logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        validate_effectful_function_plan(&logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let optimized_logical_plan = session_state
            .optimize(&logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        validate_coral_sql_plan(&optimized_logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        validate_effectful_function_plan(&optimized_logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let arrow_schema = Arc::new(optimized_logical_plan.schema().as_arrow().clone());
        let physical_plan = session_state
            .query_planner()
            .create_physical_plan(&optimized_logical_plan, &session_state)
            .await
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let batches = collect_physical_plan(physical_plan, session_state.task_ctx())
            .await
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let summary = sql_execution_summary(sql, &batches);
        self.observe_query_result(sql, arrow_schema.as_ref(), &batches)?;
        Ok(QueryExecution::new(arrow_schema, batches, summary))
    }

    fn observe_query_result(
        &self,
        sql: &str,
        schema: &arrow::datatypes::Schema,
        batches: &[arrow::record_batch::RecordBatch],
    ) -> Result<(), CoreError> {
        for observer in &self.query_result_observers {
            observer
                .observe_result(sql, schema, batches)
                .map_err(|error| query_result_observer_error(observer.name(), &error))?;
        }
        Ok(())
    }

    pub(crate) async fn explain_sql(&self, sql: &str) -> Result<QueryPlan, CoreError> {
        let df = self.sql_dataframe(sql).await?;
        validate_coral_sql_plan(df.logical_plan())
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        validate_effectful_function_plan(df.logical_plan())
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let unoptimized_logical_plan = df.logical_plan().display_indent_schema().to_string();
        let (session_state, logical_plan) = df.into_parts();
        let optimized_logical_plan = session_state
            .optimize(&logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        validate_coral_sql_plan(&optimized_logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        validate_effectful_function_plan(&optimized_logical_plan)
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let optimized_logical_plan_display =
            optimized_logical_plan.display_indent_schema().to_string();
        let physical_plan = session_state
            .query_planner()
            .create_physical_plan(&optimized_logical_plan, &session_state)
            .await
            .map_err(|err| datafusion_to_core(&err, &self.tables))?;
        let physical_plan = displayable(physical_plan.as_ref())
            .set_show_schema(true)
            .indent(true)
            .to_string();

        Ok(QueryPlan::new(
            unoptimized_logical_plan,
            optimized_logical_plan_display,
            physical_plan,
        ))
    }

    async fn sql_dataframe(&self, sql: &str) -> Result<DataFrame, CoreError> {
        self.ctx
            .sql_with_options(sql, coral_sql_options())
            .await
            .map_err(|err| datafusion_to_core_with_sql(&err, &self.tables, Some(sql)))
    }
}

fn coral_sql_options() -> SQLOptions {
    SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(true)
        .with_allow_statements(false)
}

fn validate_coral_sql_plan(plan: &LogicalPlan) -> datafusion::error::Result<()> {
    if matches!(plan, LogicalPlan::Copy(_)) {
        return Err(datafusion::error::DataFusionError::Plan(
            "COPY is not supported by Coral SQL".to_string(),
        ));
    }
    for input in plan.inputs() {
        validate_coral_sql_plan(input)?;
    }
    Ok(())
}

fn validate_effectful_function_plan(plan: &LogicalPlan) -> datafusion::error::Result<()> {
    let mut scans = Vec::new();
    collect_effectful_function_scans(plan, &mut scans);
    match scans.len() {
        0 => Ok(()),
        1 if is_allowed_effectful_function_shape(plan) => Ok(()),
        1 => Err(datafusion::error::DataFusionError::Plan(format!(
            "effectful SQL function {} must be called as a top-level simple SELECT; joins, filters, limits, subqueries, CTE reuse, and nested use are not supported",
            scans.first().expect("length checked above")
        ))),
        _ => Err(datafusion::error::DataFusionError::Plan(format!(
            "effectful SQL functions must execute exactly once; found {} effectful function scans: {}",
            scans.len(),
            scans.join(", ")
        ))),
    }
}

fn collect_effectful_function_scans(plan: &LogicalPlan, scans: &mut Vec<String>) {
    if let Some(provider) = effectful_function_scan(plan) {
        scans.push(format!(
            "{} ({}, {})",
            provider.qualified_name(),
            provider.effect().as_str(),
            provider.idempotency().as_str()
        ));
    }
    for input in plan.inputs() {
        collect_effectful_function_scans(input, scans);
    }
}

fn is_allowed_effectful_function_shape(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Projection(Projection { input, .. }) => {
            simple_effectful_function_leaf(input.as_ref())
        }
        other => simple_effectful_function_leaf(other),
    }
}

fn simple_effectful_function_leaf(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::TableScan(scan) => effectful_function_table_scan(scan).is_some(),
        LogicalPlan::SubqueryAlias(alias) => simple_effectful_function_leaf(alias.input.as_ref()),
        _ => false,
    }
}

fn effectful_function_scan(plan: &LogicalPlan) -> Option<&HttpSourceFunctionCallTableProvider> {
    match plan {
        LogicalPlan::TableScan(scan) => effectful_function_table_provider(scan),
        _ => None,
    }
}

fn effectful_function_table_scan(scan: &TableScan) -> Option<&HttpSourceFunctionCallTableProvider> {
    let provider = effectful_function_table_provider(scan)?;
    (scan.filters.is_empty() && scan.fetch.is_none()).then_some(provider)
}

fn effectful_function_table_provider(
    scan: &TableScan,
) -> Option<&HttpSourceFunctionCallTableProvider> {
    let table_source = scan.source.as_any().downcast_ref::<DefaultTableSource>()?;
    let provider = table_source
        .table_provider
        .as_any()
        .downcast_ref::<HttpSourceFunctionCallTableProvider>()?;
    (provider.effect() != WriteEffect::Read).then_some(provider)
}

fn sql_execution_summary(
    sql: &str,
    batches: &[arrow::record_batch::RecordBatch],
) -> SqlExecutionSummary {
    let statement_kind = first_sql_keyword(sql).unwrap_or("unknown");
    let effect = match statement_kind {
        "insert" | "update" => "write",
        "delete" | "truncate" => "destructive",
        _ => "read",
    };
    let affected_row_count =
        if matches!(statement_kind, "insert" | "update" | "delete" | "truncate") {
            first_count_value(batches).unwrap_or(0)
        } else {
            0
        };
    SqlExecutionSummary::new(statement_kind, effect, affected_row_count)
}

fn first_sql_keyword(sql: &str) -> Option<&'static str> {
    let keyword = sql
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || ch == '(')
        .next()?
        .to_ascii_lowercase();
    match keyword.as_str() {
        "select" => Some("select"),
        "insert" => Some("insert"),
        "update" => Some("update"),
        "delete" => Some("delete"),
        "truncate" => Some("truncate"),
        "explain" => Some("explain"),
        _ => Some("unknown"),
    }
}

fn first_count_value(batches: &[arrow::record_batch::RecordBatch]) -> Option<u64> {
    let batch = batches.first()?;
    let column = batch.columns().first()?;
    if let Some(values) = column.as_any().downcast_ref::<UInt64Array>() {
        return (!values.is_empty() && !values.is_null(0)).then(|| values.value(0));
    }
    if let Some(values) = column.as_any().downcast_ref::<Int64Array>() {
        return (!values.is_empty() && !values.is_null(0))
            .then(|| u64::try_from(values.value(0)).ok())
            .flatten();
    }
    None
}

fn query_result_observer_error(name: &str, error: &QueryResultObserverError) -> CoreError {
    let core = query_result_observer_error_to_core(error);
    match core {
        CoreError::InvalidInput(detail) => {
            CoreError::InvalidInput(format!("query result observer '{name}': {detail}"))
        }
        CoreError::FailedPrecondition(detail) => {
            CoreError::FailedPrecondition(format!("query result observer '{name}': {detail}"))
        }
        other => other,
    }
}
