//! Minimal analytical plan model owned by app-level query orchestration.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sources::SourceName;
use crate::workspaces::WorkspaceName;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct PlanId(String);

impl PlanId {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self(format!("plan_{}", Uuid::new_v4().simple()))
    }

    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for PlanId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AnalyticalPlan {
    pub(crate) plan_id: PlanId,
    pub(crate) workspace: WorkspaceName,
    pub(crate) query: PlannedQuery,
    pub(crate) workspace_sources_loaded: Vec<SourceName>,
    pub(crate) execution: PlanExecution,
}

impl AnalyticalPlan {
    #[must_use]
    pub(crate) fn from_sql(workspace_name: &WorkspaceName, sql: impl Into<String>) -> Self {
        Self {
            plan_id: PlanId::new(),
            workspace: workspace_name.clone(),
            query: PlannedQuery::Sql(sql.into()),
            workspace_sources_loaded: Vec::new(),
            execution: PlanExecution::default(),
        }
    }

    pub(crate) fn record_source_load(
        &mut self,
        workspace_sources_loaded: Vec<SourceName>,
        warnings: Vec<PlanWarning>,
    ) {
        self.workspace_sources_loaded = workspace_sources_loaded;
        self.execution.warnings.extend(warnings);
    }

    pub(crate) fn mark_execution_started(&mut self, started_at: DateTime<Utc>) {
        self.execution.status = PlanExecutionStatus::Running;
        self.execution.started_at = Some(started_at);
    }

    pub(crate) fn record_success(
        &mut self,
        row_count: u64,
        trace_id: Option<String>,
        completed_at: DateTime<Utc>,
    ) {
        self.execution.status = PlanExecutionStatus::Ok;
        self.execution.row_count = Some(row_count);
        self.execution.trace_id = trace_id;
        self.execution.completed_at = Some(completed_at);
    }

    pub(crate) fn record_error(
        &mut self,
        error_type: PlanError,
        trace_id: Option<String>,
        completed_at: DateTime<Utc>,
    ) {
        self.execution.status = PlanExecutionStatus::Error;
        self.execution.error_type = Some(error_type);
        self.execution.trace_id = trace_id;
        self.execution.completed_at = Some(completed_at);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub(crate) enum PlannedQuery {
    Sql(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanExecution {
    pub(crate) status: PlanExecutionStatus,
    pub(crate) trace_id: Option<String>,
    pub(crate) row_count: Option<u64>,
    pub(crate) error_type: Option<PlanError>,
    pub(crate) warnings: Vec<PlanWarning>,
    pub(crate) started_at: Option<DateTime<Utc>>,
    pub(crate) completed_at: Option<DateTime<Utc>>,
}

impl Default for PlanExecution {
    fn default() -> Self {
        Self {
            status: PlanExecutionStatus::NotStarted,
            trace_id: None,
            row_count: None,
            error_type: None,
            warnings: Vec::new(),
            started_at: None,
            completed_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanExecutionStatus {
    NotStarted,
    Running,
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum PlanError {
    Code(PlanErrorCode),
    QueryFailure(String),
}

impl PlanError {
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Code(error) => error.as_str(),
            Self::QueryFailure(reason) => reason.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanErrorCode {
    InvalidArgument,
    NotFound,
    FailedPrecondition,
    Unavailable,
    Unimplemented,
    Internal,
}

impl PlanErrorCode {
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::NotFound => "NOT_FOUND",
            Self::FailedPrecondition => "FAILED_PRECONDITION",
            Self::Unavailable => "UNAVAILABLE",
            Self::Unimplemented => "UNIMPLEMENTED",
            Self::Internal => "INTERNAL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PlanWarning {
    SourceSkipped { source: SourceName, message: String },
}

impl PlanWarning {
    #[must_use]
    pub(crate) fn source_skipped(source: SourceName, message: impl Into<String>) -> Self {
        Self::SourceSkipped {
            source,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use serde_json::json;

    use super::{
        AnalyticalPlan, PlanError, PlanErrorCode, PlanExecutionStatus, PlanWarning, PlannedQuery,
    };
    use crate::sources::SourceName;
    use crate::workspaces::WorkspaceName;

    #[test]
    fn sql_plan_stores_only_app_owned_facts() {
        let workspace = WorkspaceName::parse("default").expect("workspace");
        let plan = AnalyticalPlan::from_sql(&workspace, "SELECT * FROM github.issues LIMIT 5");

        assert!(plan.plan_id.as_str().starts_with("plan_"));
        assert_eq!(plan.workspace, workspace);
        assert!(matches!(
            &plan.query,
            PlannedQuery::Sql(sql) if sql == "SELECT * FROM github.issues LIMIT 5"
        ));
        assert!(plan.workspace_sources_loaded.is_empty());
        assert_eq!(plan.execution.status, PlanExecutionStatus::NotStarted);
    }

    #[test]
    fn zero_row_success_preserves_zero_as_known_count() {
        let workspace = WorkspaceName::parse("default").expect("workspace");
        let mut plan = AnalyticalPlan::from_sql(&workspace, "SELECT * FROM local.messages");
        let completed_at = chrono::Utc
            .with_ymd_and_hms(2026, 5, 14, 12, 0, 0)
            .single()
            .expect("timestamp");

        plan.record_success(0, Some("trace-1".to_string()), completed_at);

        assert_eq!(plan.execution.status, PlanExecutionStatus::Ok);
        assert_eq!(plan.execution.row_count, Some(0));
        assert_eq!(plan.execution.trace_id.as_deref(), Some("trace-1"));
    }

    #[test]
    fn error_evidence_records_error_type_without_row_count() {
        let workspace = WorkspaceName::parse("default").expect("workspace");
        let mut plan = AnalyticalPlan::from_sql(&workspace, "SELECT * FROM missing.table");
        let completed_at = chrono::Utc
            .with_ymd_and_hms(2026, 5, 14, 12, 0, 0)
            .single()
            .expect("timestamp");

        plan.record_error(
            PlanError::Code(PlanErrorCode::NotFound),
            Some("trace-error".to_string()),
            completed_at,
        );

        assert_eq!(plan.execution.status, PlanExecutionStatus::Error);
        assert_eq!(plan.execution.row_count, None);
        assert!(matches!(
            plan.execution.error_type,
            Some(PlanError::Code(PlanErrorCode::NotFound))
        ));
        assert_eq!(plan.execution.trace_id.as_deref(), Some("trace-error"));
    }

    #[test]
    fn source_load_warnings_serialize_as_plan_evidence() {
        let workspace = WorkspaceName::parse("default").expect("workspace");
        let mut plan = AnalyticalPlan::from_sql(&workspace, "SELECT * FROM secured.messages");
        plan.record_source_load(
            vec![SourceName::parse("github").expect("source name")],
            vec![PlanWarning::source_skipped(
                SourceName::parse("secured").expect("source name"),
                "source 'secured' is missing secret 'API_TOKEN'",
            )],
        );

        assert_eq!(
            plan.workspace_sources_loaded,
            vec![SourceName::parse("github").expect("source name")]
        );
        let warning = plan.execution.warnings.first().expect("warning");
        let PlanWarning::SourceSkipped { source, message } = warning;
        assert_eq!(source.as_str(), "secured");
        assert!(message.contains("API_TOKEN"));

        let value = serde_json::to_value(&plan).expect("serialize plan");
        assert_eq!(value.pointer("/query/kind"), Some(&json!("sql")));
        assert_eq!(
            value.pointer("/execution/warnings/0/kind"),
            Some(&json!("source_skipped"))
        );
    }
}
