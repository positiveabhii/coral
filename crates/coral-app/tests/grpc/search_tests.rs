#![allow(
    clippy::indexing_slicing,
    reason = "Proto regression assertions intentionally fail loudly in tests."
)]

use coral_api::v1::search_result::Payload;
use coral_api::v1::{
    SearchProvider, SearchProviderState, SearchRequest, SearchResultType, SearchSurfaceKind,
};
use coral_client::default_workspace;
use tonic::Request;

use super::harness::{GrpcHarness, fixture_manifest_with_functions_yaml};

#[tokio::test]
async fn search_returns_typed_metadata_and_native_search_results() {
    let harness = GrpcHarness::new().await;
    harness
        .import_source(
            fixture_manifest_with_functions_yaml(),
            Vec::new(),
            Vec::new(),
        )
        .await;

    let response = harness
        .search_client()
        .search(Request::new(SearchRequest {
            workspace: Some(default_workspace()),
            query: "issue search title".to_string(),
            limit: 10,
        }))
        .await
        .expect("search")
        .into_inner();

    assert_provider_state(
        &response,
        SearchProvider::CatalogMetadata,
        SearchProviderState::ResultsFound,
    );
    assert_provider_state(
        &response,
        SearchProvider::ObservedValues,
        SearchProviderState::NotEnabled,
    );
    assert!(!response.truncation.expect("truncation").truncated);
    assert!(response.results.iter().any(|result| result.r#type
        == SearchResultType::NativeSearchPath as i32
        && matches!(
            result.payload.as_ref(),
            Some(Payload::NativeSearchPath(path))
                if path
                    .table_function
                    .as_ref()
                    .is_some_and(|function| function.name == "search_issues")
                    && path.sql_call_example.contains("searchy.search_issues")
                    && path.sql_call_example.contains("q => '<q>'")
        )));
    assert!(response.results.iter().any(|result| result.r#type
        == SearchResultType::CatalogItem as i32
        && matches!(
            result.payload.as_ref(),
            Some(Payload::CatalogItem(item))
                if item.item.as_ref().is_some_and(|item| match item {
                    coral_api::v1::catalog_item::Item::TableFunction(function) =>
                        function.name == "search_issues" && function.kind == "search",
                    coral_api::v1::catalog_item::Item::Table(_) => false,
                })
        )));
    assert!(response.results.iter().any(|result| result.r#type
        == SearchResultType::ColumnHint as i32
        && matches!(
            result.payload.as_ref(),
            Some(Payload::ColumnHint(hint))
                if hint.surface_name == "search_issues"
                    && hint.surface_kind == SearchSurfaceKind::TableFunction as i32
                    && hint.name == "title"
        )));
}

#[tokio::test]
async fn search_rejects_empty_and_too_large_queries() {
    let harness = GrpcHarness::new().await;

    let empty = harness
        .search_client()
        .search(Request::new(SearchRequest {
            workspace: Some(default_workspace()),
            query: "   ".to_string(),
            limit: 10,
        }))
        .await
        .expect_err("empty query should fail");
    assert_eq!(empty.code(), tonic::Code::InvalidArgument);
    assert!(empty.message().contains("query"));

    let too_long = harness
        .search_client()
        .search(Request::new(SearchRequest {
            workspace: Some(default_workspace()),
            query: "a".repeat(513),
            limit: 10,
        }))
        .await
        .expect_err("long query should fail");
    assert_eq!(too_long.code(), tonic::Code::InvalidArgument);
    assert!(too_long.message().contains("at most 512 bytes"));
}

#[tokio::test]
async fn search_applies_limit_and_reports_truncation() {
    let harness = GrpcHarness::new().await;
    harness
        .import_source(
            fixture_manifest_with_functions_yaml(),
            Vec::new(),
            Vec::new(),
        )
        .await;

    let response = harness
        .search_client()
        .search(Request::new(SearchRequest {
            workspace: Some(default_workspace()),
            query: "issue".to_string(),
            limit: 1,
        }))
        .await
        .expect("search")
        .into_inner();
    let truncation = response.truncation.expect("truncation");

    assert_eq!(response.results.len(), 1);
    assert!(truncation.truncated);
    assert_eq!(truncation.returned_count, 1);
    assert_eq!(truncation.max_results, 1);
}

fn assert_provider_state(
    response: &coral_api::v1::SearchResponse,
    provider: SearchProvider,
    state: SearchProviderState,
) {
    let status = response
        .provider_statuses
        .iter()
        .find(|status| status.provider == provider as i32)
        .expect("provider status");
    assert_eq!(status.state, state as i32);
}
