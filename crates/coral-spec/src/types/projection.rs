//! SQL-facing projections over source model operations.

use super::source::OperationId;

/// Scalar types supported by source-model table projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionScalarType {
    /// UTF-8 string values.
    String,
    /// Signed 64-bit integer values.
    Integer,
}

/// One response field exposed by a table projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionColumn {
    name: String,
    ty: ProjectionScalarType,
    nullable: bool,
}

impl ProjectionColumn {
    /// Creates a projected response column.
    pub fn new(name: impl Into<String>, ty: ProjectionScalarType, nullable: bool) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable,
        }
    }

    /// Returns the SQL-visible column name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the neutral scalar type for this column.
    pub fn ty(&self) -> &ProjectionScalarType {
        &self.ty
    }

    /// Returns whether this column may contain null values.
    pub fn nullable(&self) -> bool {
        self.nullable
    }
}

/// One operation input exposed as a table filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionFilter {
    name: String,
    ty: ProjectionScalarType,
    required: bool,
}

impl ProjectionFilter {
    /// Creates a required filter.
    pub fn required(name: impl Into<String>, ty: ProjectionScalarType) -> Self {
        Self {
            name: name.into(),
            ty,
            required: true,
        }
    }

    /// Creates an optional filter.
    pub fn optional(name: impl Into<String>, ty: ProjectionScalarType) -> Self {
        Self {
            name: name.into(),
            ty,
            required: false,
        }
    }

    /// Returns the SQL-visible filter name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the neutral scalar type accepted by this filter.
    pub fn ty(&self) -> &ProjectionScalarType {
        &self.ty
    }

    /// Returns whether this filter must be provided before scanning.
    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// SQL-facing projection over a logical source operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableProjection {
    name: String,
    operation: OperationId,
    columns: Vec<ProjectionColumn>,
    filters: Vec<ProjectionFilter>,
}

impl TableProjection {
    /// Creates a table projection.
    pub fn new(
        name: impl Into<String>,
        operation: OperationId,
        columns: Vec<ProjectionColumn>,
        filters: Vec<ProjectionFilter>,
    ) -> Self {
        Self {
            name: name.into(),
            operation,
            columns,
            filters,
        }
    }

    /// Returns the fully qualified SQL table name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the logical source operation backing this projection.
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    /// Returns response fields exposed by this projection.
    pub fn columns(&self) -> &[ProjectionColumn] {
        &self.columns
    }

    /// Returns operation inputs exposed as filters by this projection.
    pub fn filters(&self) -> &[ProjectionFilter] {
        &self.filters
    }

    /// Returns names of filters required before a scan can execute.
    pub fn required_filter_names(&self) -> Vec<&str> {
        self.filters
            .iter()
            .filter(|filter| filter.required)
            .map(|filter| filter.name.as_str())
            .collect()
    }
}

/// Returns the spike projection for `github.issues`.
pub fn github_issues_projection() -> TableProjection {
    TableProjection::new(
        "github.issues",
        OperationId::new("github.issue.list"),
        vec![
            ProjectionColumn::new("number", ProjectionScalarType::Integer, false),
            ProjectionColumn::new("title", ProjectionScalarType::String, false),
            ProjectionColumn::new("state", ProjectionScalarType::String, false),
            ProjectionColumn::new("created_at", ProjectionScalarType::String, false),
            ProjectionColumn::new("user__login", ProjectionScalarType::String, true),
        ],
        vec![
            ProjectionFilter::required("owner", ProjectionScalarType::String),
            ProjectionFilter::required("repo", ProjectionScalarType::String),
            ProjectionFilter::optional("state", ProjectionScalarType::String),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_issues_references_issue_list_operation() {
        let projection = github_issues_projection();

        assert_eq!(projection.name(), "github.issues");
        assert_eq!(projection.operation().as_str(), "github.issue.list");
    }

    #[test]
    fn github_issues_exposes_output_columns() {
        let projection = github_issues_projection();
        let column_names = projection
            .columns()
            .iter()
            .map(ProjectionColumn::name)
            .collect::<Vec<_>>();

        assert_eq!(
            column_names,
            vec!["number", "title", "state", "created_at", "user__login"]
        );
    }

    #[test]
    fn github_issues_reports_filter_requirements() {
        let projection = github_issues_projection();
        let required = projection.required_filter_names();
        let state_filter = projection
            .filters()
            .iter()
            .find(|filter| filter.name() == "state")
            .expect("state filter");

        assert_eq!(required, vec!["owner", "repo"]);
        assert!(!state_filter.is_required());
    }
}
