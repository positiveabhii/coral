//! MCP Code Mode projection over Coral's finite function bridge.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use coral_api::v1::Table;
use coral_code_mode::{
    CodeModeNestedToolCall, CodeModeService, CodeModeToolKind, CodeModeTurnHost,
    CodeModeTurnWorker, ExecuteRequest, FunctionCallOutputContentItem, RuntimeResponse,
    ToolDefinition, ToolName, WaitOutcome, WaitRequest, build_exec_tool_description,
    build_wait_tool_description, normalize_code_mode_identifier, parse_exec_source,
};
use rmcp::model::{Content, Tool};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::bridge::{BridgeCallOutcome, CoralToolBridge, bridge_outcome_result};
use crate::surface::{ExecArguments, WaitArguments};
use crate::telemetry;

const MAX_NESTED_CALLS_PER_CELL: usize = 100;
const RESULT_SLOT: &str = "__coral_code_mode_result";
const TAGGED_TEMPLATE_KEY: &str = "__coral_code_mode_tagged_template";
const CORAL_SQL_TYPESCRIPT_DECLARATIONS: &str = r"type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };
type SqlValue = JsonValue;
type SqlRow = Record<string, SqlValue>;
type SqlType = { kind: string; [key: string]: JsonValue };
type SqlColumn<TName extends string = string, TType extends SqlType = SqlType> = {
  name: TName;
  data_type: TType;
  nullable: boolean;
};
type SqlParamValue = null | boolean | number | string;
type SqlParams = SqlParamValue[] | Record<string, SqlParamValue>;
type SqlInput = string | { sql: string; params?: SqlParams };
type SqlResult<TRow extends SqlRow = SqlRow> = {
  columns: SqlColumn[];
  rows: TRow[];
  row_count: number;
};
type SqlFunction = {
  <TRow extends SqlRow = SqlRow>(input: SqlInput): Promise<SqlResult<TRow>>;
  <TRow extends SqlRow = SqlRow>(
    strings: TemplateStringsArray,
    ...params: SqlParamValue[]
  ): Promise<SqlResult<TRow>>;
};
type CoralToolError = Error & {
  summary?: string;
  detail?: string;
  grpc_code?: string;
  retryable?: boolean;
  metadata?: Record<string, unknown>;
};
declare const tools: { sql: SqlFunction };";

pub(crate) struct CodeModeState {
    service: CodeModeService,
    host: Arc<CoralCodeModeHost>,
    _worker: CodeModeTurnWorker,
    waiting_cells: Mutex<HashSet<String>>,
}

impl CodeModeState {
    pub(crate) fn new(bridge: CoralToolBridge) -> Self {
        let service = CodeModeService::new();
        let host = Arc::new(CoralCodeModeHost::new(bridge));
        let worker = service.start_turn_worker(host.clone());
        Self {
            service,
            host,
            _worker: worker,
            waiting_cells: Mutex::new(HashSet::new()),
        }
    }

    pub(crate) fn exec_description(
        nested_tools: &[Tool],
        schema_declarations: Option<&str>,
    ) -> String {
        let definitions = tool_definitions(nested_tools);
        let mut description =
            build_exec_tool_description(&definitions, &BTreeMap::new(), true, false);
        description.push_str(
            r#"

Coral guidance:
- Return the JSON-serializable value you want `exec` to return; do not rely on printing, a bare final expression, or a bare awaited tool call.
- Prefer one SQL query for filtering, projection, joins, grouping, ordering, and limits. Coral SQL is backed by DataFusion, so these operations belong in SQL rather than JavaScript loops when possible.
- Use `information_schema` and `LIMIT 0` queries for schema inspection when you need the concrete output columns before fetching rows.
- Use source-aware `LIMIT` pushdown by putting `LIMIT` in SQL; do not fetch broad result sets and slice them in JavaScript.
- Use registered JSON SQL functions such as `json_get_str`, `json_get_int`, `json_get_bool`, and `json_as_text` for filtering and projection over JSON payloads.
- Use `tools.list_catalog`, `tools.search_catalog`, `tools.describe_table`, and `tools.list_columns` for discovery, then query with `tools.sql`.
- `tools.sql("SELECT 1 AS n")` and `tools.sql({ sql: "SELECT $1 AS n", params: [1] })` return `{ columns, rows, row_count }`.
- `tools.sql` also supports tagged-template syntax such as `tools.sql`SELECT ${value} AS n``; template values become bound parameters, not interpolated SQL text.
- Models may optionally narrow simple SQL results, for example `await tools.sql<{ n: number }>("SELECT 1 AS n")`; this is a model-authored assertion, not runtime SQL inference.
- Nested tools are limited to Coral's finite MCP functions. Source tables and provider API operations are queryable through SQL, not direct `tools.*` functions.
"#,
        );
        description.push_str("\nCoral SQL TypeScript helpers:\n```ts\n");
        description.push_str(CORAL_SQL_TYPESCRIPT_DECLARATIONS);
        description.push_str("\n```");
        if let Some(schema_declarations) = schema_declarations.filter(|value| !value.is_empty()) {
            description.push_str("\nCoral schema declarations:\n```ts\n");
            description.push_str(schema_declarations);
            description.push_str("\n```");
        }
        description
    }

    pub(crate) async fn execute(
        &self,
        arguments: ExecArguments,
        nested_tools: &[Tool],
    ) -> BridgeCallOutcome {
        let parsed = match parse_exec_source(&arguments.source) {
            Ok(parsed) => parsed,
            Err(error) => return failed_code_mode_result(error),
        };
        let source = wrap_source(&parsed.code);
        let cell_id = self.service.allocate_cell_id();
        telemetry::record_code_mode_cell_id(&tracing::Span::current(), &cell_id);
        let request = ExecuteRequest {
            cell_id,
            enabled_tools: tool_definitions(nested_tools),
            source,
            stored_values: self.service.stored_values().await,
            yield_time_ms: arguments.yield_time_ms.or(parsed.yield_time_ms),
            max_output_tokens: arguments.max_output_tokens.or(parsed.max_output_tokens),
        };

        match self.service.execute(request).await {
            Ok(response) => self.runtime_response_result(response).await,
            Err(error) => failed_code_mode_result(error),
        }
    }

    pub(crate) async fn wait(&self, arguments: WaitArguments) -> BridgeCallOutcome {
        let cell_id = arguments.cell_id.clone();
        telemetry::record_code_mode_cell_id(&tracing::Span::current(), &cell_id);
        if let Err(outcome) = self.begin_wait(&cell_id).await {
            return outcome;
        }
        let response = self
            .service
            .wait(WaitRequest {
                cell_id: arguments.cell_id,
                yield_time_ms: arguments
                    .yield_time_ms
                    .unwrap_or(coral_code_mode::DEFAULT_WAIT_YIELD_TIME_MS),
                terminate: arguments.terminate,
            })
            .await;
        self.end_wait(&cell_id).await;

        match response {
            Ok(WaitOutcome::LiveCell(response) | WaitOutcome::MissingCell(response)) => {
                self.runtime_response_result(response).await
            }
            Err(error) => failed_code_mode_result(error),
        }
    }

    async fn runtime_response_result(&self, response: RuntimeResponse) -> BridgeCallOutcome {
        let cell_id = runtime_response_cell_id(&response).to_string();
        let terminal = !matches!(response, RuntimeResponse::Yielded { .. });
        let result = runtime_response_to_bridge(response);
        if terminal {
            self.host.clear_cell(&cell_id).await;
        }
        result
    }

    async fn begin_wait(&self, cell_id: &str) -> Result<(), BridgeCallOutcome> {
        let mut waiting_cells = self.waiting_cells.lock().await;
        if !waiting_cells.insert(cell_id.to_string()) {
            return Err(failed_code_mode_result(format!(
                "exec cell {cell_id} already has a wait in progress"
            )));
        }
        Ok(())
    }

    async fn end_wait(&self, cell_id: &str) {
        self.waiting_cells.lock().await.remove(cell_id);
    }
}

struct CoralCodeModeHost {
    bridge: CoralToolBridge,
    nested_call_counts: Mutex<HashMap<String, usize>>,
}

impl CoralCodeModeHost {
    fn new(bridge: CoralToolBridge) -> Self {
        Self {
            bridge,
            nested_call_counts: Mutex::new(HashMap::new()),
        }
    }

    async fn clear_cell(&self, cell_id: &str) {
        self.nested_call_counts.lock().await.remove(cell_id);
    }

    async fn increment_nested_call(&self, cell_id: &str) -> Result<(), String> {
        let mut counts = self.nested_call_counts.lock().await;
        let count = counts.entry(cell_id.to_string()).or_default();
        *count = count.saturating_add(1);
        if *count > MAX_NESTED_CALLS_PER_CELL {
            return Err(format!(
                "code mode cell {cell_id} exceeded the nested call limit of {MAX_NESTED_CALLS_PER_CELL}"
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl CodeModeTurnHost for CoralCodeModeHost {
    async fn invoke_tool(
        &self,
        invocation: CodeModeNestedToolCall,
        _cancellation_token: CancellationToken,
    ) -> Result<Value, String> {
        if invocation.tool_kind != CodeModeToolKind::Function {
            return Err("Coral Code Mode only supports function tools".to_string());
        }
        self.increment_nested_call(&invocation.cell_id).await?;
        let tool_name = invocation.tool_name.name;
        let input = normalize_nested_tool_input(&tool_name, invocation.input)?;
        let arguments = input.as_object().ok_or_else(|| {
            format!("tools.{tool_name} expects an object argument in Coral Code Mode")
        })?;
        bridge_outcome_result(self.bridge.call(&tool_name, Some(arguments)).await)
    }
}

pub(crate) fn tool_definitions(tools: &[Tool]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|tool| {
            let name = tool.name.to_string();
            ToolDefinition {
                name: normalize_code_mode_identifier(&name),
                tool_name: ToolName::plain(name),
                description: tool.description.as_deref().unwrap_or_default().to_string(),
                kind: CodeModeToolKind::Function,
                input_schema: Some(Value::Object((*tool.input_schema).clone())),
                output_schema: tool
                    .output_schema
                    .as_ref()
                    .map(|schema| Value::Object((**schema).clone())),
            }
        })
        .collect()
}

pub(crate) fn wait_description() -> &'static str {
    build_wait_tool_description()
}

pub(crate) fn schema_declarations(tables: &[Table]) -> String {
    if tables.is_empty() {
        return String::new();
    }

    let mut output = String::from(
        "declare namespace CoralSchema {\n  type Row<S extends keyof Tables, T extends keyof Tables[S]> = Tables[S][T];\n  interface Tables {\n",
    );
    let mut current_schema: Option<&str> = None;
    for table in tables {
        if current_schema != Some(table.schema_name.as_str()) {
            if current_schema.is_some() {
                output.push_str("    };\n");
            }
            current_schema = Some(&table.schema_name);
            output.push_str("    ");
            output.push_str(&quoted_ts_key(&table.schema_name));
            output.push_str(": {\n");
        }
        output.push_str("      ");
        output.push_str(&quoted_ts_key(&table.name));
        output.push_str(": {\n");
        for column in &table.columns {
            output.push_str("        ");
            output.push_str(&quoted_ts_key(&column.name));
            output.push_str(": ");
            output.push_str(typescript_type_for_datafusion(&column.data_type));
            if column.nullable {
                output.push_str(" | null");
            }
            output.push_str(";\n");
        }
        output.push_str("      };\n");
    }
    output.push_str("    };\n  }\n}\n");
    output
}

fn normalize_nested_tool_input(tool_name: &str, input: Option<Value>) -> Result<Value, String> {
    match (tool_name, input) {
        ("sql", Some(Value::String(sql))) => Ok(json!({ "sql": sql })),
        ("sql", Some(Value::Object(mut input))) if input.contains_key(TAGGED_TEMPLATE_KEY) => {
            tagged_template_sql_input(&mut input)
        }
        (_, Some(input @ Value::Object(_))) => Ok(input),
        (_, None) => Ok(Value::Object(Map::new())),
        (_, Some(other)) => Err(format!(
            "tools.{tool_name} expects an object argument, got {}",
            json_type_name(&other)
        )),
    }
}

fn tagged_template_sql_input(input: &mut Map<String, Value>) -> Result<Value, String> {
    let template = input
        .remove(TAGGED_TEMPLATE_KEY)
        .ok_or_else(|| "tagged-template input missing payload".to_string())?;
    let template = template
        .as_object()
        .ok_or_else(|| "tagged-template payload must be an object".to_string())?;
    let strings = template
        .get("strings")
        .and_then(Value::as_array)
        .ok_or_else(|| "tagged-template strings must be an array".to_string())?;
    let values = template
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| "tagged-template values must be an array".to_string())?;
    if strings.len() != values.len().saturating_add(1) {
        return Err("tagged-template strings length must equal values length plus one".to_string());
    }

    let mut sql = String::new();
    for (index, segment) in strings.iter().enumerate() {
        let segment = segment
            .as_str()
            .ok_or_else(|| "tagged-template strings must contain only strings".to_string())?;
        sql.push_str(segment);
        if index < values.len() {
            sql.push('$');
            sql.push_str(&(index + 1).to_string());
        }
    }
    Ok(json!({
        "sql": sql,
        "params": values
    }))
}

fn wrap_source(source: &str) -> String {
    let source = source.trim();
    if looks_like_function_expression(source) {
        format!(
            r#"const __coral_code_mode_entry = ({source});
if (typeof __coral_code_mode_entry !== "function") {{
  throw new TypeError("Code Mode function expression must evaluate to a function");
}}
globalThis.{RESULT_SLOT} = await __coral_code_mode_entry();"#
        )
    } else {
        format!(
            r"globalThis.{RESULT_SLOT} = await (async () => {{
{source}
}})();"
        )
    }
}

fn looks_like_function_expression(source: &str) -> bool {
    if let Some(after_async) = source.strip_prefix("async") {
        if after_async.trim_start().starts_with('(') {
            return true;
        }
        if after_async.starts_with(" function(") || after_async.starts_with(" function (") {
            return true;
        }
    }
    if source.starts_with("function(") || source.starts_with("function (") {
        return true;
    }
    source.starts_with('(')
        && (source.contains("=>")
            || source.starts_with("(function")
            || source.starts_with("(async function"))
}

fn runtime_response_to_bridge(response: RuntimeResponse) -> BridgeCallOutcome {
    match response {
        RuntimeResponse::Yielded {
            cell_id,
            content_items,
        } => running_code_mode_result(&cell_id, content_items),
        RuntimeResponse::Terminated {
            cell_id,
            content_items,
        } => code_mode_result(
            json!({
                "status": "terminated",
                "cell_id": cell_id,
            }),
            content_items,
        ),
        RuntimeResponse::Result {
            cell_id: _,
            content_items,
            result: _,
            error_text: Some(error_text),
            ..
        } => code_mode_result(
            json!({
                "status": "failed",
                "error": {
                    "message": error_text,
                }
            }),
            content_items,
        ),
        RuntimeResponse::Result {
            content_items,
            result: Some(result),
            error_text: None,
            ..
        } => code_mode_result(
            json!({
                "status": "completed",
                "result": result,
            }),
            content_items,
        ),
        RuntimeResponse::Result {
            content_items,
            result: None,
            error_text: None,
            ..
        } => code_mode_result(
            json!({
                "status": "completed",
            }),
            content_items,
        ),
    }
}

fn running_code_mode_result(
    cell_id: &str,
    content_items: Vec<FunctionCallOutputContentItem>,
) -> BridgeCallOutcome {
    code_mode_result(
        json!({
            "status": "running",
            "cell_id": cell_id,
        }),
        content_items,
    )
}

fn failed_code_mode_result(message: impl Into<String>) -> BridgeCallOutcome {
    BridgeCallOutcome::Success(json!({
        "status": "failed",
        "error": {
            "message": message.into(),
        }
    }))
}

fn code_mode_result(
    value: Value,
    content_items: Vec<FunctionCallOutputContentItem>,
) -> BridgeCallOutcome {
    let content = code_mode_content(content_items);
    if content.is_empty() {
        BridgeCallOutcome::Success(value)
    } else {
        BridgeCallOutcome::SuccessWithContent { value, content }
    }
}

fn code_mode_content(items: Vec<FunctionCallOutputContentItem>) -> Vec<Content> {
    items
        .into_iter()
        .map(|item| match item {
            FunctionCallOutputContentItem::InputImage { image_url, .. } => {
                Content::text(format!("[image] {image_url}"))
            }
        })
        .collect()
}

fn runtime_response_cell_id(response: &RuntimeResponse) -> &str {
    match response {
        RuntimeResponse::Yielded { cell_id, .. }
        | RuntimeResponse::Terminated { cell_id, .. }
        | RuntimeResponse::Result { cell_id, .. } => cell_id,
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn quoted_ts_key(value: &str) -> String {
    serde_json::to_string(value).expect("string key serializes")
}

fn typescript_type_for_datafusion(data_type: &str) -> &'static str {
    let data_type = data_type.to_ascii_lowercase();
    if data_type.contains("int")
        || data_type.contains("float")
        || data_type.contains("decimal")
        || data_type.contains("double")
    {
        "number"
    } else if data_type.contains("bool") {
        "boolean"
    } else if data_type.contains("utf8")
        || data_type.contains("string")
        || data_type.contains("json")
        || data_type.contains("date")
        || data_type.contains("time")
    {
        "string"
    } else {
        "JsonValue"
    }
}
