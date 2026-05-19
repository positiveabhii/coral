use std::collections::BTreeMap;

use coral_engine::{CoralQuery, CoreError, QuerySource, StatusCode};
use coral_spec::{
    HttpMethod, OperationDetails, OperationInput, OperationResult, RestOperationDetails,
    SOURCE_MODEL_IR_VERSION, ScalarType, SourceModelIr, SourceModelOperation, SourceModelSurface,
    SurfaceProtocol, TypeRef, parse_source_manifest_value,
};
use serde_json::{Value, json};

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

fn source_model_source(include_required_input_columns: bool) -> QuerySource {
    let manifest =
        parse_source_manifest_value(source_model_manifest(include_required_input_columns))
            .expect("source-model manifest should parse");
    QuerySource::new(manifest, BTreeMap::default(), BTreeMap::default())
        .with_source_model_ir(source_model_ir())
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

fn required_string_input(name: &str) -> OperationInput {
    OperationInput {
        name: name.to_string(),
        ty: TypeRef::scalar(ScalarType::String),
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
