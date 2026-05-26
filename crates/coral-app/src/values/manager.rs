use tokio::task;

use crate::bootstrap::AppError;
use crate::state::AppStateLayout;
use crate::values::store::{
    StoredValueSearchPage, StoredValueSearchRequest, StoredValueSearchResult, ValueMemoryError,
    ValueMemoryStore,
};
use crate::workspaces::WorkspaceName;

#[derive(Debug, Clone)]
pub(crate) struct SearchValuesRequest {
    pub(crate) workspace_name: WorkspaceName,
    pub(crate) term: String,
    pub(crate) schema_name: Option<String>,
    pub(crate) table_name: Option<String>,
    pub(crate) column_path: Option<String>,
    pub(crate) limit: u32,
    pub(crate) offset: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ValueSearchResult {
    pub(crate) workspace_name: WorkspaceName,
    pub(crate) schema_name: String,
    pub(crate) table_name: String,
    pub(crate) column_path: String,
    pub(crate) value: String,
    pub(crate) value_truncated: bool,
    pub(crate) seen_count: u64,
    pub(crate) first_seen_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) field_total_count: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct SearchValuesPage {
    pub(crate) values: Vec<ValueSearchResult>,
    pub(crate) total_count: u32,
    pub(crate) limit: u32,
    pub(crate) offset: u32,
    pub(crate) has_more: bool,
    pub(crate) next_offset: u32,
}

#[derive(Clone)]
pub(crate) struct ValueMemoryManager {
    layout: AppStateLayout,
}

impl ValueMemoryManager {
    pub(crate) fn new(layout: AppStateLayout) -> Self {
        Self { layout }
    }

    pub(crate) async fn search_values(
        &self,
        request: SearchValuesRequest,
    ) -> Result<SearchValuesPage, AppError> {
        let store = ValueMemoryStore::new(self.layout.value_memory_file(&request.workspace_name));
        let stored_request = StoredValueSearchRequest {
            workspace_name: request.workspace_name.as_str().to_string(),
            term: request.term,
            schema_name: request.schema_name,
            table_name: request.table_name,
            column_path: request.column_path,
            limit: request.limit,
            offset: request.offset,
        };
        task::spawn_blocking(move || store.search(stored_request))
            .await?
            .map(search_page_from_store)
            .map_err(value_memory_error_to_app)
    }
}

fn search_page_from_store(page: StoredValueSearchPage) -> SearchValuesPage {
    SearchValuesPage {
        values: page
            .values
            .into_iter()
            .filter_map(value_search_result_from_store)
            .collect(),
        total_count: page.total_count,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        next_offset: page.next_offset,
    }
}

fn value_search_result_from_store(result: StoredValueSearchResult) -> Option<ValueSearchResult> {
    let workspace_name = WorkspaceName::parse(&result.workspace_name).ok()?;
    Some(ValueSearchResult {
        workspace_name,
        schema_name: result.schema_name,
        table_name: result.table_name,
        column_path: result.column_path,
        value: result.value,
        value_truncated: result.value_truncated,
        seen_count: result.seen_count,
        first_seen_at: result.first_seen_at,
        last_seen_at: result.last_seen_at,
        field_total_count: result.field_total_count,
    })
}

pub(crate) fn value_memory_error_to_app(error: ValueMemoryError) -> AppError {
    match error {
        ValueMemoryError::InvalidInput(detail) => AppError::InvalidInput(detail),
        ValueMemoryError::Io(error) => AppError::Io(error),
        ValueMemoryError::Sqlite(error) => {
            AppError::FailedPrecondition(format!("value memory store is unavailable: {error}"))
        }
    }
}
