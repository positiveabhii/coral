//! Implements the gRPC `ValueService`.

use coral_api::v1::value_service_server::ValueService as ValueServiceApi;
use coral_api::v1::{
    PaginationResponse, SearchValuesRequest as SearchValuesRequestProto,
    SearchValuesResponse as SearchValuesResponseProto, ValueSearchResult as ValueSearchResultProto,
};
use tonic::{Request, Response, Status};

use crate::bootstrap::app_status;
use crate::transport::{grpc_span, instrument_grpc, workspace_name_from_proto, workspace_to_proto};
use crate::values::manager::{
    SearchValuesPage, SearchValuesRequest, ValueMemoryManager, ValueSearchResult,
};

#[derive(Clone)]
pub(crate) struct ValueService {
    values: ValueMemoryManager,
}

impl ValueService {
    pub(crate) fn new(values: ValueMemoryManager) -> Self {
        Self { values }
    }
}

#[tonic::async_trait]
impl ValueServiceApi for ValueService {
    async fn search_values(
        &self,
        request: Request<SearchValuesRequestProto>,
    ) -> Result<Response<SearchValuesResponseProto>, Status> {
        let span = grpc_span(&request);
        let values = self.values.clone();
        instrument_grpc(span, async move {
            let inner = request.into_inner();
            let workspace_name = workspace_name_from_proto(inner.workspace.as_ref())?;
            let pagination = inner.pagination.unwrap_or_default();
            let request = SearchValuesRequest {
                workspace_name,
                term: inner.term,
                schema_name: optional_proto_string(inner.schema_name),
                table_name: optional_proto_string(inner.table_name),
                column_path: optional_proto_string(inner.column_path),
                limit: pagination.limit,
                offset: pagination.offset,
            };
            values
                .search_values(request)
                .await
                .map(search_values_response_to_proto)
                .map(Response::new)
                .map_err(app_status)
        })
        .await
    }
}

fn optional_proto_string(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn search_values_response_to_proto(page: SearchValuesPage) -> SearchValuesResponseProto {
    SearchValuesResponseProto {
        values: page
            .values
            .into_iter()
            .map(value_search_result_to_proto)
            .collect(),
        pagination: Some(PaginationResponse {
            total_count: page.total_count,
            limit: page.limit,
            offset: page.offset,
            has_more: page.has_more,
            next_offset: page.next_offset,
        }),
    }
}

fn value_search_result_to_proto(result: ValueSearchResult) -> ValueSearchResultProto {
    ValueSearchResultProto {
        workspace: Some(workspace_to_proto(&result.workspace_name)),
        schema_name: result.schema_name,
        table_name: result.table_name,
        column_path: result.column_path,
        value: result.value,
        value_truncated: result.value_truncated,
        seen_count: result.seen_count,
        first_seen_at: result.first_seen_at,
        last_seen_at: result.last_seen_at,
        field_total_count: result.field_total_count,
    }
}
