//! Concrete `DataFusion` runtime assembly for the data plane.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::error::DataFusionError;
use datafusion::execution::SessionStateBuilder;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::{SQLOptions, SessionConfig, SessionContext};
use datafusion_tracing::{InstrumentationOptions, RuleInstrumentationOptions};

use crate::backends::compile_query_source;
use crate::runtime::catalog;
use crate::runtime::dependent_join::error::resolver_rows_exceeded;
use crate::runtime::dependent_join::optimizer;
use crate::runtime::dependent_join::planner::DependentJoinExtensionPlanner;
use crate::runtime::error::{
    datafusion_to_core, datafusion_to_core_with_sql, query_result_observer_error_to_core,
};
use crate::runtime::json::register_json_support;
use crate::runtime::pattern_validator::register_pattern_validator;
use crate::runtime::query_planner::CoralQueryPlanner;
use crate::runtime::registry::{
    CompiledQuerySource, SourceRegistrationCandidate, SourceRegistrationFailure, register_sources,
};
use crate::runtime::source_functions::SourceFunctionRegistry;
use crate::{
    CoreError, DependentJoinConfig, QueryExecution, QueryMemoryConfig, QueryResultObserver,
    QueryResultObserverError, QueryRuntimeConfig, QueryRuntimeContext, QuerySource,
    RequestAuthenticator, SourceDecorator, TableInfo,
};

pub(crate) struct QueryRuntimeAdapter {
    ctx: Arc<SessionContext>,
    fallback_runtime: Option<FallbackRuntimeConfig>,
    tables: Vec<TableInfo>,
    failures: Vec<SourceRegistrationFailure>,
    query_result_observers: Vec<Arc<dyn QueryResultObserver>>,
}

#[derive(Clone)]
struct FallbackRuntimeConfig {
    sources: Vec<QuerySource>,
    runtime_context: QueryRuntimeContext,
    dependent_join: DependentJoinConfig,
    memory: QueryMemoryConfig,
    request_authenticators: HashMap<String, Arc<dyn RequestAuthenticator>>,
}

struct RegisteredRuntime {
    ctx: Arc<SessionContext>,
    tables: Vec<TableInfo>,
    failures: Vec<SourceRegistrationFailure>,
}

enum SqlExecutionFailure {
    Planning(DataFusionError),
    Collection(DataFusionError),
    Observer(CoreError),
}

pub(crate) async fn build_runtime(
    sources: &[QuerySource],
    runtime: QueryRuntimeConfig,
) -> Result<QueryRuntimeAdapter, CoreError> {
    let QueryRuntimeConfig {
        context: runtime_context,
        memory,
        dependent_join,
        mut extensions,
    } = runtime;
    let request_authenticators = extensions.request_authenticators.clone();
    // Resolver-row overflow can retry without the dependent-join optimizer only
    // when runtime registration is replayable. Source decorators are mutable
    // one-shot registration hooks today, so decorated runtimes keep resolver-row
    // overflow as a hard error instead of applying decorators a second time with
    // potentially different side effects.
    let fallback_without_dependent_join = extensions.source_decorators.is_empty();
    let fallback_runtime = fallback_without_dependent_join.then(|| FallbackRuntimeConfig {
        sources: sources.to_vec(),
        runtime_context: runtime_context.clone(),
        dependent_join: dependent_join.clone(),
        memory: memory.clone(),
        request_authenticators: request_authenticators.clone(),
    });

    let primary = build_registered_runtime(
        sources,
        &runtime_context,
        &request_authenticators,
        extensions.source_decorators.as_mut_slice(),
        &dependent_join,
        &memory,
    )
    .await?;

    Ok(QueryRuntimeAdapter {
        ctx: primary.ctx,
        fallback_runtime,
        tables: primary.tables,
        failures: primary.failures,
        query_result_observers: extensions.query_result_observers,
    })
}

async fn build_registered_runtime(
    sources: &[QuerySource],
    runtime_context: &QueryRuntimeContext,
    request_authenticators: &HashMap<String, Arc<dyn RequestAuthenticator>>,
    source_decorators: &mut [Box<dyn SourceDecorator>],
    dependent_join: &DependentJoinConfig,
    memory: &QueryMemoryConfig,
) -> Result<RegisteredRuntime, CoreError> {
    let ctx = build_session_context(dependent_join, memory)?;
    let registration = register_runtime_sources(
        &ctx,
        sources,
        runtime_context,
        request_authenticators,
        source_decorators,
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

    Ok(RegisteredRuntime {
        ctx,
        tables,
        failures: registration.failures,
    })
}

fn build_session_context(
    dependent_join: &DependentJoinConfig,
    memory: &QueryMemoryConfig,
) -> Result<Arc<SessionContext>, CoreError> {
    let session_config = SessionConfig::new().with_information_schema(true);
    let mut runtime_env_builder = RuntimeEnvBuilder::new().with_object_list_cache_limit(0);
    if let Some(limit) = memory.limit {
        runtime_env_builder = runtime_env_builder.with_memory_limit(limit.as_bytes(), 1.0);
    }
    let runtime_env = Arc::new(
        runtime_env_builder
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
    let mut builder = SessionStateBuilder::new()
        .with_config(session_config)
        .with_runtime_env(runtime_env)
        .with_default_features();
    if dependent_join.optimizer_enabled() {
        builder = builder.with_optimizer_rule(Arc::new(optimizer::rule(dependent_join.clone())));
    }
    let session_state = builder
        .with_query_planner(Arc::new(CoralQueryPlanner::new(vec![Arc::new(
            DependentJoinExtensionPlanner,
        )])))
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
    Ok(Arc::new(ctx))
}

async fn register_runtime_sources(
    ctx: &SessionContext,
    sources: &[QuerySource],
    runtime_context: &QueryRuntimeContext,
    request_authenticators: &HashMap<String, Arc<dyn RequestAuthenticator>>,
    source_decorators: &mut [Box<dyn SourceDecorator>],
) -> Result<crate::runtime::registry::SourceRegistrationResult, CoreError> {
    let mut source_candidates = Vec::new();
    for source in sources {
        match compile_query_source(source, runtime_context, request_authenticators) {
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
    register_sources(ctx, source_candidates, source_decorators).await
}

impl QueryRuntimeAdapter {
    pub(crate) fn list_tables(
        &self,
        source_filter: Option<&str>,
        table_filter: Option<&str>,
    ) -> Vec<TableInfo> {
        self.tables
            .iter()
            .filter(|table| source_filter.is_none_or(|value| table.schema_name == value))
            .filter(|table| table_filter.is_none_or(|value| table.table_name == value))
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
        match self.execute_sql_once(&self.ctx, sql).await {
            Ok(execution) => Ok(execution),
            Err(SqlExecutionFailure::Collection(error)) => {
                // Resolver-row overflow is a dependent-join buffering limit, not
                // a SQL correctness boundary. Retry the original query with only
                // the dependent-join rewrite disabled; binding fanout and
                // per-binding fetch caps remain hard execution errors.
                let Some(cap_error) = resolver_rows_exceeded(&error) else {
                    return Err(datafusion_to_core(&error, &self.tables));
                };
                let Some(fallback_runtime) = &self.fallback_runtime else {
                    return Err(datafusion_to_core(&error, &self.tables));
                };

                tracing::warn!(
                    target = "coral_engine::dependent_join",
                    source = %cap_error.source_schema,
                    table = %cap_error.table,
                    observed = cap_error.observed,
                    cap = cap_error.cap,
                    disposition = "fallback",
                    "dependent join resolver row cap exceeded",
                );

                let fallback = fallback_runtime.build_without_dependent_join().await?;

                self.execute_sql_once(&fallback.ctx, sql)
                    .await
                    .map_err(|error| self.sql_execution_failure_to_core(error, sql))
            }
            Err(error) => Err(self.sql_execution_failure_to_core(error, sql)),
        }
    }

    async fn execute_sql_once(
        &self,
        ctx: &SessionContext,
        sql: &str,
    ) -> Result<QueryExecution, SqlExecutionFailure> {
        let df = ctx
            .sql_with_options(sql, read_only_sql_options())
            .await
            .map_err(SqlExecutionFailure::Planning)?;
        let arrow_schema = Arc::new(df.schema().as_arrow().clone());
        let batches = df
            .collect()
            .await
            .map_err(SqlExecutionFailure::Collection)?;
        self.observe_query_result(sql, arrow_schema.as_ref(), &batches)
            .map_err(SqlExecutionFailure::Observer)?;
        Ok(QueryExecution::new(arrow_schema, batches))
    }

    fn sql_execution_failure_to_core(&self, error: SqlExecutionFailure, sql: &str) -> CoreError {
        match error {
            SqlExecutionFailure::Planning(error) => {
                datafusion_to_core_with_sql(&error, &self.tables, Some(sql))
            }
            SqlExecutionFailure::Collection(error) => datafusion_to_core(&error, &self.tables),
            SqlExecutionFailure::Observer(error) => error,
        }
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
}

impl FallbackRuntimeConfig {
    async fn build_without_dependent_join(&self) -> Result<RegisteredRuntime, CoreError> {
        let mut source_decorators = Vec::new();
        build_registered_runtime(
            &self.sources,
            &self.runtime_context,
            &self.request_authenticators,
            source_decorators.as_mut_slice(),
            &self.dependent_join.without_rewrites(),
            &self.memory,
        )
        .await
    }
}

fn read_only_sql_options() -> SQLOptions {
    SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false)
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr as _;

    use datafusion::execution::memory_pool::MemoryConsumer;

    use super::{FallbackRuntimeConfig, build_session_context};
    use crate::{DependentJoinConfig, MemorySize, QueryMemoryConfig, QueryRuntimeContext};

    #[test]
    fn build_session_context_applies_memory_limit() {
        let ctx = build_session_context(
            &DependentJoinConfig::default(),
            &QueryMemoryConfig {
                limit: Some(MemorySize::from_str("1Ki").unwrap()),
            },
        )
        .expect("session context should build");
        let pool = ctx.runtime_env().memory_pool.clone();
        let reservation = MemoryConsumer::new("test").register(&pool);

        reservation
            .try_grow(512)
            .expect("reservation below limit should succeed");
        let error = reservation
            .try_grow(1024)
            .expect_err("reservation above limit should fail");

        assert!(
            error.to_string().contains("Resources exhausted"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn fallback_runtime_preserves_memory_limit() {
        let fallback = FallbackRuntimeConfig {
            sources: Vec::new(),
            runtime_context: QueryRuntimeContext::default(),
            dependent_join: DependentJoinConfig::default(),
            memory: QueryMemoryConfig {
                limit: Some(MemorySize::from_str("1Ki").unwrap()),
            },
            request_authenticators: HashMap::new(),
        };

        let runtime = fallback
            .build_without_dependent_join()
            .await
            .expect("fallback runtime should build");
        let pool = runtime.ctx.runtime_env().memory_pool.clone();
        let reservation = MemoryConsumer::new("fallback-test").register(&pool);

        reservation
            .try_grow(512)
            .expect("reservation below fallback limit should succeed");
        let error = reservation
            .try_grow(1024)
            .expect_err("reservation above fallback limit should fail");

        assert!(
            error.to_string().contains("Resources exhausted"),
            "unexpected error: {error}"
        );
    }
}
