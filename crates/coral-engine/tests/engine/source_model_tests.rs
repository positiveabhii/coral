use std::collections::BTreeMap;

use coral_engine::{CoralQuery, CoreError, QuerySource, StatusCode};
use coral_spec::{
    HttpMethod, OperationDetails, OperationInput, OperationResult, RestOperationDetails,
    RestPagination, RestParameter, RestParameterLocation, RestResponse, RestStatusCode,
    SOURCE_MODEL_IR_VERSION, ScalarType, SourceModelIr, SourceModelOperation, SourceModelSurface,
    SurfaceProtocol, TypeRef, parse_source_manifest_value,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::harness::{execution_to_rows, test_runtime};

#[tokio::test]
async fn source_model_registers_explicit_table_projection_from_imported_ir() {
    let source = source_model_source(true);

    let tables = CoralQuery::list_tables(&[source], test_runtime(), Some("github"), None)
        .await
        .expect("source-model table projection should register");

    assert_eq!(tables.len(), 1);
    let table = tables
        .first()
        .expect("source-model source should register one table");
    assert_eq!(table.schema_name, "github");
    assert_eq!(table.table_name, "issues");
    assert_eq!(table.required_filters, ["owner", "repo"]);
    let required_columns = table
        .columns
        .iter()
        .filter(|column| column.is_required_filter)
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(required_columns, ["owner", "repo"]);
}

#[tokio::test]
async fn source_model_registers_explicit_function_projection_metadata() {
    let source = source_model_source(true);

    let rows = execution_to_rows(
        &CoralQuery::execute_sql(
            &[source],
            test_runtime(),
            "SELECT schema_name, function_name, arguments_json, result_columns_json \
             FROM coral.table_functions WHERE schema_name = 'github'",
        )
        .await
        .expect("source-model function metadata should query"),
    );

    assert_eq!(rows.len(), 1);
    let row = rows
        .first()
        .expect("source-model source should register one function");
    assert_eq!(row["schema_name"], "github");
    assert_eq!(row["function_name"], "search_issues");
    assert_eq!(
        serde_json::from_str::<Value>(row["arguments_json"].as_str().unwrap()).unwrap(),
        json!([
            { "name": "q", "required": true, "values": [] },
            { "name": "per_page", "required": false, "values": [] }
        ])
    );
    assert_eq!(
        serde_json::from_str::<Value>(row["result_columns_json"].as_str().unwrap()).unwrap(),
        json!([
            { "name": "title", "type": "Utf8", "nullable": true, "description": "" }
        ])
    );
}

#[tokio::test]
async fn source_model_table_projection_error_names_missing_required_operation_input() {
    let source = source_model_source(false);

    let error = CoralQuery::validate_source(&source, test_runtime(), &[])
        .await
        .expect_err("table projection missing required operation input should fail");

    assert_eq!(error.status_code(), StatusCode::FailedPrecondition);
    let CoreError::FailedPrecondition(detail) = error else {
        panic!("expected failed precondition");
    };
    assert!(
        detail.contains("source schema 'github' projection/table 'issues'"),
        "error should name source schema and projection/table: {detail}"
    );
    assert!(
        detail.contains("missing required operation input 'owner'"),
        "error should name missing operation input: {detail}"
    );
}

#[tokio::test]
async fn source_model_rest_table_executes_path_query_and_list_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "number": 1, "title": "first" },
            { "number": 2, "title": "second" }
        ])))
        .mount(&server)
        .await;

    let source = source_model_runtime_source(
        &server.uri(),
        &[issues_table_projection()],
        vec![github_issues_operation(
            OperationResult::List {
                item: TypeRef::scalar(ScalarType::Json),
            },
            None,
        )],
    );

    let rows = execution_to_rows(
        &CoralQuery::execute_sql(
            &[source],
            test_runtime(),
            "SELECT number, title FROM github.issues \
             WHERE owner = 'acme' AND repo = 'coral' AND state = 'open' \
             ORDER BY number",
        )
        .await
        .expect("source-model REST table should query"),
    );

    assert_eq!(
        rows,
        vec![
            json!({ "number": 1, "title": "first" }),
            json!({ "number": 2, "title": "second" })
        ]
    );
}

#[tokio::test]
async fn source_model_rest_function_executes_path_params_and_singleton_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 7,
            "title": "single"
        })))
        .mount(&server)
        .await;

    let source = source_model_runtime_source(
        &server.uri(),
        &[json!({
            "name": "get_issue",
            "kind": "function",
            "surface": "github-rest",
            "operation": "issues/get",
            "columns": [
                { "name": "number", "type": "Int64" },
                { "name": "title", "type": "Utf8" }
            ]
        })],
        vec![github_get_issue_operation()],
    );

    let rows = execution_to_rows(
        &CoralQuery::execute_sql(
            &[source],
            test_runtime(),
            "SELECT number, title FROM github.get_issue(owner => 'acme', repo => 'coral', issue_number => 7)",
        )
        .await
        .expect("source-model REST function should query singleton response"),
    );

    assert_eq!(rows, vec![json!({ "number": 7, "title": "single" })]);
}

#[tokio::test]
async fn source_model_rest_function_executes_query_params_and_wrapped_response_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param("q", "repo:withcoral/coral label:bug"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 1,
            "items": [{ "title": "wrapped" }]
        })))
        .mount(&server)
        .await;

    let source = source_model_runtime_source(
        &server.uri(),
        &[json!({
            "name": "search_issues",
            "kind": "function",
            "surface": "github-rest",
            "operation": "search/issues-and-pull-requests",
            "columns": [{ "name": "title", "type": "Utf8" }]
        })],
        vec![github_search_operation()],
    );

    let rows = execution_to_rows(
        &CoralQuery::execute_sql(
            &[source],
            test_runtime(),
            "SELECT title FROM github.search_issues(q => 'repo:withcoral/coral label:bug')",
        )
        .await
        .expect("source-model REST function should query wrapped response"),
    );

    assert_eq!(rows, vec![json!({ "title": "wrapped" })]);
}

#[tokio::test]
async fn source_model_rest_table_executes_link_header_pagination() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues"))
        .and(query_param_is_missing("page"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("Link", "</repos/acme/coral/issues?page=2>; rel=\"next\"")
                .set_body_json(json!([{ "number": 1, "title": "first" }])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "number": 2, "title": "second" }
        ])))
        .mount(&server)
        .await;

    let source = source_model_runtime_source(
        &server.uri(),
        &[issues_table_projection()],
        vec![github_issues_operation(
            OperationResult::List {
                item: TypeRef::scalar(ScalarType::Json),
            },
            Some(RestPagination::LinkHeader {
                next_rel: Some("next".to_string()),
                page_size_input: None,
            }),
        )],
    );

    let rows = execution_to_rows(
        &CoralQuery::execute_sql(
            &[source],
            test_runtime(),
            "SELECT number, title FROM github.issues \
             WHERE owner = 'acme' AND repo = 'coral' \
             ORDER BY number",
        )
        .await
        .expect("source-model REST table should follow link header pagination"),
    );

    assert_eq!(
        rows,
        vec![
            json!({ "number": 1, "title": "first" }),
            json!({ "number": 2, "title": "second" })
        ]
    );
}

#[tokio::test]
async fn source_model_rest_table_executes_page_and_per_page_pagination() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues"))
        .and(query_param("page", "1"))
        .and(query_param("per_page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "number": 1, "title": "first" },
            { "number": 2, "title": "second" }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues"))
        .and(query_param("page", "2"))
        .and(query_param("per_page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "number": 3, "title": "third" }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/coral/issues"))
        .and(query_param("page", "3"))
        .and(query_param("per_page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let source = source_model_runtime_source(
        &server.uri(),
        &[issues_table_projection()],
        vec![github_issues_operation(
            OperationResult::List {
                item: TypeRef::scalar(ScalarType::Json),
            },
            Some(RestPagination::Page {
                page_input: "page".to_string(),
                page_size_input: Some("per_page".to_string()),
            }),
        )],
    );

    let rows = execution_to_rows(
        &CoralQuery::execute_sql(
            &[source],
            test_runtime(),
            "SELECT number, title FROM github.issues \
             WHERE owner = 'acme' AND repo = 'coral' AND per_page = 2 \
             ORDER BY number",
        )
        .await
        .expect("source-model REST table should page with per_page query param"),
    );

    assert_eq!(
        rows,
        vec![
            json!({ "number": 1, "title": "first" }),
            json!({ "number": 2, "title": "second" }),
            json!({ "number": 3, "title": "third" })
        ]
    );
}

fn source_model_source(include_required_input_columns: bool) -> QuerySource {
    let manifest =
        parse_source_manifest_value(source_model_manifest(include_required_input_columns))
            .expect("source-model manifest should parse");
    QuerySource::new(manifest, BTreeMap::default(), BTreeMap::default())
        .with_source_model_ir(source_model_ir())
}

fn source_model_runtime_source(
    base_url: &str,
    projections: &[Value],
    operations: Vec<SourceModelOperation>,
) -> QuerySource {
    let manifest = parse_source_manifest_value(json!({
        "name": "github",
        "version": "1.0.0",
        "dsl_version": 4,
        "backend": "source_model",
        "surfaces": [{
            "id": "github-rest",
            "type": "open-api",
            "url": "https://example.com/openapi.yaml",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "base_url": base_url
        }],
        "projections": projections
    }))
    .expect("source-model manifest should parse");
    QuerySource::new(manifest, BTreeMap::default(), BTreeMap::default()).with_source_model_ir(
        SourceModelIr {
            ir_version: SOURCE_MODEL_IR_VERSION,
            surfaces: vec![SourceModelSurface {
                id: "github-rest".to_string(),
                description: String::new(),
                protocol: SurfaceProtocol::Rest,
                base_url: Some(base_url.to_string()),
            }],
            types: Vec::new(),
            operations,
            entities: Vec::new(),
        },
    )
}

fn source_model_manifest(include_required_input_columns: bool) -> Value {
    let mut columns = vec![json!({ "name": "title", "type": "Utf8" })];
    if include_required_input_columns {
        columns.insert(
            0,
            json!({ "name": "repo", "type": "Utf8", "virtual": true }),
        );
        columns.insert(
            0,
            json!({ "name": "owner", "type": "Utf8", "virtual": true }),
        );
    }

    json!({
        "name": "github",
        "version": "1.0.0",
        "dsl_version": 4,
        "backend": "source_model",
        "surfaces": [{
            "id": "github-rest",
            "type": "open-api",
            "url": "https://example.com/openapi.yaml",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "base_url": "https://api.github.com"
        }],
        "projections": [
            {
                "name": "issues",
                "kind": "table",
                "surface": "github-rest",
                "operation": "issues/list-for-repo",
                "columns": columns
            },
            {
                "name": "search_issues",
                "kind": "function",
                "surface": "github-rest",
                "operation": "search/issues-and-pull-requests",
                "columns": [{ "name": "title", "type": "Utf8" }]
            }
        ]
    })
}

fn issues_table_projection() -> Value {
    json!({
        "name": "issues",
        "kind": "table",
        "surface": "github-rest",
        "operation": "issues/list-for-repo",
        "columns": [
            { "name": "owner", "type": "Utf8", "virtual": true, "expr": { "kind": "from_filter", "key": "owner" } },
            { "name": "repo", "type": "Utf8", "virtual": true, "expr": { "kind": "from_filter", "key": "repo" } },
            { "name": "state", "type": "Utf8", "virtual": true, "expr": { "kind": "from_filter", "key": "state" } },
            { "name": "per_page", "type": "Int64", "virtual": true, "expr": { "kind": "from_filter", "key": "per_page" } },
            { "name": "number", "type": "Int64" },
            { "name": "title", "type": "Utf8" }
        ]
    })
}

fn source_model_ir() -> SourceModelIr {
    SourceModelIr {
        ir_version: SOURCE_MODEL_IR_VERSION,
        surfaces: vec![SourceModelSurface {
            id: "github-rest".to_string(),
            description: String::new(),
            protocol: SurfaceProtocol::Rest,
            base_url: Some("https://api.github.com".to_string()),
        }],
        types: Vec::new(),
        operations: vec![
            rest_operation(
                "issues/list-for-repo",
                vec![
                    required_string_input("owner"),
                    required_string_input("repo"),
                    optional_integer_input("per_page"),
                ],
                OperationResult::List {
                    item: TypeRef::scalar(ScalarType::Json),
                },
            ),
            rest_operation(
                "search/issues-and-pull-requests",
                vec![
                    required_string_input("q"),
                    optional_integer_input("per_page"),
                ],
                OperationResult::WrappedList {
                    item: TypeRef::scalar(ScalarType::Json),
                    items_path: vec!["items".to_string()],
                    total_count_path: vec!["total_count".to_string()],
                },
            ),
        ],
        entities: Vec::new(),
    }
}

fn github_issues_operation(
    result: OperationResult,
    pagination: Option<RestPagination>,
) -> SourceModelOperation {
    rest_operation_with(
        "issues/list-for-repo",
        vec![
            required_string_input("owner"),
            required_string_input("repo"),
            optional_string_input("state"),
            optional_integer_input("page"),
            optional_integer_input("per_page"),
        ],
        result,
        "/repos/{owner}/{repo}/issues",
        vec![
            rest_parameter("owner", "owner", RestParameterLocation::Path, true),
            rest_parameter("repo", "repo", RestParameterLocation::Path, true),
            rest_parameter("state", "state", RestParameterLocation::Query, false),
            rest_parameter("per_page", "per_page", RestParameterLocation::Query, false),
        ],
        pagination,
    )
}

fn github_get_issue_operation() -> SourceModelOperation {
    rest_operation_with(
        "issues/get",
        vec![
            required_string_input("owner"),
            required_string_input("repo"),
            required_integer_input("issue_number"),
        ],
        OperationResult::Single {
            ty: TypeRef::scalar(ScalarType::Json),
        },
        "/repos/{owner}/{repo}/issues/{issue_number}",
        vec![
            rest_parameter("owner", "owner", RestParameterLocation::Path, true),
            rest_parameter("repo", "repo", RestParameterLocation::Path, true),
            rest_parameter(
                "issue_number",
                "issue_number",
                RestParameterLocation::Path,
                true,
            ),
        ],
        None,
    )
}

fn github_search_operation() -> SourceModelOperation {
    rest_operation_with(
        "search/issues-and-pull-requests",
        vec![required_string_input("q")],
        OperationResult::WrappedList {
            item: TypeRef::scalar(ScalarType::Json),
            items_path: vec!["items".to_string()],
            total_count_path: vec!["total_count".to_string()],
        },
        "/search/issues",
        vec![rest_parameter("q", "q", RestParameterLocation::Query, true)],
        None,
    )
}

fn rest_operation(
    id: &str,
    inputs: Vec<OperationInput>,
    result: OperationResult,
) -> SourceModelOperation {
    SourceModelOperation {
        id: id.to_string(),
        surface: "github-rest".to_string(),
        description: String::new(),
        inputs,
        result,
        details: OperationDetails::Rest {
            rest: Box::new(RestOperationDetails {
                method: HttpMethod::GET,
                path: "/unused".to_string(),
                parameters: Vec::new(),
                request_body: None,
                responses: Vec::new(),
                pagination: None,
            }),
        },
    }
}

fn rest_operation_with(
    id: &str,
    inputs: Vec<OperationInput>,
    result: OperationResult,
    path: &str,
    parameters: Vec<RestParameter>,
    pagination: Option<RestPagination>,
) -> SourceModelOperation {
    SourceModelOperation {
        id: id.to_string(),
        surface: "github-rest".to_string(),
        description: String::new(),
        inputs,
        result,
        details: OperationDetails::Rest {
            rest: Box::new(RestOperationDetails {
                method: HttpMethod::GET,
                path: path.to_string(),
                parameters,
                request_body: None,
                responses: vec![RestResponse {
                    status: RestStatusCode::Code(200),
                    content_type: Some("application/json".to_string()),
                    body: Some(TypeRef::scalar(ScalarType::Json)),
                    body_path: Vec::new(),
                    error: false,
                }],
                pagination,
            }),
        },
    }
}

fn rest_parameter(
    name: &str,
    input: &str,
    location: RestParameterLocation,
    required: bool,
) -> RestParameter {
    RestParameter {
        name: name.to_string(),
        input: input.to_string(),
        location,
        required,
        style: None,
        explode: None,
    }
}

fn required_string_input(name: &str) -> OperationInput {
    OperationInput {
        name: name.to_string(),
        ty: TypeRef::scalar(ScalarType::String),
        required: true,
        description: String::new(),
    }
}

fn optional_string_input(name: &str) -> OperationInput {
    OperationInput {
        name: name.to_string(),
        ty: TypeRef::scalar(ScalarType::String),
        required: false,
        description: String::new(),
    }
}

fn required_integer_input(name: &str) -> OperationInput {
    OperationInput {
        name: name.to_string(),
        ty: TypeRef::scalar(ScalarType::Integer),
        required: true,
        description: String::new(),
    }
}

fn optional_integer_input(name: &str) -> OperationInput {
    OperationInput {
        name: name.to_string(),
        ty: TypeRef::scalar(ScalarType::Integer),
        required: false,
        description: String::new(),
    }
}
