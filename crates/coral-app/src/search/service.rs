//! Implements the gRPC `SearchService`.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use coral_api::v1::search_result::Payload;
use coral_api::v1::search_service_server::SearchService as SearchServiceApi;
use coral_api::v1::{
    ColumnHint, NativeSearchPath, SearchProvider, SearchProviderState, SearchProviderStatus,
    SearchRequest, SearchResponse, SearchResult, SearchResultTruncation, SearchResultType,
    SearchSurfaceKind,
};
use coral_engine::{
    CatalogInfo, ColumnInfo, TableFunctionArgumentInfo, TableFunctionInfo,
    TableFunctionResultColumnInfo, TableInfo,
};
use tonic::{Request, Response, Status};

use crate::bootstrap::{AppError, app_status};
use crate::query::manager::{QueryManager, QueryManagerError};
use crate::transport::{
    catalog_item_to_proto, grpc_span, instrument_grpc, query_status, table_function_to_proto,
    workspace_name_from_proto, workspace_to_proto,
};
use crate::workspaces::WorkspaceName;

const DEFAULT_SEARCH_LIMIT: u32 = 10;
const MAX_SEARCH_LIMIT: u32 = 50;
const MAX_QUERY_BYTES: usize = 512;
const MAX_COLUMN_HINTS_PER_SURFACE: usize = 2;

#[derive(Clone)]
pub(crate) struct SearchService {
    search: UniversalSearch,
}

impl SearchService {
    pub(crate) fn new(query_manager: QueryManager) -> Self {
        Self {
            search: UniversalSearch::new(query_manager),
        }
    }
}

#[tonic::async_trait]
impl SearchServiceApi for SearchService {
    async fn search(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<SearchResponse>, Status> {
        let span = grpc_span(&request);
        let search = self.search.clone();
        instrument_grpc(span, async move {
            let request = request.into_inner();
            let workspace_name = workspace_name_from_proto(request.workspace.as_ref())?;
            let limit = search_limit(request.limit).map_err(app_status)?;
            let response = search
                .search(&workspace_name, &request.query, limit)
                .await
                .map_err(query_status)?;
            Ok(Response::new(response))
        })
        .await
    }
}

#[derive(Clone)]
struct UniversalSearch {
    queries: QueryManager,
}

impl UniversalSearch {
    fn new(query_manager: QueryManager) -> Self {
        Self {
            queries: query_manager,
        }
    }

    async fn search(
        &self,
        workspace_name: &WorkspaceName,
        query: &str,
        limit: u32,
    ) -> Result<SearchResponse, QueryManagerError> {
        let terms = query_terms(query).map_err(QueryManagerError::App)?;
        let catalog = self.queries.list_catalog(workspace_name, None).await?;
        let mut candidates =
            dedupe_candidates(catalog_candidates(workspace_name, &catalog, &terms));
        candidates.sort();

        let total_count = candidates.len();
        let max_results = usize::try_from(limit).unwrap_or(usize::MAX);
        let truncated = total_count > max_results;
        let results = candidates
            .into_iter()
            .take(max_results)
            .map(|candidate| candidate.result)
            .collect::<Vec<_>>();
        let returned_count = u32::try_from(results.len()).unwrap_or(u32::MAX);
        let provider_state = if total_count == 0 {
            SearchProviderState::Empty
        } else {
            SearchProviderState::ResultsFound
        };

        Ok(SearchResponse {
            results,
            provider_statuses: vec![
                SearchProviderStatus {
                    provider: SearchProvider::CatalogMetadata as i32,
                    state: provider_state as i32,
                    note: catalog_provider_note(provider_state, total_count),
                },
                SearchProviderStatus {
                    provider: SearchProvider::ObservedValues as i32,
                    state: SearchProviderState::NotEnabled as i32,
                    note: "Observed-value search is not enabled in this release".to_string(),
                },
            ],
            truncation: Some(SearchResultTruncation {
                truncated,
                returned_count,
                max_results: limit,
                note: truncation_note(truncated, total_count, max_results),
            }),
        })
    }
}

#[derive(Clone)]
struct Candidate {
    key: String,
    score: u32,
    type_order: u8,
    result: SearchResult,
}

impl Eq for Candidate {}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (Reverse(self.score), self.type_order, self.key.as_str()).cmp(&(
            Reverse(other.score),
            other.type_order,
            other.key.as_str(),
        ))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn dedupe_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut deduped = BTreeMap::<String, Candidate>::new();
    for candidate in candidates {
        deduped
            .entry(candidate.key.clone())
            .and_modify(|existing| {
                if candidate.score > existing.score {
                    *existing = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    deduped.into_values().collect()
}

#[derive(Clone, Debug)]
struct QueryTerms {
    normalized_query: String,
    terms: Vec<String>,
}

fn search_limit(limit: u32) -> Result<u32, AppError> {
    if limit == 0 {
        return Ok(DEFAULT_SEARCH_LIMIT);
    }
    if limit > MAX_SEARCH_LIMIT {
        return Err(AppError::InvalidInput(format!(
            "search limit must be between 1 and {MAX_SEARCH_LIMIT}"
        )));
    }
    Ok(limit)
}

fn query_terms(query: &str) -> Result<QueryTerms, AppError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(AppError::InvalidInput(
            "argument 'query' must not be empty".to_string(),
        ));
    }
    if query.len() > MAX_QUERY_BYTES {
        return Err(AppError::InvalidInput(format!(
            "argument 'query' must be at most {MAX_QUERY_BYTES} bytes"
        )));
    }

    let normalized_query = normalize(query);
    let mut terms = query
        .split(|ch: char| !is_query_token_char(ch))
        .filter_map(|part| {
            let part = normalize(part);
            (part.len() > 1).then_some(part)
        })
        .collect::<Vec<_>>();
    if !terms.iter().any(|term| term == &normalized_query) {
        terms.push(normalized_query.clone());
    }
    terms.sort();
    terms.dedup();

    Ok(QueryTerms {
        normalized_query,
        terms,
    })
}

fn is_query_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '#' | '/' | '@')
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn catalog_candidates(
    workspace_name: &WorkspaceName,
    catalog: &CatalogInfo,
    terms: &QueryTerms,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for table in &catalog.tables {
        table_candidates(workspace_name, table, terms, &mut candidates);
    }
    for function in &catalog.table_functions {
        table_function_candidates(workspace_name, function, terms, &mut candidates);
    }
    candidates
}

fn table_candidates(
    workspace_name: &WorkspaceName,
    table: &TableInfo,
    terms: &QueryTerms,
    candidates: &mut Vec<Candidate>,
) {
    let name = qualified_name(&table.schema_name, &table.table_name);
    let fields = [
        ("schema_name", table.schema_name.as_str(), 2),
        ("table_name", table.table_name.as_str(), 4),
        ("name", name.as_str(), 4),
        ("description", table.description.as_str(), 2),
        ("guide", table.guide.as_str(), 1),
    ];
    if let Some((score, _matched_fields)) = score_fields(terms, fields) {
        candidates.push(Candidate {
            key: format!("catalog:table:{}.{}", table.schema_name, table.table_name),
            score,
            type_order: 1,
            result: SearchResult {
                r#type: SearchResultType::CatalogItem as i32,
                payload: Some(Payload::CatalogItem(catalog_item_to_proto(
                    workspace_name,
                    crate::catalog::discovery::CatalogItem::Table(table_summary(table)),
                ))),
            },
        });
    }

    let mut hints_for_surface = 0usize;
    for column in &table.columns {
        if hints_for_surface >= MAX_COLUMN_HINTS_PER_SURFACE {
            break;
        }
        let fields = [
            ("column_name", column.name.as_str(), 4),
            ("description", column.description.as_str(), 2),
            ("data_type", column.data_type.as_str(), 1),
        ];
        if let Some((score, matched_fields)) = score_fields(terms, fields) {
            candidates.push(column_hint_candidate(
                ColumnHintCandidate {
                    workspace_name,
                    schema_name: &table.schema_name,
                    surface_name: &table.table_name,
                    surface_kind: SearchSurfaceKind::Table,
                    name: &column.name,
                    data_type: &column.data_type,
                    required: column.is_required_filter,
                    description: &column.description,
                    matched_fields,
                },
                score + required_filter_boost(column),
            ));
            hints_for_surface += 1;
        }
    }

    for filter in &table.required_filters {
        if hints_for_surface >= MAX_COLUMN_HINTS_PER_SURFACE {
            break;
        }
        let fields = [("required_filter", filter.as_str(), 5)];
        if let Some((score, matched_fields)) = score_fields(terms, fields) {
            candidates.push(column_hint_candidate(
                ColumnHintCandidate {
                    workspace_name,
                    schema_name: &table.schema_name,
                    surface_name: &table.table_name,
                    surface_kind: SearchSurfaceKind::Table,
                    name: filter,
                    data_type: "",
                    required: true,
                    description: "Required table filter",
                    matched_fields,
                },
                score + 2,
            ));
            hints_for_surface += 1;
        }
    }
}

fn table_function_candidates(
    workspace_name: &WorkspaceName,
    function: &TableFunctionInfo,
    terms: &QueryTerms,
    candidates: &mut Vec<Candidate>,
) {
    let name = qualified_name(&function.schema_name, &function.function_name);
    let fields = [
        ("schema_name", function.schema_name.as_str(), 2),
        ("function_name", function.function_name.as_str(), 5),
        ("name", name.as_str(), 5),
        ("description", function.description.as_str(), 2),
        ("kind", function.kind.as_str(), 2),
    ];
    if let Some((score, matched_fields)) = score_fields(terms, fields) {
        candidates.push(Candidate {
            key: format!(
                "catalog:function:{}.{}",
                function.schema_name, function.function_name
            ),
            score,
            type_order: 1,
            result: SearchResult {
                r#type: SearchResultType::CatalogItem as i32,
                payload: Some(Payload::CatalogItem(catalog_item_to_proto(
                    workspace_name,
                    crate::catalog::discovery::CatalogItem::TableFunction(function.clone()),
                ))),
            },
        });

        if function.kind == "search" {
            candidates.push(native_search_path_candidate(
                workspace_name,
                function,
                matched_fields,
                score + 4,
            ));
        }
    }

    let mut hints_for_surface = 0usize;
    for argument in &function.arguments {
        if hints_for_surface >= MAX_COLUMN_HINTS_PER_SURFACE {
            break;
        }
        if let Some(candidate) = argument_hint_candidate(workspace_name, function, argument, terms)
        {
            candidates.push(candidate);
            hints_for_surface += 1;
        }
    }
    for column in &function.result_columns {
        if hints_for_surface >= MAX_COLUMN_HINTS_PER_SURFACE {
            break;
        }
        if let Some(candidate) =
            result_column_hint_candidate(workspace_name, function, column, terms)
        {
            candidates.push(candidate);
            hints_for_surface += 1;
        }
    }
}

fn argument_hint_candidate(
    workspace_name: &WorkspaceName,
    function: &TableFunctionInfo,
    argument: &TableFunctionArgumentInfo,
    terms: &QueryTerms,
) -> Option<Candidate> {
    let values = argument.values.join(" ");
    let fields = [
        ("argument", argument.name.as_str(), 5),
        ("allowed_values", values.as_str(), 2),
    ];
    let (score, matched_fields) = score_fields(terms, fields)?;
    Some(column_hint_candidate(
        ColumnHintCandidate {
            workspace_name,
            schema_name: &function.schema_name,
            surface_name: &function.function_name,
            surface_kind: SearchSurfaceKind::TableFunction,
            name: &argument.name,
            data_type: "",
            required: argument.required,
            description: "Table function argument",
            matched_fields,
        },
        score + u32::from(argument.required),
    ))
}

fn result_column_hint_candidate(
    workspace_name: &WorkspaceName,
    function: &TableFunctionInfo,
    column: &TableFunctionResultColumnInfo,
    terms: &QueryTerms,
) -> Option<Candidate> {
    let fields = [
        ("result_column", column.name.as_str(), 4),
        ("description", column.description.as_str(), 2),
        ("data_type", column.data_type.as_str(), 1),
    ];
    let (score, matched_fields) = score_fields(terms, fields)?;
    Some(column_hint_candidate(
        ColumnHintCandidate {
            workspace_name,
            schema_name: &function.schema_name,
            surface_name: &function.function_name,
            surface_kind: SearchSurfaceKind::TableFunction,
            name: &column.name,
            data_type: &column.data_type,
            required: false,
            description: &column.description,
            matched_fields,
        },
        score,
    ))
}

struct ColumnHintCandidate<'a> {
    workspace_name: &'a WorkspaceName,
    schema_name: &'a str,
    surface_name: &'a str,
    surface_kind: SearchSurfaceKind,
    name: &'a str,
    data_type: &'a str,
    required: bool,
    description: &'a str,
    matched_fields: Vec<String>,
}

fn column_hint_candidate(input: ColumnHintCandidate<'_>, score: u32) -> Candidate {
    Candidate {
        key: format!(
            "column:{}:{}.{}:{}",
            input.surface_kind.as_str_name(),
            input.schema_name,
            input.surface_name,
            input.name
        ),
        score,
        type_order: 2,
        result: SearchResult {
            r#type: SearchResultType::ColumnHint as i32,
            payload: Some(Payload::ColumnHint(ColumnHint {
                workspace: Some(workspace_to_proto(input.workspace_name)),
                schema_name: input.schema_name.to_string(),
                surface_name: input.surface_name.to_string(),
                surface_kind: input.surface_kind as i32,
                name: input.name.to_string(),
                data_type: input.data_type.to_string(),
                required: input.required,
                description: input.description.to_string(),
                matched_fields: input.matched_fields,
            })),
        },
    }
}

fn native_search_path_candidate(
    workspace_name: &WorkspaceName,
    function: &TableFunctionInfo,
    matched_fields: Vec<String>,
    score: u32,
) -> Candidate {
    Candidate {
        key: format!(
            "native_search:{}.{}",
            function.schema_name, function.function_name
        ),
        score,
        type_order: 0,
        result: SearchResult {
            r#type: SearchResultType::NativeSearchPath as i32,
            payload: Some(Payload::NativeSearchPath(NativeSearchPath {
                table_function: Some(table_function_to_proto(workspace_name, function.clone())),
                sql_call_example: sql_call_example(function),
                matched_fields,
            })),
        },
    }
}

fn score_fields<const N: usize>(
    terms: &QueryTerms,
    fields: [(&'static str, &str, u32); N],
) -> Option<(u32, Vec<String>)> {
    let mut score = 0u32;
    let mut matched_fields = BTreeSet::<&'static str>::new();
    for (field, value, weight) in fields {
        if value.trim().is_empty() {
            continue;
        }
        let normalized = normalize(value);
        let exact_bonus = u32::from(normalized == terms.normalized_query) * weight * 2;
        let matched_term_count = terms
            .terms
            .iter()
            .filter(|term| normalized.contains(term.as_str()))
            .count();
        let term_score = u32::try_from(matched_term_count).unwrap_or(u32::MAX) * weight;
        if exact_bonus > 0 || term_score > 0 {
            score += exact_bonus + term_score;
            matched_fields.insert(field);
        }
    }
    (score > 0).then(|| {
        (
            score,
            matched_fields
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        )
    })
}

fn table_summary(table: &TableInfo) -> TableInfo {
    let mut table = table.clone();
    table.columns.clear();
    table
}

fn required_filter_boost(column: &ColumnInfo) -> u32 {
    if column.is_required_filter { 2 } else { 0 }
}

fn qualified_name(schema_name: &str, name: &str) -> String {
    format!("{schema_name}.{name}")
}

fn sql_call_example(function: &TableFunctionInfo) -> String {
    let args = function
        .arguments
        .iter()
        .filter(|argument| argument.required)
        .map(|argument| format!("{} => '<{}>'", argument.name, argument.name))
        .collect::<Vec<_>>();
    format!(
        "SELECT * FROM {}.{}({}) LIMIT 10",
        function.schema_name,
        function.function_name,
        args.join(", ")
    )
}

fn catalog_provider_note(state: SearchProviderState, total_count: usize) -> String {
    match state {
        SearchProviderState::ResultsFound => {
            format!("Catalog metadata returned {total_count} candidate search hints")
        }
        SearchProviderState::Empty => "Catalog metadata returned no search hints".to_string(),
        _ => String::new(),
    }
}

fn truncation_note(truncated: bool, total_count: usize, max_results: usize) -> String {
    if truncated {
        format!("Returned {max_results} of {total_count} search hints")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use coral_api::v1::SearchSurfaceKind;

    use super::{query_terms, score_fields};

    #[test]
    fn query_terms_preserve_identifier_punctuation() {
        let terms = query_terms("payments-api #eng acme/repo").expect("terms");

        assert!(terms.terms.iter().any(|term| term == "payments-api"));
        assert!(terms.terms.iter().any(|term| term == "#eng"));
        assert!(terms.terms.iter().any(|term| term == "acme/repo"));
    }

    #[test]
    fn score_fields_matches_terms_and_exact_query() {
        let terms = query_terms("issue title").expect("terms");
        let (score, fields) = score_fields(
            &terms,
            [("name", "title", 4), ("description", "Issue title", 2)],
        )
        .expect("score");

        assert!(score >= 8);
        assert_eq!(fields, vec!["description", "name"]);
    }

    #[test]
    fn surface_kind_has_stable_proto_names() {
        assert_eq!(
            SearchSurfaceKind::Table.as_str_name(),
            "SEARCH_SURFACE_KIND_TABLE"
        );
    }
}
