use std::sync::Arc;

use coral_api::v1::{Source, Table};
use rmcp::{
    ErrorData,
    model::{CallToolResult, Content, Tool, ToolAnnotations},
};
use serde_json::{Map, Value, json};

use super::resources::{source_names, visible_schema_count, visible_table_count};

pub(crate) fn sql_tool(sources: &[Source], tables: &[Table]) -> Tool {
    Tool::new(
        "sql",
        sql_tool_description(sources, tables),
        json_object_schema(&json!({
            "type": "object",
            "required": ["sql"],
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "A single SQL statement to execute."
                }
            }
        })),
    )
    .with_annotations(
        ToolAnnotations::with_title("Run SQL")
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(true),
    )
}

pub(crate) fn list_tables_tool(sources: &[Source], tables: &[Table]) -> Tool {
    Tool::new(
        "list_tables",
        list_tables_description(sources, tables),
        json_object_schema(&json!({
            "type": "object",
            "properties": {}
        })),
    )
    .with_annotations(
        ToolAnnotations::with_title("List Tables")
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

pub(crate) fn required_string_argument(
    arguments: Option<&Map<String, Value>>,
    key: &str,
) -> Result<String, ErrorData> {
    let value = arguments
        .and_then(|arguments| arguments.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("missing string argument '{key}'"), None)
        })?;
    Ok(value.to_string())
}

pub(crate) fn build_tool_result(value: Value) -> Result<CallToolResult, ErrorData> {
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    let mut result = CallToolResult::structured(value);
    result.content = vec![Content::text(pretty)];
    Ok(result)
}

fn sql_tool_description(sources: &[Source], tables: &[Table]) -> String {
    let source_clause = sources_clause(sources);
    if tables.is_empty() {
        format!(
            "Run a SQL query against local Coral sources. {source_clause}, but no visible SQL schemas are currently available."
        )
    } else {
        format!(
            "Run a SQL query against local Coral sources. {source_clause}. {} visible SQL schema(s) are currently available.",
            visible_schema_count(tables)
        )
    }
}

fn list_tables_description(sources: &[Source], tables: &[Table]) -> String {
    format!(
        "List queryable fully qualified tables. {}. {} table(s) are currently visible.",
        sources_clause(sources),
        visible_table_count(tables)
    )
}

fn sources_clause(sources: &[Source]) -> String {
    let names = source_names(sources);
    if names.is_empty() {
        "0 configured source(s)".to_string()
    } else {
        format!("{} configured source(s): {}", names.len(), names.join(", "))
    }
}

fn json_object_schema(value: &Value) -> Arc<Map<String, Value>> {
    Arc::new(
        value
            .as_object()
            .cloned()
            .expect("tool schemas should be JSON objects"),
    )
}
