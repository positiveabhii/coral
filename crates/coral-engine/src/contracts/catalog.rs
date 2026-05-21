//! Typed query-visible catalog metadata.

/// SQL operation supported by a query-visible relation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RelationOperation {
    /// Relation supports `SELECT`.
    Read,
    /// Relation supports `INSERT`.
    Insert,
    /// Relation supports `UPDATE`.
    Update,
    /// Relation supports `DELETE`.
    Delete,
    /// Relation supports `TRUNCATE`.
    Truncate,
}

impl RelationOperation {
    /// Returns the stable catalog/API spelling for this operation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Truncate => "truncate",
        }
    }
}

/// Write behavior for one queryable column.
#[derive(Debug, Clone, Default)]
pub struct ColumnWriteBehavior {
    /// Whether the column is used as a direct write target key.
    pub is_key: bool,
    /// Whether the column can be assigned or inserted through a write operation.
    pub is_writable: bool,
    /// Whether inserts must provide this writable column.
    pub required_on_insert: bool,
}

/// Describes one queryable column.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// Column name.
    pub name: String,
    /// Data type rendered in `Arrow`/`DataFusion` string form.
    pub data_type: String,
    /// Whether the column can contain null values.
    pub nullable: bool,
    /// Whether the column is provider-derived metadata, such as a filter or computed column.
    pub is_virtual: bool,
    /// Whether the column must be constrained before querying the table.
    pub is_required_filter: bool,
    /// Write behavior for this column.
    pub write_behavior: ColumnWriteBehavior,
    /// User-facing column description.
    pub description: String,
    /// Zero-based position of the column within the table.
    pub ordinal_position: u32,
}

impl ColumnInfo {
    /// Returns whether this column is a direct write target key.
    #[must_use]
    pub fn is_key(&self) -> bool {
        self.write_behavior.is_key
    }

    /// Returns whether this column is writable.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.write_behavior.is_writable
    }

    /// Returns whether this column is required on insert.
    #[must_use]
    pub fn write_required_on_insert(&self) -> bool {
        self.write_behavior.required_on_insert
    }
}

/// Operation and effect metadata for one query-visible relation.
#[derive(Debug, Clone)]
pub struct RelationCapabilities {
    /// Supported SQL operations.
    pub operations: Vec<RelationOperation>,
    /// Direct-key columns that write operations derive from request templates.
    pub derived_key_columns: Vec<String>,
    /// Highest effect class exposed by this relation.
    pub effect: String,
}

impl RelationCapabilities {
    /// Returns whether the relation supports `operation`.
    #[must_use]
    pub fn supports(&self, operation: RelationOperation) -> bool {
        self.operations.contains(&operation)
    }
}

/// Describes one queryable relation.
#[derive(Debug, Clone)]
pub struct RelationInfo {
    /// `SQL` schema name.
    pub schema_name: String,
    /// Relation name within the schema.
    pub relation_name: String,
    /// User-facing relation description.
    pub description: String,
    /// User-facing query guidance.
    pub guide: String,
    /// Exposed columns for the relation.
    pub columns: Vec<ColumnInfo>,
    /// Required filter names for the relation.
    pub required_filters: Vec<String>,
    /// Relation operation and effect metadata.
    pub capabilities: RelationCapabilities,
}

impl RelationInfo {
    /// Returns whether the relation supports `operation`.
    #[must_use]
    pub fn supports(&self, operation: RelationOperation) -> bool {
        self.capabilities.supports(operation)
    }
}
