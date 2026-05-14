//! SQL-facing projections over source model operations.

use super::source::{
    EntityField, OperationId, OperationInput, ScalarType, SourceModel, TypeRef, github_source_model,
};

/// Types supported by source-model table projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionScalarType {
    /// UTF-8 string values.
    String,
    /// Signed 64-bit integer values.
    Integer,
    /// Boolean values.
    Boolean,
    /// JSON value serialized as UTF-8 text for the current SQL surface.
    Json,
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

/// Number of rows an operation output can produce for one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionCardinality {
    /// The operation returns at most one projected row.
    One,
    /// The operation returns zero or more projected rows.
    Many,
}

/// SQL-facing projection over a logical source operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableProjection {
    name: String,
    operation: OperationId,
    cardinality: ProjectionCardinality,
    columns: Vec<ProjectionColumn>,
    filters: Vec<ProjectionFilter>,
}

impl TableProjection {
    fn new(
        name: impl Into<String>,
        operation: OperationId,
        cardinality: ProjectionCardinality,
        columns: Vec<ProjectionColumn>,
        filters: Vec<ProjectionFilter>,
    ) -> Self {
        Self {
            name: name.into(),
            operation,
            cardinality,
            columns,
            filters,
        }
    }

    /// Creates a table projection by resolving an operation's source entity.
    pub fn from_source_operation(
        model: &SourceModel,
        name: impl Into<String>,
        operation: OperationId,
    ) -> Option<Self> {
        let operation_model = model.operation(&operation)?;
        let (entity_id, cardinality) = output_entity_and_cardinality(operation_model.output())?;
        let entity = model.entity(entity_id)?;
        let columns = entity
            .fields()
            .iter()
            .map(projection_column_for_entity_field)
            .collect::<Vec<_>>();
        let filters = operation_model
            .inputs()
            .iter()
            .map(projection_filter_for_operation_input)
            .collect::<Vec<_>>();

        Some(Self::new(name, operation, cardinality, columns, filters))
    }

    /// Returns the fully qualified SQL table name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the logical source operation backing this projection.
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    /// Returns how many rows this operation can emit per invocation.
    pub fn cardinality(&self) -> ProjectionCardinality {
        self.cardinality
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
///
/// # Panics
///
/// Panics if the built-in GitHub issue list operation no longer resolves.
pub fn github_issues_projection() -> TableProjection {
    TableProjection::from_source_operation(
        &github_source_model(),
        "github.issues",
        OperationId::new("github.issue.list"),
    )
    .expect("github issue list operation should resolve")
}

/// Returns the spike projection for `github.issue_search`.
///
/// # Panics
///
/// Panics if the built-in GitHub issue search operation no longer resolves.
pub fn github_issue_search_projection() -> TableProjection {
    TableProjection::from_source_operation(
        &github_source_model(),
        "github.issue_search",
        OperationId::new("github.issue.search"),
    )
    .expect("github issue search operation should resolve")
}

/// Returns the spike projection for `github.issue`.
///
/// # Panics
///
/// Panics if the built-in GitHub issue get operation no longer resolves.
pub fn github_issue_projection() -> TableProjection {
    TableProjection::from_source_operation(
        &github_source_model(),
        "github.issue",
        OperationId::new("github.issue.get"),
    )
    .expect("github issue get operation should resolve")
}

fn output_entity_and_cardinality(
    output: &TypeRef,
) -> Option<(&super::source::EntityId, ProjectionCardinality)> {
    match output {
        TypeRef::Entity(entity) => Some((entity, ProjectionCardinality::One)),
        TypeRef::List(inner) => match inner.as_ref() {
            TypeRef::Entity(entity) => Some((entity, ProjectionCardinality::Many)),
            _ => None,
        },
        TypeRef::Scalar(_) => None,
    }
}

fn projection_column_for_entity_field(field: &EntityField) -> ProjectionColumn {
    ProjectionColumn::new(
        field.name(),
        projection_type_for_type_ref(field.ty()),
        field.nullable(),
    )
}

fn projection_filter_for_operation_input(input: &OperationInput) -> ProjectionFilter {
    let ty = projection_type_for_type_ref(input.ty());
    if input.is_required() {
        ProjectionFilter::required(input.name(), ty)
    } else {
        ProjectionFilter::optional(input.name(), ty)
    }
}

fn projection_type_for_type_ref(ty: &TypeRef) -> ProjectionScalarType {
    match ty {
        TypeRef::Scalar(ScalarType::String) => ProjectionScalarType::String,
        TypeRef::Scalar(ScalarType::Integer) => ProjectionScalarType::Integer,
        TypeRef::Scalar(ScalarType::Boolean) => ProjectionScalarType::Boolean,
        TypeRef::Entity(_) | TypeRef::List(_) => ProjectionScalarType::Json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_issues_references_issue_list_operation() {
        let projection = github_issues_projection();

        assert_eq!(projection.name(), "github.issues");
        assert_eq!(projection.operation().as_str(), "github.issue.list");
        assert_eq!(projection.cardinality(), ProjectionCardinality::Many);
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
            vec!["number", "title", "state", "created_at", "html_url", "user"]
        );
    }

    #[test]
    fn github_issues_projects_nested_user_as_json_column() {
        let projection = github_issues_projection();
        let user = projection
            .columns()
            .iter()
            .find(|column| column.name() == "user")
            .expect("user column");

        assert_eq!(user.ty(), &ProjectionScalarType::Json);
        assert!(user.nullable());
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

    #[test]
    fn github_issue_search_references_issue_search_operation() {
        let projection = github_issue_search_projection();

        assert_eq!(projection.name(), "github.issue_search");
        assert_eq!(projection.operation().as_str(), "github.issue.search");
        assert_eq!(projection.cardinality(), ProjectionCardinality::Many);
    }

    #[test]
    fn github_issue_search_reports_filter_requirements() {
        let projection = github_issue_search_projection();
        let required = projection.required_filter_names();
        let optional_filters = projection
            .filters()
            .iter()
            .filter(|filter| !filter.is_required())
            .map(ProjectionFilter::name)
            .collect::<Vec<_>>();

        assert_eq!(required, vec!["q"]);
        assert_eq!(optional_filters, vec!["sort", "order"]);
    }

    #[test]
    fn github_issue_projection_references_singleton_get_operation() {
        let projection = github_issue_projection();
        let required = projection.required_filter_names();

        assert_eq!(projection.name(), "github.issue");
        assert_eq!(projection.operation().as_str(), "github.issue.get");
        assert_eq!(projection.cardinality(), ProjectionCardinality::One);
        assert_eq!(required, vec!["owner", "repo", "issue_number"]);
    }
}
