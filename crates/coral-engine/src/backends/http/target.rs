//! Engine-local fetch shape shared by HTTP relations and table functions.

use std::sync::Arc;

use coral_spec::backends::http::{
    HttpRelationSpec, HttpRelationWriteOperation, HttpRelationWriteOperationSpec,
};
use coral_spec::{ColumnSpec, PaginationSpec, RequestSpec, ResponseSpec, SourceTableFunctionSpec};

/// SQL-facing HTTP target kind used for tracing and execution metadata.
#[derive(Debug, Clone, Copy)]
pub(crate) enum HttpSqlTargetKind {
    Relation,
    Function,
}

impl HttpSqlTargetKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Relation => "relation",
            Self::Function => "function",
        }
    }
}

/// The HTTP request/response description needed to fetch rows.
///
/// Relations and table functions are distinct manifest concepts, but once their
/// SQL-facing inputs have been resolved they share the same HTTP execution path.
#[derive(Clone)]
pub(crate) struct HttpFetchTarget {
    kind: HttpSqlTargetKind,
    name: Arc<str>,
    columns: Arc<[ColumnSpec]>,
    fetch_limit_default: Option<usize>,
    resolved_request: RequestSpec,
    response: Arc<ResponseSpec>,
    pagination: Arc<PaginationSpec>,
}

/// The HTTP request/response description needed to execute one write operation.
#[derive(Clone)]
pub(crate) struct HttpWriteTarget {
    name: Arc<str>,
    operation: HttpRelationWriteOperation,
    resolved_request: RequestSpec,
    response: Arc<ResponseSpec>,
}

impl std::fmt::Debug for HttpWriteTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpWriteTarget")
            .field("name", &self.name)
            .field("operation", &self.operation.as_str())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for HttpFetchTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpFetchTarget")
            .field("name", &self.name)
            .field("columns", &self.columns)
            .field("fetch_limit_default", &self.fetch_limit_default)
            .finish_non_exhaustive()
    }
}

impl HttpWriteTarget {
    pub(crate) fn from_relation_write(
        relation: &HttpRelationSpec,
        operation: &HttpRelationWriteOperationSpec,
    ) -> Self {
        Self {
            name: Arc::from(relation.name()),
            operation: operation.operation,
            resolved_request: operation.request.clone(),
            response: Arc::new(operation.response.clone()),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn operation(&self) -> HttpRelationWriteOperation {
        self.operation
    }

    pub(crate) fn resolved_request(&self) -> &RequestSpec {
        &self.resolved_request
    }

    pub(crate) fn response(&self) -> &ResponseSpec {
        &self.response
    }
}

impl HttpFetchTarget {
    pub(crate) fn from_resolved_table_request(
        relation: &HttpRelationSpec,
        resolved_request: RequestSpec,
    ) -> Self {
        Self {
            kind: HttpSqlTargetKind::Relation,
            name: Arc::from(relation.name()),
            columns: Arc::from(relation.columns().to_vec()),
            fetch_limit_default: relation.fetch_limit_default(),
            resolved_request,
            response: Arc::new(
                relation
                    .read()
                    .expect("resolved table request requires readable relation")
                    .response
                    .clone(),
            ),
            pagination: Arc::new(
                relation
                    .read()
                    .expect("resolved table request requires readable relation")
                    .pagination
                    .clone(),
            ),
        }
    }

    pub(crate) fn with_resolved_request(&self, resolved_request: RequestSpec) -> Self {
        Self {
            kind: self.kind,
            name: Arc::clone(&self.name),
            columns: Arc::clone(&self.columns),
            fetch_limit_default: self.fetch_limit_default,
            resolved_request,
            response: Arc::clone(&self.response),
            pagination: Arc::clone(&self.pagination),
        }
    }

    pub(crate) fn from_function(function: &SourceTableFunctionSpec) -> Self {
        Self {
            kind: HttpSqlTargetKind::Function,
            name: Arc::from(function.name.as_str()),
            columns: Arc::from(function.columns.clone()),
            fetch_limit_default: function.fetch_limit_default,
            resolved_request: function.request.clone(),
            response: Arc::new(function.response.clone()),
            pagination: Arc::new(function.pagination.clone()),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn kind(&self) -> HttpSqlTargetKind {
        self.kind
    }

    pub(crate) fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }

    pub(crate) fn fetch_limit_default(&self) -> Option<usize> {
        self.fetch_limit_default
    }

    pub(crate) fn resolved_request(&self) -> &RequestSpec {
        &self.resolved_request
    }

    pub(crate) fn response(&self) -> &ResponseSpec {
        &self.response
    }

    pub(crate) fn pagination(&self) -> &PaginationSpec {
        &self.pagination
    }
}
