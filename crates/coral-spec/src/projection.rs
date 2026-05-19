#![allow(
    missing_docs,
    reason = "This module defines field-heavy DSL v4 projection types."
)]

//! Explicit SQL projection model for DSL v4 source-model manifests.
//!
//! Projections are author-facing SQL exposure metadata. They are intentionally
//! separate from [`crate::source_model`], which is importer-produced provider IR.

use serde::{Deserialize, Serialize};

use crate::ColumnSpec;

/// One explicit SQL projection declared by a DSL v4 manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceModelProjection {
    pub name: String,
    pub kind: ProjectionKind,
    #[serde(flatten)]
    pub operation: SourceModelOperationRef,
    #[serde(default)]
    pub columns: Vec<ColumnSpec>,
}

impl SourceModelProjection {
    pub fn reference(&self) -> SourceModelProjectionRef {
        SourceModelProjectionRef {
            name: self.name.clone(),
            kind: self.kind,
            operation: self.operation.clone(),
        }
    }
}

/// Stable surface-scoped operation reference used by DSL v4 projections.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceModelOperationRef {
    pub surface: String,
    pub operation: String,
}

/// Projection reference shape used to validate authored projections against IR.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceModelProjectionRef {
    pub name: String,
    pub kind: ProjectionKind,
    #[serde(flatten)]
    pub operation: SourceModelOperationRef,
}

/// SQL affordance type exposed by a DSL v4 projection.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKind {
    Table,
    Function,
}
