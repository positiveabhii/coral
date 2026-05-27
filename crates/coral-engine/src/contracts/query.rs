//! Typed query inputs and results.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;
use coral_spec::ValidatedSourceManifest;
use opentelemetry::Context as OtelContext;

use super::ColumnInfo;
use crate::EngineExtensions;

/// One managed source selected into the current query runtime.
#[derive(Debug, Clone)]
pub struct QuerySource {
    source_spec: ValidatedSourceManifest,
    variables: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
}

impl QuerySource {
    #[must_use]
    /// Builds one app-to-query source selection from installed metadata and a
    /// validated declarative source spec.
    pub fn new(
        source_spec: ValidatedSourceManifest,
        variables: BTreeMap<String, String>,
        secrets: BTreeMap<String, String>,
    ) -> Self {
        Self {
            source_spec,
            variables,
            secrets,
        }
    }

    #[must_use]
    /// Returns the canonical source name. This is also the visible SQL schema name.
    pub fn source_name(&self) -> &str {
        self.source_spec.schema_name()
    }

    #[must_use]
    /// Returns the installed manifest version for this source.
    pub fn version(&self) -> &str {
        self.source_spec.source_version()
    }

    #[must_use]
    /// Returns the validated declarative source spec for this source.
    pub fn source_spec(&self) -> &ValidatedSourceManifest {
        &self.source_spec
    }

    #[must_use]
    /// Returns configured non-secret source variables.
    pub fn variables(&self) -> &BTreeMap<String, String> {
        &self.variables
    }

    #[must_use]
    /// Returns resolved source secrets required by the manifest.
    pub fn secrets(&self) -> &BTreeMap<String, String> {
        &self.secrets
    }
}

/// One source-spec validation query executed during source validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTestResult {
    sql: String,
    result: Result<QueryTestSuccess, QueryTestFailure>,
}

/// Success metadata for one validation query execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTestSuccess {
    row_count: u64,
}

impl QueryTestSuccess {
    #[must_use]
    /// Returns the row count captured for the successful query.
    pub fn row_count(&self) -> u64 {
        self.row_count
    }
}

/// Failure details for one validation query execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTestFailure {
    error_message: String,
}

impl QueryTestFailure {
    #[must_use]
    /// Returns the error message captured for the failed query.
    pub fn error_message(&self) -> &str {
        &self.error_message
    }
}

impl QueryTestResult {
    #[must_use]
    /// Builds one successful query-test result entry.
    pub fn success(sql: impl Into<String>, row_count: u64) -> Self {
        Self {
            sql: sql.into(),
            result: Ok(QueryTestSuccess { row_count }),
        }
    }

    #[must_use]
    /// Builds one failed query-test result entry.
    pub fn failure(sql: impl Into<String>, error_message: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            result: Err(QueryTestFailure {
                error_message: error_message.into(),
            }),
        }
    }

    #[must_use]
    /// Returns the SQL text that was executed.
    pub fn sql(&self) -> &str {
        &self.sql
    }

    #[must_use]
    /// Returns whether the query executed successfully.
    pub fn passed(&self) -> bool {
        self.result.is_ok()
    }

    #[must_use]
    /// Returns the captured row count for successful queries.
    pub fn row_count(&self) -> Option<u64> {
        self.result.as_ref().ok().map(QueryTestSuccess::row_count)
    }

    #[must_use]
    /// Returns the error message for failed queries, when present.
    pub fn error_message(&self) -> Option<&str> {
        self.result
            .as_ref()
            .err()
            .map(QueryTestFailure::error_message)
    }

    /// Returns the execution result metadata for this query test.
    pub fn result(&self) -> &Result<QueryTestSuccess, QueryTestFailure> {
        &self.result
    }
}

/// Structured report for validating one source and its optional test queries.
#[derive(Debug, Clone)]
pub struct SourceValidationReport {
    /// Tables exposed by the validated source.
    pub tables: Vec<super::TableInfo>,
    /// One result per declared validation query, in manifest order.
    pub query_tests: Vec<QueryTestResult>,
}

impl SourceValidationReport {
    #[must_use]
    /// Builds one structured source-validation report.
    pub fn new(tables: Vec<super::TableInfo>, query_tests: Vec<QueryTestResult>) -> Self {
        Self {
            tables,
            query_tests,
        }
    }
}

/// App-owned non-secret runtime inputs needed while compiling sources.
#[derive(Clone, Default)]
pub struct QueryRuntimeContext {
    /// Current user's home directory for local path resolution.
    pub home_dir: Option<PathBuf>,
    /// Active query trace context, when the app layer is executing under one.
    pub trace_context: Option<OtelContext>,
}

impl fmt::Debug for QueryRuntimeContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryRuntimeContext")
            .field("home_dir", &self.home_dir)
            .field("trace_context", &self.trace_context.is_some())
            .finish()
    }
}

/// Owned runtime-build inputs needed while compiling and registering sources.
#[derive(Default)]
pub struct QueryRuntimeConfig {
    /// Non-secret runtime inputs owned by the application layer.
    pub context: QueryRuntimeContext,
    /// Optional engine extensions for this runtime build.
    pub extensions: EngineExtensions,
    /// Engine-wide query memory policy.
    pub memory: QueryMemoryConfig,
    /// Runtime policy for dependent predicate pushdown.
    pub dependent_join: DependentJoinConfig,
}

impl QueryRuntimeConfig {
    /// Builds one runtime config from app-owned context and extension state.
    #[must_use]
    pub fn new(context: QueryRuntimeContext, extensions: EngineExtensions) -> Self {
        Self {
            context,
            extensions,
            memory: QueryMemoryConfig::default(),
            dependent_join: DependentJoinConfig::default(),
        }
    }
}

/// Engine-wide query memory policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryMemoryConfig {
    /// Optional total query-engine memory limit.
    pub limit: Option<MemorySize>,
}

/// Human-readable memory size stored internally as bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MemorySize {
    bytes: usize,
}

impl MemorySize {
    /// Builds a memory size from a positive byte count.
    ///
    /// # Errors
    ///
    /// Returns an error when `bytes` is zero.
    pub fn from_bytes(bytes: usize) -> Result<Self, MemorySizeParseError> {
        if bytes == 0 {
            return Err(MemorySizeParseError::new(
                "memory limit must be greater than 0",
            ));
        }
        Ok(Self { bytes })
    }

    /// Returns this size in bytes.
    #[must_use]
    pub fn as_bytes(self) -> usize {
        self.bytes
    }
}

/// Error returned when parsing a human-readable memory size fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySizeParseError {
    detail: String,
}

impl MemorySizeParseError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for MemorySizeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for MemorySizeParseError {}

impl FromStr for MemorySize {
    type Err = MemorySizeParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let value = raw.trim();
        if value.is_empty() {
            return Err(MemorySizeParseError::new("memory limit must not be empty"));
        }

        let (number, multiplier) = parse_memory_unit(value)?;
        if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(MemorySizeParseError::new(
                "memory limit must be an integer followed by Ki, Mi, Gi, or Ti",
            ));
        }

        let amount = number
            .parse::<u128>()
            .map_err(|_error| MemorySizeParseError::new("memory limit is too large"))?;
        if amount == 0 {
            return Err(MemorySizeParseError::new(
                "memory limit must be greater than 0",
            ));
        }
        let bytes = amount
            .checked_mul(multiplier)
            .ok_or_else(|| MemorySizeParseError::new("memory limit is too large"))?;
        let bytes = usize::try_from(bytes)
            .map_err(|_error| MemorySizeParseError::new("memory limit is too large"))?;
        Self::from_bytes(bytes)
    }
}

fn parse_memory_unit(value: &str) -> Result<(&str, u128), MemorySizeParseError> {
    for (suffix, multiplier) in [
        ("Ki", 1024_u128),
        ("Mi", 1024_u128.pow(2)),
        ("Gi", 1024_u128.pow(3)),
        ("Ti", 1024_u128.pow(4)),
    ] {
        if let Some(number) = value.strip_suffix(suffix) {
            return Ok((number, multiplier));
        }
    }
    Err(MemorySizeParseError::new(
        "memory limit must use binary unit Ki, Mi, Gi, or Ti",
    ))
}

/// Runtime policy for dependent predicate pushdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependentJoinConfig {
    /// Default enablement for dependent join rewrites.
    pub enabled: bool,
    /// Maximum distinct join-key combinations to push into upstream APIs.
    pub max_bindings: usize,
    /// Maximum rows read from the key-supplying side before falling back.
    pub max_resolver_rows: usize,
    /// Maximum rows accepted for one join-key combination across the full upstream fetch.
    pub max_rows_per_binding: usize,
    /// Maximum key-supplying rows allowed for one join-key combination.
    pub max_resolver_rows_per_binding: usize,
    /// Maximum concurrent upstream requests issued by one dependent join.
    pub max_concurrency: usize,
    /// Source-specific overrides keyed by source name.
    pub per_source: BTreeMap<String, DependentJoinSourceConfig>,
}

/// Source-specific dependent predicate pushdown policy overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependentJoinSourceConfig {
    /// Overrides dependent join rewrite enablement for this source.
    pub enabled: Option<bool>,
    /// Overrides maximum distinct join-key combinations for this source.
    pub max_bindings: Option<usize>,
    /// Overrides maximum resolver-side rows for this source.
    pub max_resolver_rows: Option<usize>,
    /// Overrides maximum rows accepted from one upstream request.
    pub max_rows_per_binding: Option<usize>,
    /// Overrides maximum resolver rows allowed for one join-key combination.
    pub max_resolver_rows_per_binding: Option<usize>,
    /// Overrides concurrent upstream requests issued by one dependent join.
    pub max_concurrency: Option<usize>,
}

/// Fully resolved dependent predicate pushdown policy for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveDependentJoinConfig {
    /// Enables dependent join rewrites for this source.
    pub enabled: bool,
    /// Maximum distinct join-key combinations to push into upstream APIs.
    pub max_bindings: usize,
    /// Maximum rows read from the key-supplying side before falling back.
    pub max_resolver_rows: usize,
    /// Maximum rows accepted from one upstream request.
    pub max_rows_per_binding: usize,
    /// Maximum key-supplying rows allowed for one join-key combination.
    pub max_resolver_rows_per_binding: usize,
    /// Maximum concurrent upstream requests issued by one dependent join.
    pub max_concurrency: usize,
}

impl Default for DependentJoinConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_bindings: 500,
            max_resolver_rows: 10_000,
            max_rows_per_binding: 1_000,
            max_resolver_rows_per_binding: 1_000,
            max_concurrency: 8,
            per_source: BTreeMap::new(),
        }
    }
}

impl DependentJoinConfig {
    /// Returns a copy with all dependent join rewrites disabled.
    #[must_use]
    pub fn without_rewrites(&self) -> Self {
        Self {
            enabled: false,
            per_source: BTreeMap::new(),
            ..self.clone()
        }
    }

    /// Returns whether the optimizer rule should be registered.
    #[must_use]
    pub fn optimizer_enabled(&self) -> bool {
        self.enabled
            || self
                .per_source
                .values()
                .any(|source| source.enabled == Some(true))
    }

    /// Resolves the effective dependent join policy for one source.
    #[must_use]
    pub fn for_source(&self, source_name: &str) -> EffectiveDependentJoinConfig {
        let source = self.per_source.get(source_name);
        let max_concurrency = source
            .and_then(|override_config| override_config.max_concurrency)
            .unwrap_or(self.max_concurrency)
            .max(1);
        EffectiveDependentJoinConfig {
            enabled: source
                .and_then(|override_config| override_config.enabled)
                .unwrap_or(self.enabled),
            max_bindings: source
                .and_then(|override_config| override_config.max_bindings)
                .unwrap_or(self.max_bindings),
            max_resolver_rows: source
                .and_then(|override_config| override_config.max_resolver_rows)
                .unwrap_or(self.max_resolver_rows),
            max_rows_per_binding: source
                .and_then(|override_config| override_config.max_rows_per_binding)
                .unwrap_or(self.max_rows_per_binding),
            max_resolver_rows_per_binding: source
                .and_then(|override_config| override_config.max_resolver_rows_per_binding)
                .unwrap_or(self.max_resolver_rows_per_binding),
            max_concurrency,
        }
    }
}

/// The fully materialized result of executing one `SQL` statement.
#[derive(Debug, Clone)]
pub struct QueryExecution {
    schema: Vec<ColumnInfo>,
    arrow_schema: Arc<Schema>,
    batches: Vec<RecordBatch>,
    row_count: usize,
}

impl QueryExecution {
    #[must_use]
    /// Builds a validated fully materialized query result.
    pub fn new(arrow_schema: Arc<Schema>, batches: Vec<RecordBatch>) -> Self {
        let schema = arrow_schema
            .fields()
            .iter()
            .enumerate()
            .map(|(position, field)| ColumnInfo {
                name: field.name().clone(),
                data_type: field.data_type().to_string(),
                nullable: field.is_nullable(),
                is_virtual: false,
                is_required_filter: false,
                description: String::new(),
                ordinal_position: u32::try_from(position).unwrap_or(u32::MAX),
            })
            .collect();
        let row_count = batches.iter().map(RecordBatch::num_rows).sum();
        Self {
            schema,
            arrow_schema,
            batches,
            row_count,
        }
    }

    #[must_use]
    /// Returns the logical result-set schema.
    pub fn schema(&self) -> &[ColumnInfo] {
        &self.schema
    }

    #[must_use]
    /// Returns the Arrow schema preserved even for empty result sets.
    pub fn arrow_schema(&self) -> &Arc<Schema> {
        &self.arrow_schema
    }

    #[must_use]
    /// Returns the materialized Arrow record batches.
    pub fn batches(&self) -> &[RecordBatch] {
        &self.batches
    }

    #[must_use]
    /// Returns the total number of rows across all batches.
    pub fn row_count(&self) -> usize {
        self.row_count
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::MemorySize;

    #[test]
    fn memory_size_parses_binary_units() {
        assert_eq!(MemorySize::from_str("1Ki").unwrap().as_bytes(), 1024);
        assert_eq!(
            MemorySize::from_str("2Mi").unwrap().as_bytes(),
            2 * 1024 * 1024
        );
        assert_eq!(
            MemorySize::from_str("3Gi").unwrap().as_bytes(),
            3 * 1024 * 1024 * 1024
        );
        assert_eq!(
            MemorySize::from_str("1Ti").unwrap().as_bytes(),
            1024_usize.pow(4)
        );
    }

    #[test]
    fn memory_size_rejects_invalid_values() {
        for raw in ["", "0Mi", "2GiB", "2.5Gi", "2gi", "2G", "Gi"] {
            assert!(
                MemorySize::from_str(raw).is_err(),
                "{raw:?} should be rejected"
            );
        }
    }
}
