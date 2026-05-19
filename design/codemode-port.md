# Coral Code Mode Port Plan

## Problem

Coral wants the useful part of Codex Code Mode: a model can orchestrate a small,
finite set of callable functions instead of seeing an unbounded or provider-sized
tool surface. This work stacks on withcoral/coral#459, so the finite set should
match the unified Coral MCP surface: `sql`, `list_catalog`, `search_catalog`,
`describe_table`, `list_columns`, and optional `feedback`.

`feedback` should be callable from Code Mode whenever direct MCP feedback is
enabled. It is already part of the finite MCP surface and is visible to the same
agent. Do not add a stricter nested-only policy unless we find a concrete abuse
or privacy failure mode.

Do not preserve removed `list_tables` / `search_tables` aliases inside Code Mode
unless Coral intentionally adds MCP compatibility aliases first.

The bad implementation would copy Codex Code Mode wholesale into the wrong
layer. Codex needs a V8 runtime because its model writes JavaScript through an
`exec` custom tool. Coral needs the equivalent model-visible `exec`/`wait`
surface, but that surface must be backed only by the finite Coral MCP functions,
not by every provider/source tool Coral can eventually reach.

The implementation should therefore happen in two layers:

- a transport-owned finite function bridge reused by ordinary MCP calls and
  nested Code Mode calls;
- an opt-in MCP `exec`/`wait` projection over that bridge.

The bridge is a prerequisite, not a substitute for `exec`/`wait`.

There are two separate registries:

- advertised MCP tools, which are what `tools/list` returns;
- callable nested functions, which are what a Code Mode runtime can call through
  `tools.<name>(...)`.

Those registries intentionally diverge in code-mode-only mode: `sql` can be
hidden from `tools/list` while still callable inside `exec`.

Confidence: high.

## Current Context

Codex Code Mode is split cleanly:

- `codex-rs/code-mode/src/lib.rs` defines the public `exec` and `wait` names.
- `codex-rs/code-mode/src/description.rs` owns `ToolDefinition`, nested tool
  descriptions, JavaScript identifier normalization, `// @exec:` pragma
  parsing, and JSON-schema-to-TypeScript rendering.
- `codex-rs/code-mode/src/runtime/mod.rs` owns `ExecuteRequest`, `WaitRequest`,
  `RuntimeResponse`, `CodeModeNestedToolCall`, V8 startup, yielded cells, and
  pending runtime events.
- `codex-rs/code-mode/src/service.rs` exposes `CodeModeTurnHost`, so runtime
  nested tool calls go through a host instead of bypassing normal dispatch.
- `codex-rs/core/src/tools/spec_plan.rs` builds two separate things: the
  model-visible tool list and the executable registry. Code-mode-only tests pin
  that the model-visible list can be exactly `exec` and `wait`.
- `codex-rs/core/src/tools/code_mode/mod.rs` routes nested tool calls back
  through the normal tool runtime with `ToolCallSource::CodeMode`.

Coral's post-#459 finite function surface is MCP-shaped:

- `crates/coral-mcp/src/lib.rs` documents the exposed MCP tools and resources.
- `crates/coral-mcp/src/surface/tools.rs` defines MCP tool schemas,
  descriptions, annotations, and argument parsing.
- `crates/coral-mcp/src/server.rs` dispatches each MCP tool. `sql` calls
  `QueryService.ExecuteSql`; catalog discovery tools call the app-owned catalog
  service; `feedback` calls `FeedbackService`.
- `crates/coral-mcp/src/surface/catalog.rs` and `surface/values.rs` render
  catalog service responses into tool-oriented JSON.
- `crates/coral-api/proto/coral/v1/catalog.proto` exposes
  `CatalogService.ListCatalog`, `SearchCatalog`, `DescribeTable`, and
  `ListColumns`.
- `crates/coral-api/proto/coral/v1/query.proto` owns query execution, not
  catalog discovery.
- `crates/coral-client/src/lib.rs` is intentionally thin and should not become
  a rich SDK.
- `crates/coral-app/src/query/manager.rs` owns workspace-scoped source loading
  before engine execution. It does not own agent-facing function ergonomics.
- `crates/coral-app/src/catalog/discovery.rs` owns provider-independent catalog
  matching, kind filtering, ordering, pagination, exact table lookup, column
  filtering, table-function metadata collection, and missing-table context.
- `crates/coral-engine` owns query execution, runtime-visible catalog metadata,
  and source table functions, but not MCP discovery ergonomics.

The important distinction: post-#459 MCP functions are not all engine functions.
`list_catalog`, `search_catalog`, `describe_table`, and `list_columns` are
agent-facing discovery functions. Their behavior is app-owned catalog behavior
projected through MCP, not MCP-owned matching over raw query metadata.

## Stacking Baseline

This work stacks on withcoral/coral#459 (`feat(catalog)!: expose unified catalog
discovery`), which itself stacks on withcoral/coral#448
(`feat(app): add catalog discovery service`). Assume both have landed in the
base branch for this design.

Implementation is not ready to start against a checkout that only has the
pre-#448/#459 MCP shape. If the base branch does not contain `CatalogService`,
`list_catalog`, `search_catalog`, and app-owned catalog discovery, stop and
rebase onto the prerequisite stack before implementing this plan. Seeing
`list_tables` / `search_tables` in the current worktree is a baseline mismatch,
not a signal to preserve those names in Code Mode.

- `coral-app` owns catalog discovery behavior and serves it through
  `CatalogService`;
- `coral-api` has a dedicated `catalog.proto` contract;
- `coral-mcp` is thinner and mostly parses arguments, calls `CatalogClient`, and
  renders tool-shaped JSON;
- `list_catalog` and `search_catalog` are the discovery functions, not
  `list_tables` and `search_tables`;
- table functions appear as catalog item metadata, but they are not separate
  Code Mode-callable provider tools;
- `describe_table` and `list_columns` remain table-specific;
- `feedback` is optional direct MCP surface and should be part of the nested
  Code Mode callable set when enabled;
- Code Mode should expose the same finite MCP surface and should not resurrect
  removed discovery tools as hidden aliases.

## External Context

Cloudflare's Code Mode writeup
(`https://blog.cloudflare.com/code-mode/`) supports the narrow design, not a
provider-tool explosion. Their useful claim is that MCP tools can be converted
into a typed TypeScript API, then invoked from sandboxed code. The sandbox is
isolated from the Internet and reaches outside only through bound APIs
representing the MCP servers. For Coral, that maps to a no-ambient-access
JavaScript runtime whose only host bindings are the post-#459 Coral MCP
functions.

The Cloudflare design also pushes against HTTP-proxy-shaped security. The
sandbox should not receive API keys, generic network access, or provider SDKs.
It should receive host-owned bindings that dispatch back through Coral's normal
MCP/app paths. That is exactly why `exec` should call a finite bridge instead of
growing a new provider API surface.

The Executor repo (`/home/james/src/RhysSullivan/executor`) points to the same
boundary, but with a different product goal. Executor wants one tool catalog
across OpenAPI, GraphQL, MCP, and other sources. Coral should not import that
goal for this feature. The parts worth stealing are:

- split the generic execution contract from the sandbox runtime;
- isolate heavyweight runtime dependencies so default builds do not carry a
  sandbox they are not using;
- pass a `SandboxToolInvoker`/host trait into the runtime so sandbox calls
  cannot bypass normal dispatch;
- distinguish successful domain values from execution failures. For Coral, an
  empty SQL result, a missing catalog table recovery object, or a no-match
  search page is a value; invalid arguments, infrastructure failures, and
  structured tool errors should reject the nested JavaScript promise;
- support pause/resume when tools need user input, approval, or another
  interaction that cannot complete in the first `exec` call;
- build the `exec` description dynamically from the actual enabled function
  catalog.

Codex did not always use V8 for Code Mode. Git history shows an earlier
experimental implementation under `codex-rs/core/src/tools/code_mode` that
spawned Node with `--experimental-vm-modules`, used `worker_threads`, and ran
code through `node:vm` (`runner.cjs`, `worker.rs`, `process.rs`, and
`protocol.rs`). The March 2026 V8 migration deleted that process/Node runner and
added the dedicated `codex-code-mode` crate with a direct `v8` dependency:

- `1746126881` (`Extract code_mode runtime into dedicated crate`) first pulled
  the runtime into a dedicated crate while still carrying the old bridge shape;
- `a8e4ae7612` (`Code Mode => New Crate + v8`) added the V8-backed crate and
  deleted the old process runner on the feature branch;
- `e4eedd6170` (`Code mode on v8 (#15276)`) landed the V8-backed Code Mode
  implementation;
- `36460387ec` (`Enable V8 sandboxing for source-built builds (#21146)`) later
  hardened source-built V8 sandboxing.

For Coral, this means "copy Codex" means copy the current V8-backed design, not
the earlier Node/process runner. The migration history is still useful because
it proves V8 was chosen after an initial subprocess prototype, not by accident.

## Resolved Decisions

- First exposure is MCP-only. Do not add app/API Code Mode service methods until
  a non-MCP caller exists.
- First runtime is Codex-style V8. Copy the current `codex-code-mode` runtime
  shape closely instead of designing around QuickJS, Deno, WASM, or a generic
  executor abstraction first.
- First MCP Code Mode exposure ships `exec` and `wait` together. Do not ship an
  `exec`-only MCP variant for this stack.
- V8 support ships behind an explicit feature/build variant for now. Default
  workspace checks and default Coral binaries should not pull in heavyweight V8
  dependencies accidentally.
- `sql` results inside Code Mode are not capped differently from direct MCP
  `sql` results. It is the same model receiving the output, and parity is more
  important than speculative nested-only truncation. If Coral later adds a
  general MCP SQL cap, Code Mode should inherit it through the bridge.
- This first stack enforces the cell-level budget that protects the finite
  bridge directly: max nested bridge calls per `exec` cell. Broader runtime
  policy budgets such as pending-cell caps, wall-clock TTL, heap limits, stored
  value byte caps, emitted content byte caps, and feedback-specific quotas
  remain follow-up hardening. Those future budgets should still apply around the
  whole cell rather than making one nested SQL call behave differently from one
  direct MCP SQL call.
- Bound SQL parameters are in scope for this stack. Direct MCP `sql`, Code Mode
  `tools.sql({ sql, params })`, and the Code Mode tagged-template overload
  should all route through one bridge function and one `ExecuteSql` transport
  contract.
- `feedback` is callable from Code Mode whenever direct MCP feedback is enabled.
  It uses the same bridge and provenance machinery as every other nested
  function.
- Code Mode JavaScript should expose ergonomic domain return values, not raw MCP
  `CallToolResult` envelopes. Direct MCP keeps the MCP envelope; Code Mode gets
  the successful structured value directly and receives structured failures as
  rejected promises.
- `exec` should capture the JavaScript entrypoint's final JSON-serializable
  return value and expose it as the MCP tool's structured output. Coral should
  not expose Codex-style `text(...)` or `notify(...)` output helpers in the
  MCP-first contract.
- SQL is the primary data-access and relational transformation primitive inside
  Code Mode. JavaScript is for orchestration, dynamic query construction,
  schema/catalog walking, nested JSON traversal, and small final reshaping; it
  should not encourage row-by-row tool loops when one SQL query can do the work.
- DataFusion advantages should be visible in the Code Mode guidance: read-only
  SQL enforcement, Arrow schema metadata, `information_schema`, registered JSON
  SQL functions, and source-aware `LIMIT` pushdown. Do not hide those behind a
  generic JavaScript data API.

## Technical Plan

### 1. Factor The Existing MCP Tool Dispatcher

Add a small transport-local finite function bridge inside `crates/coral-mcp`,
probably under `src/surface/functions.rs` or `src/surface/bridge.rs`.

The bridge should expose:

- a typed function enum for the post-#459 surface:
  `Sql`, `ListCatalog`, `SearchCatalog`, `DescribeTable`, `ListColumns`,
  `Feedback`;
- one canonical list of enabled function names, descriptions, input schemas,
  output schemas, optional TypeScript declaration overrides, and annotations;
- a render context for dynamic descriptions, including at least visible source
  names, visible catalog counts, feedback enablement, and whether the target is
  direct MCP or Code Mode documentation;
- one dispatch path that takes a function name plus JSON arguments and returns
  the same structured value MCP tools return today;
- one source/provenance field, even if the first version only records
  `McpDirect` and `CodeModeNested { cell_id, runtime_tool_call_id }`.

The `Sql` bridge function should accept `params` in addition to `sql`:

- positional params are a JSON array and bind to DataFusion placeholders `$1`,
  `$2`, and so on;
- named params are a JSON object and bind to DataFusion placeholders such as
  `$zone_id`;
- supported parameter values are `null`, boolean, finite number, and string;
- integral JSON numbers that fit `i64` become int64 params, and other finite
  JSON numbers become float64 params;
- arrays and objects are rejected as unsupported parameter values in the first
  version. Use JSON strings plus SQL JSON functions when a query needs JSON
  structure.

The engine mapping should be explicit and DataFusion-native:

- arrays become `ParamValues::List`;
- objects become `ParamValues::Map` with names stored without the leading `$`;
- JSON `null` maps to `ScalarValue::Null`;
- booleans map to `ScalarValue::Boolean(Some(value))`;
- integral numbers that fit `i64` map to `ScalarValue::Int64(Some(value))`;
- other finite numbers map to `ScalarValue::Float64(Some(value))`;
- strings map to `ScalarValue::Utf8(Some(value))`;
- after `SessionContext::sql_with_options(..., read_only_sql_options())`
  returns a `DataFrame`, call `DataFrame::with_param_values(...)` before
  observer notification, collect, or result materialization.

Then make `CoralMcpServer::call_tool` call that bridge instead of owning the
`match request.name` directly.

This is the highest-leverage Codex lesson: do not let model/tool visibility and
runtime dispatch diverge.

This bridge is adapter machinery, not a new discovery owner. Dispatch for
`list_catalog`, `search_catalog`, `describe_table`, and `list_columns` should
call `CatalogClient` and reuse `surface/catalog.rs` and `surface/values.rs`
renderers. Do not move regex matching, pagination, exact lookup, column
filtering, function metadata collection, or missing-table suggestion logic back
into `coral-mcp`.

The bridge should require an output schema for every finite function. Do not
preserve the post-#459 asymmetry where only `list_catalog`, `search_catalog`,
and `list_columns` advertise output schemas. If we are allowed to choose the
cleaner contract, direct MCP and Code Mode should share the same schema source
for all functions.

The bridge must not enumerate provider/source tools. It should expose exactly
the finite Coral MCP-equivalent functions listed above. It may keep `Feedback`
as a bridge variant when `feedback_enabled` is true, and the nested Code Mode
callable registry should include it under the same enablement condition.

Use explicit internal result types so direct MCP and nested Code Mode can share
dispatch without losing error semantics:

```rust
enum FunctionCallSource {
    McpDirect,
    CodeModeNested { cell_id: String, runtime_tool_call_id: String },
}

struct FunctionCall {
    name: CoralFunction,
    arguments: serde_json::Map<String, serde_json::Value>,
    source: FunctionCallSource,
}

enum FunctionCallFailure {
    InvalidArguments(rmcp::ErrorData),
    Infrastructure(tonic::Status),
}

struct FunctionCallResult {
    structured_content: serde_json::Value,
    content: Vec<rmcp::model::Content>,
    is_error: bool,
}
```

Direct MCP maps `InvalidArguments` to protocol errors, `Infrastructure` to the
existing structured tool error result, and `FunctionCallResult` to
`CallToolResult`. Nested Code Mode maps `InvalidArguments`, infrastructure
failures, and `FunctionCallResult { is_error: true }` to rejected JavaScript
promises with structured error fields. Successful nested calls unwrap
`structured_content` into the JavaScript return value.

### 2. Add Code Mode As The MCP `exec`/`wait` Extension

Add a new `crates/coral-code-mode` crate by porting the generic parts of
`codex-code-mode`. Use Codex's current V8-backed runtime as the implementation
baseline.

Keep it free of Codex protocol types. Coral should define its own:

- `ToolDefinition { name, description, input_schema, output_schema, kind }`
- `ExecuteRequest { cell_id, tool_call_id, enabled_tools, source,
  stored_values, yield_time_ms, max_output_tokens }`
- `WaitRequest`
- `RuntimeResponse`
- `CodeModeNestedToolCall`
- `CodeModeHost` with `invoke_tool(...) -> Result<FunctionCallResult,
  FunctionCallFailure>`

For the first cut, copy the Codex runtime shape closely but trim unsupported
features instead of generalizing prematurely:

- keep `tools.<name>(args)`, `ALL_TOOLS`, `text`, `image`, `store`, `load`,
  `yield_control`, and `exit`;
- keep `wait`/yield semantics and implement the MCP `wait` projection;
- skip Codex freeform custom-tool grammar. Keep the small Codex-compatible
  `// @exec:` pragma parser for `yield_time_ms` and `max_output_tokens` because
  it stays inside the `source` string and does not widen the MCP tool surface.

Do not put V8 directly in `coral-mcp`, `coral-app`, or `coral-client`.

Because the workspace includes `crates/coral-*`, a new `crates/coral-code-mode`
crate would be built by default during workspace checks. If the crate is added
under that glob, it must be default-light: no V8 or other heavyweight sandbox
dependency unless an explicit feature is enabled. Otherwise place the
experimental runtime outside the glob until we are willing to pay that build
cost in normal Coral development.

Executor's QuickJS package is a good warning about dependency blast radius, not
a reason to build a multi-runtime abstraction first. Coral should keep the
V8-heavy dependency behind a feature and keep the host/runtime boundary narrow,
but the first implementation should be a V8 port rather than a pluggable runtime
framework.

### 3. Project Code Mode Through MCP

Add an opt-in MCP option, for example:

- `coral mcp-stdio --enable-code-mode`
- optionally later `--code-mode-only` if we want Codex's model-visible
  `exec`/`wait`-only behavior.

Mode behavior should be explicit:

| Mode | `tools/list` advertises | Direct `tools/call` for finite functions | Runtime `ALL_TOOLS` |
| --- | --- | --- | --- |
| default | `sql`, `list_catalog`, `search_catalog`, `describe_table`, `list_columns`, optional `feedback` | allowed | not available |
| `--enable-code-mode` in a `code-mode` build | direct tools plus `exec`, `wait` | allowed | `sql`, `list_catalog`, `search_catalog`, `describe_table`, `list_columns`, optional `feedback` |
| `--enable-code-mode --code-mode-only` in a `code-mode` build | `exec`, `wait` only | rejected for hidden finite functions | `sql`, `list_catalog`, `search_catalog`, `describe_table`, `list_columns`, optional `feedback` |

The first implementation target for this stack is full `exec`/`wait`. Do not
ship an MCP Code Mode variant that advertises `exec` without `wait` unless this
artifact and its acceptance criteria are updated first. `wait` is part of the
requested contract, not a later compatibility garnish.

MCP does not have Codex's OpenAI freeform custom-tool input shape. The MCP
projection should therefore use JSON-object tool schemas:

- `exec` input:
  - required `source: string`
  - optional `yield_time_ms: integer`
  - optional `max_output_tokens: integer`
- `wait` input:
  - required `cell_id: string`
  - optional `yield_time_ms: integer`
  - optional `terminate: boolean`

Normalize `source` in a Cloudflare-compatible way: if the trimmed source parses
as an async/sync function expression or arrow function expression, invoke it and
use its return value. Otherwise treat `source` as the body of an implicit async
function and allow normal `return` statements. Both forms should be able to
return any JSON-serializable value.

When enabled, `tools/list` should include `exec` and `wait`. The `exec`
description should render the finite Coral function list as TypeScript-like
samples using the same schemas as the bridge from step 1. Nested calls should
call the bridge, not reconstruct MCP JSON-RPC requests.

`ALL_TOOLS` inside the runtime should contain only these callable functions:
`sql`, `list_catalog`, `search_catalog`, `describe_table`, `list_columns`, and
optional `feedback` when direct MCP feedback is enabled. `ALL_TOOLS` should not
contain source tables, provider API operations, OpenAPI methods, GraphQL fields,
Slack actions, Jira actions, or any other backend-specific functions.

Table functions may appear in the structured results returned by
`list_catalog` and `search_catalog`, but they must not become direct
`tools.<provider>.<function>(...)` calls.

The initial model-visible strategy should be conservative:

- default: expose existing direct tools plus optional `exec`/`wait`;
- code-mode-only: advertise only `exec`/`wait`, while the target branch's finite
  nested functions remain callable from JavaScript.

Do not copy Executor's MCP naming of `execute`/`resume` unless there is a client
compatibility reason. Codex compatibility and the user's target shape point to
`exec`/`wait`.

### 4. Generate TypeScript Tool Types From The Bridge

Code Mode does not run TypeScript. The types are model-facing declarations in
the `exec` description, copied from Codex's current JSON-Schema-to-TypeScript
approach. They should make the JavaScript API obvious without becoming a second
contract.

The source of truth is the finite function bridge:

```rust
struct FunctionSpec {
    name: CoralFunction,
    description: String,
    input_schema: serde_json::Value,
    output_schema: serde_json::Value,
    typescript_declaration: Option<String>,
    annotations: FunctionAnnotations,
}
```

Direct MCP and Code Mode both use `input_schema` and `output_schema`. Direct MCP
wraps the value in `CallToolResult`; Code Mode unwraps the successful structured
value for the JavaScript API. Code Mode normally renders TypeScript from the
schemas. `typescript_declaration` exists only for cases where JSON Schema cannot
express the best model-facing TypeScript shape.

`sql` is that case. Coral knows source table schemas, so Code Mode should expose
those schemas as model-facing authoring hints. But the exact row type of an
arbitrary SQL query is not the same thing as the input table type: a query can
alias, cast, aggregate, join, outer join, `UNION`, select literals, call JSON
functions, or build `CASE` expressions. Do not pretend TypeScript can infer
that from a SQL string.

The honest design has two typed surfaces:

- generated table schema declarations for known catalog tables, used as context
  while writing SQL;
- DataFusion/Arrow output schema metadata for each concrete SQL result, returned
  at runtime in `columns`.

The `rows` type should remain generic by default. Models can narrow it when they
know what they selected, but Coral's runtime guarantee is the `columns`
metadata plus JSON-compatible row values.

Generate declarations like this:

```ts
type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };
type SqlValue = JsonValue;
type SqlRow = Record<string, SqlValue>;
type SqlType =
  | { kind: "null" }
  | { kind: "boolean" }
  | { kind: "integer"; signed: boolean; bit_width: 8 | 16 | 32 | 64 }
  | { kind: "float"; bit_width: 16 | 32 | 64 }
  | { kind: "decimal"; precision: number; scale: number }
  | { kind: "string" }
  | { kind: "binary" }
  | { kind: "date"; unit: "day" | "millisecond" }
  | {
      kind: "time";
      unit: "second" | "millisecond" | "microsecond" | "nanosecond";
    }
  | {
      kind: "timestamp";
      unit: "second" | "millisecond" | "microsecond" | "nanosecond";
      timezone?: string;
    }
  | { kind: "list"; item: SqlType }
  | { kind: "struct"; fields: SqlColumn[] }
  | { kind: "unknown"; data_type: string };
type SqlColumn<
  TName extends string = string,
  TType extends SqlType = SqlType,
> = {
  name: TName;
  data_type: TType;
  nullable: boolean;
};
type SqlParamValue = null | boolean | number | string;
type SqlParams = SqlParamValue[] | Record<string, SqlParamValue>;
type ParameterizedSqlInput = {
  sql: string;
  params?: SqlParams;
};
type SqlInput = string | ParameterizedSqlInput;
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
  summary: string;
  detail: string;
  hint?: string;
  grpc_code: string;
  reason?: string;
  retryable: boolean;
  metadata: Record<string, unknown>;
};

declare namespace CoralSchema {
  interface Tables {
    // Generated from the visible catalog when the schema budget allows it.
    // Example:
    // "github.issues": {
    //   number: number | null;
    //   title: string | null;
    //   created_at: string | null;
    // };
  }
}
type TableName = keyof CoralSchema.Tables;
type TableRow<TTable extends TableName> = CoralSchema.Tables[TTable];

declare const tools: {
  sql: SqlFunction;
  list_catalog(args?: ListCatalogArgs): Promise<ListCatalogResult>;
  search_catalog(args: SearchCatalogArgs): Promise<SearchCatalogResult>;
  describe_table(args: DescribeTableArgs): Promise<DescribeTableResult>;
  list_columns(args: ListColumnsArgs): Promise<ListColumnsResult>;
  feedback(args: FeedbackArgs): Promise<FeedbackResult>;
};
```

Models can optionally narrow simple queries when they know what they selected:

```ts
const result = await tools.sql<{ n: number }>("SELECT 1 AS n");
return result.rows[0]?.n;
```

That is a model-authored assertion, not a runtime guarantee. Runtime guarantees
come from the returned `columns` metadata and the JSON values in `rows`. For
unknown queries, the default `SqlRow` type is honest and dynamic.

Known table schemas improve the prompt without changing the runtime contract:

```ts
type IssueRow = TableRow<"github.issues">;

const { rows } = await tools.sql<Pick<IssueRow, "number" | "title">>(`
  SELECT number, title
  FROM github.issues
  WHERE state = 'open'
  LIMIT 50
`);
```

Generated table declarations are authoring hints, not a SQL type checker. For a
large catalog, the `exec` description should include only the schemas that fit
the configured description budget and tell the model to call `describe_table`
or `list_columns` for exact table details. Do not add an `InferSql<"...">`
TypeScript parser or a parallel planner in Code Mode.

The initial `CoralSchema.Tables` value mapping should follow the JSON rows Coral
actually returns through `coral-client::batches_to_json_rows`, not idealized SQL
types:

- manifest `Boolean` becomes `boolean`;
- manifest `Int64` and `Float64` become `number`;
- manifest `Utf8`, `Json`, and `Timestamp` become `string`;
- nullable columns add `| null`;
- unknown or complex Arrow/DataFusion types become `SqlValue` until the row JSON
  renderer has an explicit tested mapping for them.

Do not expose `bigint` in the model-facing TypeScript unless Coral changes the
JSON row renderer to preserve integers outside JavaScript's safe numeric range.

Only include `feedback` in the generated declaration when feedback is enabled.
Only include `wait`/`exec` as top-level MCP tools, not as nested `tools.*`
functions.

The JSON-Schema-to-TypeScript renderer should be small and copied from Codex's
current renderer where possible. It must support the schema constructs Coral
already uses: `object`, `array`, `required`, `additionalProperties`,
`enum`/`const`, `anyOf`/`oneOf`, `allOf`, nullable unions, strings, numbers,
integers, booleans, and `null`. Unsupported schema fragments should render as
`unknown`, not panic or hallucinate a narrower type.

For the first implementation, define structured output schemas for the complete
finite set:

- `sql`: `{ columns: SqlColumn[], rows: SqlRow[], row_count: number }`, with
  `columns` derived from the concrete DataFusion/Arrow output schema;
- `list_catalog`: existing catalog page schema;
- `search_catalog`: existing catalog search page schema;
- `describe_table`: found-table or missing-table recovery object;
- `list_columns`: column page or missing-table recovery object;
- `feedback`: `{ feedback_id: string, created_at: string, message: string }`.

These schemas are not optional niceties. If the model cannot see the shape of
`structuredContent`, it will treat Code Mode as stringly JavaScript and lose the
main benefit of this feature. The SQL schema should be dynamic-row-safe, not
column-name-precise.

Tests should snapshot the generated TypeScript declarations and separately
validate live structured outputs against the same schemas. That catches both
type drift and renderer drift.

### 5. Make SQL The Native Data Idiom

The most idiomatic Coral Code Mode examples should look like database code plus
small JavaScript transformations, not like direct provider API scripting.

Prefer this shape:

```js
const needle = "%rulesets%";
const { rows, columns, row_count } = await tools.sql(`
  SELECT method, path, summary
  FROM cloudflare.openapi_operations
  WHERE path LIKE '/zones/%'
    AND (path LIKE '%firewall/waf%' OR path LIKE ${needle})
  ORDER BY path, method
`);

return { columns, row_count, operations: rows };
```

Use JavaScript when SQL becomes the wrong language for the last step:

```js
const { rows } = await tools.sql("SELECT spec FROM cloudflare.openapi LIMIT 1");
const spec = rows[0]?.spec;

return Object.entries(spec.paths).flatMap(([path, methods]) => {
  if (
    !path.includes("/zones/") ||
    !(path.includes("firewall/waf") || path.includes("rulesets"))
  ) {
    return [];
  }

  return Object.entries(methods).map(([method, op]) => ({
    method: method.toUpperCase(),
    path,
    summary: op.summary,
  }));
});
```

The generated `exec` description should teach these rules:

- use `list_catalog`, `search_catalog`, `describe_table`, and `list_columns` to
  discover tables and columns before writing SQL;
- use `information_schema` when SQL-native filtering or joining over table and
  column metadata is more convenient than separate catalog calls;
- push projection, filtering, joins, ordering, aggregation, and limits into SQL;
- use `SELECT ... LIMIT 0` to inspect result columns without materializing rows;
- use JavaScript for dynamic query construction, branching, nested JSON walking,
  and final object shaping;
- avoid issuing one SQL query per row when a single SQL query or batched query
  can express the work;
- include `LIMIT` while exploring broad tables, even though Code Mode does not
  add a nested-only SQL cap. For Coral sources, `LIMIT` is not just output
  trimming; it can reduce source pagination and upstream API work;
- use registered JSON SQL functions such as `json_get_str`, `json_get_int`,
  `json_get_bool`, `json_get_json`, `json_contains`, `json_length`,
  `json_object_keys`, and `json_as_text` for known JSON paths before falling
  back to JavaScript traversal;
- pass dynamic values through SQL parameters, not string interpolation;
- inspect `columns` when the result shape is uncertain, and use generic
  TypeScript narrowing only as a model-authored assertion.

Do not add separate nested bridge functions like `rows`, `first`, `one`, or
`scalar` in the first version. They look ergonomic, but they widen the apparent
tool surface and hide the schema metadata that makes SQL results inspectable.
If those helpers prove useful later, implement them as pure JavaScript
convenience wrappers around `tools.sql`, not as additional host-callable
functions.

The tagged-template overload is still the same finite host function, `sql`; it
is not a new nested tool. The runtime wrapper translates the template form into
parameterized SQL such as `{ sql: "SELECT ... WHERE id = $1", params: [id] }`
before invoking the bridge.

Do not add a DataFrame-like JavaScript API in the first version. DataFusion is
already the DataFrame engine. A parallel JS `df.filter(...).groupBy(...)` layer
would be a weaker, stringly query planner that cannot beat the optimizer,
registered source providers, Arrow schema handling, or existing SQL diagnostics.

Do not add `explain_sql` to the MCP Code Mode surface in this stack. Coral
already has `ExplainSql` at the gRPC/API layer and DataFusion can produce
logical and physical plans, so it is a strong future candidate if we relax the
"only add exec/wait to MCP" constraint. For this stack, keep the visible MCP
addition to `exec`/`wait` plus the richer `sql` input/output contract.

### 6. Capture Script Return Values As Structured Output

Coral should deliberately diverge from Codex here. Codex's current V8 runtime
uses output functions such as `text(...)` and `notify(...)` as observable result
channels and does not expose a fulfilled module promise value as the tool's
structured result. That is the wrong ergonomic shape for Coral's SQL/catalog use
case.

The JavaScript contract should be:

- `return value` is the primary structured result of `exec`;
- `value` must convert cleanly to JSON: object, array, string, number, boolean,
  or null;
- `undefined` means "no structured return value";
- functions, symbols, cyclic objects, non-finite numbers, and unsupported V8
  host objects reject with a clear structured script error;
- `text(...)` and `notify(...)` are not globals in Coral MCP Code Mode; use
  `return` instead of print-style side effects;
- if the script yields, the final returned value appears only in the terminal
  `wait` result.

This enables the model to use code as a real transformation layer over SQL and
catalog results:

```js
async () => {
  const { rows } = await tools.sql("SELECT spec FROM cloudflare_openapi LIMIT 1");
  const spec = rows[0]?.spec;
  const results = [];

  for (const [path, methods] of Object.entries(spec.paths)) {
    if (
      path.includes("/zones/") &&
      (path.includes("firewall/waf") || path.includes("rulesets"))
    ) {
      for (const [method, op] of Object.entries(methods)) {
        results.push({
          method: method.toUpperCase(),
          path,
          summary: op.summary,
        });
      }
    }
  }

  return results;
}
```

At the JavaScript boundary, the returned value is raw: an array return is an
array, an object return is an object, and a scalar return is that scalar.

At the MCP boundary, `exec` and `wait` should always return a stable object
shape in `structuredContent`:

- terminal success with a JSON return value:
  `{ "status": "completed", "result": <json> }`;
- terminal success with `undefined`:
  `{ "status": "completed" }`;
- yielded/running cell:
  `{ "status": "running", "cell_id": "<id>" }`;
- explicit termination:
  `{ "status": "terminated", "cell_id": "<id>" }`;
- terminal script or nested-tool failure:
  `{ "status": "failed", "error": <structured error> }`.

Do not expose arbitrary raw JSON as the outer MCP `structuredContent` for
`exec`/`wait`. The JavaScript contract stays raw and ergonomic; the MCP
projection stays object-shaped and stable.

### 7. Preserve Domain Semantics And Declare Contract Changes

The bridge should produce an internal result shape with no information loss:

- `structured_content: serde_json::Value`
- `content: Vec<...>` or a simplified text content equivalent
- `is_error: bool`

Direct MCP calls map that result to `CallToolResult`. Nested Code Mode calls
should return the successful structured value directly. That is cleaner than
forcing every script through `.structuredContent`, and it avoids leaking MCP
transport shape into the JavaScript API.

For nested Code Mode calls, bad nested arguments should reject that nested
function's promise. Backend/tool execution failures should also reject the
promise with a structured `CoralToolError`. If the script catches the rejection,
`exec` can continue. If the rejection is uncaught, `exec` fails with the script
error. This is intentionally more JavaScript-native than the raw MCP
`isError`-as-value convention.

Nested Code Mode calls must preserve the same structured content as direct MCP
calls:

- `await tools.sql(sql)` returns `{ columns: [...], rows: [...], row_count: n }`;
- `await tools.list_catalog(...)` returns the same paged discovery object as
  direct MCP structured content, including table-function catalog items;
- `await tools.search_catalog(...)` uses app-owned regex search over catalog
  metadata;
- `await tools.describe_table(...)` still uses `CatalogService.DescribeTable`,
  not `ExecuteSql`;
- `await tools.list_columns(...)` still returns missing-table recovery hints;
- `await tools.feedback(...)` uses the same feedback path as direct MCP when
  feedback is enabled.

Bad arguments should stay protocol/tool-input errors at the outer direct MCP
call boundary. Inside Code Mode, they are JavaScript promise rejections.

Direct MCP `sql` currently returns only `{ rows }` in the pre-stack checkout.
This plan intentionally changes the SQL structured output contract to
`{ columns, rows, row_count }`. That is a breaking MCP result-shape change, but
it is the cleaner design: Code Mode, direct MCP, live parity tests, and
TypeScript declarations should share one schema-rich SQL envelope. Do not hide
this as an implementation detail, and update docs/release notes accordingly
when the MCP surface changes.

The sandbox should have no ambient filesystem, process environment, secret, or
network access. If any runtime exposes `fetch`, it must either be disabled or
bound to an explicit Coral-controlled proxy. The default answer should be no
network, because all legitimate work goes through `tools.<coral_function>`.

### 8. Enforce The First Code Mode Budget

Code Mode must not rely on "same agent receives the output" as the only safety
control. Individual `tools.sql(...)` calls should inherit the same result
behavior as direct MCP SQL, but an `exec` cell can loop and fan out nested
calls. For this stack, enforce a max nested bridge-call count per `exec` cell.

Budget exhaustion should cancel the cell's ability to continue making bridge
calls and return a structured script error. This is a Code Mode execution
budget, not a nested-only SQL row cap. A single direct MCP SQL call and a single
nested `tools.sql(...)` call should remain equivalent; a script that calls SQL
1,000 times should fail because the cell exceeded its call budget.

Leave broader policy budgets to a follow-up hardening stack after this port is
landed:

- max concurrent nested bridge calls per cell;
- max yielded/running cells per MCP session;
- wall-clock timeout/TTL per cell, separate from `yield_time_ms`;
- V8 heap limit and runtime memory limit owned by the Code Mode feature;
- aggregate `content` plus structured-result byte limit across `exec` and all
  `wait` calls for the cell;
- max stored-value bytes for `store(...)` / `load(...)`;
- feedback call budget, defaulting to one feedback submission per cell and
  still counting against the nested bridge-call budget.

### 9. Own The `wait` Cell Lifecycle In MCP

If `wait` is exposed, `crates/coral-mcp` owns the in-memory cell store for MCP
stdio sessions. The first implementation should be deliberately local and
non-durable:

- cells are scoped to one `CoralMcpServer` instance;
- `cell_id` is an unguessable runtime-generated id;
- cells are removed after completion, explicit termination, or server shutdown;
- process restart loses cells and `wait` returns a structured not-found error;
- `wait { terminate: true }` cancels the runtime task, releases stored values,
  and returns a terminal structured result;
- concurrent `wait` calls for the same cell must be serialized or one must
  receive a deterministic already-waiting error;
- every yielded cell carries the finite function set it was allowed to call.

Add fixed TTL, maximum pending-cell count, and last-wait bookkeeping with the
follow-up budget hardening stack.

This is not an app-level service yet. Do not add `ListCells`/`CallFunction`
gRPC surface unless a second non-MCP consumer appears.

### 10. Render Dynamic Specs Predictably

The bridge should support two rendering modes:

- live render: used by direct `tools/list` and `exec` description generation,
  loading sources and catalog counts through existing clients;
- static render: used by nested runtime `ALL_TOOLS`, tests, and fallback paths
  when dynamic counts are unavailable.

If live render fails, `tools/list` should keep today's behavior: return the
protocol error rather than inventing stale counts. For `exec`, prefer failing
tool registration/listing over advertising a broken Code Mode description. The
nested callable registry itself must not depend on live counts; it should be
constructible from the canonical static specs plus enablement policy.

### 11. Wire Provenance And Telemetry

Keep `finish_tool_call` behavior for direct MCP calls. Nested Code Mode calls
should reuse the same result conversion logic but run under distinct spans:

- direct calls: existing `mcp.tool.name = <tool>`;
- `exec`: `mcp.tool.name = exec`, `mcp.code_mode.enabled = true`;
- nested calls: `mcp.tool.name = <nested_tool>`,
  `mcp.tool.source = code_mode_nested`, `mcp.code_mode.cell_id`,
  `mcp.code_mode.runtime_tool_call_id`.

Do not annotate provider-specific tool ids because Code Mode must not call them.
Use correlation ids for infrastructure defects so host details do not leak into
the sandbox.

## Alternatives

### Copy Codex Code Mode Into `coral-mcp`

Rejected. It couples V8 lifecycle, yielded cells, and runtime globals directly
to an MCP adapter. It also makes the ordinary MCP tool path and Code Mode nested
path drift.

### Put The Bridge In `coral-engine`

Rejected for the post-#459 MCP-equivalent function set. `sql` execution is
engine owned, but catalog discovery, `describe_table`, and `list_columns` are
app-owned catalog behavior. Engine should not learn MCP ergonomics or app
catalog presentation.

### Put The Bridge In `coral-client`

Rejected. `coral-client` explicitly stays narrow: endpoint dialing plus Arrow
decode/render helpers. A richer SDK is the wrong direction.

### Add `ListFunctions`/`CallFunction` gRPC First

Rejected for the first implementation. That becomes correct only if Coral wants
the same finite function bridge outside MCP. For an MCP Code Mode port, it adds
wire surface before there is a second consumer.

### Add A JavaScript DataFrame API Over SQL

Rejected. DataFusion is already the query planner and DataFrame engine. A
JavaScript DataFrame facade would duplicate projection, filtering, grouping,
ordering, and expression handling in a less capable layer. The correct ergonomic
shape is SQL for relational work and JavaScript for orchestration and final
reshaping.

### Add `explain_sql` To MCP Code Mode Immediately

Rejected for this stack. `ExplainSql` is already available in the Coral API and
is a good future finite function because it is DataFusion-native, provider
independent, and useful for debugging generated SQL without executing it.
However, the current product constraint is to add Code Mode as an MCP
`exec`/`wait` projection over the existing finite surface, not to add another
top-level MCP tool. Revisit after `exec`/`wait` and parameterized `sql` land.

### Expose Code Mode Through App/API First

Rejected. First exposure is MCP-only. The bridge can be reused later by an
app/API service, but adding that surface now would force lifecycle, auth, and
compatibility decisions before there is a caller.

### Treat Source Table Functions As The Code Mode Function Set

Rejected for the immediate request. Coral source table functions are useful SQL
constructs and are discoverable as catalog metadata, but they are not the direct
Code Mode function set. They should be invoked through SQL, not promoted into
`tools.<provider>.<function>(...)`.

### Expose All Provider Tools Through Code Mode

Rejected. This is Executor's product direction, not Coral's requested feature.
Cloudflare's Code Mode can handle many tools, but Coral's immediate goal is
different: use code as the orchestration language over the small MCP-equivalent
surface we already trust and document. Provider APIs remain behind `sql` and
catalog discovery metadata unless a later design deliberately expands the
callable catalog.

Table functions being discoverable through `list_catalog` / `search_catalog`
does not change this rejection. Metadata discovery is not the same thing as
making every provider operation directly callable from Code Mode.

## Detailed Implementation

Expected first PR, SQL parameters plus the bridge foundation for `exec`/`wait`:

- `crates/coral-api/proto/coral/v1/query.proto`
  - Add SQL parameter messages to `ExecuteSqlRequest`.
  - Model params as a `oneof` positional list or named map so callers cannot
    send both forms in one request.
  - Support positional parameters for `$1`, `$2`, and named parameters for
    `$name`.
  - Represent parameter values explicitly as null, bool, int64, float64, or
    string so JSON numbers do not become stringly transport values.
- `crates/coral-engine/src/runtime/query.rs`
  - Thread optional parameter values through query execution.
  - Convert Coral SQL parameters into DataFusion `ParamValues`.
  - Map positional params to `ParamValues::List` and named params to
    `ParamValues::Map`, with named keys stored without the leading `$`.
  - Map Coral parameter values to `ScalarValue::Null`,
    `ScalarValue::Boolean(Some(_))`, `ScalarValue::Int64(Some(_))`,
    `ScalarValue::Float64(Some(_))`, and `ScalarValue::Utf8(Some(_))`.
  - Apply parameters with `DataFrame::with_param_values(...)` before collect,
    observer notification, and result materialization.
- `crates/coral-app/src/query/service.rs`
  - Convert the new proto parameter values into the engine query parameter
    representation and preserve current read-only SQL options.
- `crates/coral-mcp/src/surface/functions.rs`
  - Add `CoralFunction`, `FunctionSpec`, `FunctionInvocation`,
    `FunctionCallSource`, and output/error types.
  - Add `FunctionRenderContext` so dynamic descriptions keep current source and
    catalog-count text.
  - Add `FunctionSpec::output_schema` for every finite function.
  - Add `FunctionSpec::typescript_declaration` for special cases like generic
    `sql<TRow>()`.
  - Advertise output schemas consistently through direct MCP and use the same
    schemas for Code Mode type rendering.
  - Parse direct MCP `sql` inputs as either `{ "sql": "..." }` or
    `{ "sql": "...", "params": [...] | { ... } }`.
  - Move existing argument parsing and dispatchable structured result
    construction behind this bridge.
  - Route catalog functions through `CatalogClient` using `list_catalog` /
    `search_catalog`.
- `crates/coral-mcp/src/surface/tools.rs`
  - Keep RMCP `Tool` conversion here.
  - Generate tool definitions from the bridge specs where possible.
- `crates/coral-mcp/src/surface/code_mode_types.rs` or
  `crates/coral-code-mode/src/description.rs`
  - Copy/adapt Codex's JSON-Schema-to-TypeScript renderer.
  - Render ergonomic domain-value declarations for every enabled nested
    function.
  - Render `SqlInput` and
    the `SqlFunction` overloads as special declarations.
  - Render a stable structured `SqlType` declaration and convert
    DataFusion/Arrow field types into that shape for SQL result columns.
  - Render compact `CoralSchema.Tables` declarations from visible catalog table
    schemas when they fit the configured description budget.
  - Map manifest column types into TypeScript value hints using the same JSON
    value semantics as `coral-client::batches_to_json_rows`: booleans to
    `boolean`, numeric manifest types to `number`, `Utf8` / `Json` /
    `Timestamp` to `string`, nullable columns to `| null`, and unknown complex
    types to `SqlValue`.
  - Omit or truncate generated table declarations when the catalog is too large,
    and point models to `describe_table` / `list_columns` for exact schemas.
  - Do not implement a TypeScript SQL parser or an `InferSql<"...">` layer.
  - Render SQL-first Code Mode guidance: use catalog tools for discovery, SQL
    for relational work, and JavaScript for dynamic orchestration and final
    reshaping.
  - Snapshot the generated declarations for default and feedback-enabled
    function sets.
- `crates/coral-mcp/src/surface/catalog.rs` and `surface/values.rs`
  - Reuse existing post-#459 catalog result renderers from the function bridge.
- `crates/coral-mcp/src/server.rs`
  - Replace direct `match request.name` dispatch with bridge dispatch.
  - Keep separate advertised-tool and nested-callable-function sets for
    code-mode-only.
  - Keep protocol spans and `finish_tool_call` behavior.
- `crates/coral-mcp/src/tests.rs`
  - Add parity tests asserting the bridge exposes the same finite names and
    returns the same structured content as direct MCP calls.
- `crates/coral-cli/tests/mcp.rs`
  - Keep the raw MCP schema and routing assertions green.

Expected second PR with JavaScript Code Mode:

- `crates/coral-code-mode`
  - Port the current Codex V8 runtime shape, parser, description builder, and
    service host trait.
  - Keep heavyweight V8 dependencies isolated here and feature-gated.
  - Do not design a QuickJS/Deno/WASM runtime abstraction in the first pass.
  - Make the runtime default-deny: no ambient network, filesystem, environment,
    or secret access.
  - Implement the `tools.sql` JavaScript wrapper with both object/string input
    and tagged-template input. The template form must generate `$1`, `$2`, ...
    placeholders and pass values through `params`, never concatenate parameter
    values into SQL text.
- workspace `Cargo.toml`
  - Add `coral-code-mode` and optional feature wiring so default builds do not
    compile V8.
- `crates/coral-mcp/src/lib.rs`
  - Add `McpOptions { code_mode_enabled, code_mode_only, ... }`.
- `crates/coral-cli/src/lib.rs`
  - Add `mcp-stdio --enable-code-mode` and possibly `--code-mode-only`.
- `crates/coral-mcp/src/server.rs`
  - Add `exec`/`wait` tools when enabled.
  - Host nested runtime calls by invoking the finite function bridge.
  - Map successful nested calls to unwrapped structured values.
  - Wrap outer MCP `exec`/`wait` structured output in the stable
    `{ status, result?, cell_id?, error? }` object shape.
  - Map nested tool errors to structured JavaScript promise rejections.
  - Assert no provider/source tools are added to the nested callable catalog.
  - Include `feedback` in the nested callable catalog when direct MCP feedback
    is enabled.
  - Assert removed `list_tables` / `search_tables` names are not callable
    through Code Mode.
- `docs/guides/use-coral-over-mcp.mdx` and
  `docs/reference/cli-reference.mdx`
  - Update only when the CLI/MCP surface changes.

## Acceptance Criteria

For SQL parameters and the bridge refactor:

- `cargo test -p coral-engine`
- `cargo test -p coral-app`
- `cargo test -p coral-mcp`
- `cargo test -p coral-cli --features cli-test-server --test mcp`
- Existing direct MCP finite tool names and annotations remain unchanged.
  Direct MCP `sql` intentionally gains optional `params`, and direct MCP `sql`
  output intentionally moves to `{ columns, rows, row_count }`.
- Direct MCP `sql` accepts positional params for `$1`, `$2`, and named params
  for `$name`.
- Direct MCP `sql` rejects unsupported parameter values such as arrays and
  objects, and named parameter keys with a leading `$`.
- Direct MCP `sql` tests cover `null`, boolean, int64, float64, and string
  parameter values.
- Direct MCP `sql` tests prove named parameter map keys are supplied to
  DataFusion without the leading `$`.
- Direct MCP `sql` returns deterministic errors for missing required parameter
  placeholders.
- Engine tests prove parameterized SQL uses DataFusion parameters rather than
  string interpolation, covering both positional and named `ParamValues`.
- Direct MCP structured outputs are bridge-owned. Every finite function
  advertises a direct MCP output schema, and structured values validate against
  those schemas in tests.
- `sql` returns a dynamic-row-safe result envelope with `columns`, `rows`, and
  `row_count`, not a fake statically inferred row type.
- `sql` returns structured `columns` from the concrete DataFusion/Arrow output
  schema even when the query returns zero rows, including `SELECT ... LIMIT 0`.
- SQL schema-output tests cover aliases, casts, literals, JSON function
  expressions, and joins to prove result columns come from the planned query
  output, not from input table schemas.
- Direct and nested SQL preserve DataFusion's read-only enforcement: DDL, DML,
  and SQL statements such as `SET` remain rejected.
- Catalog functions call `CatalogService` and do not reimplement catalog
  matching or fall back to `ExecuteSql` in `coral-mcp`.

For JavaScript Code Mode:

- Default CLI/MCP builds do not compile V8. V8-backed Code Mode builds and
  tests run only when the explicit `code-mode` feature is enabled.
- `tools/list` includes `exec` and `wait` only when `--enable-code-mode` is set.
- `--code-mode-only` lists exactly `exec` and `wait`.
- `ALL_TOOLS` contains only the finite Coral MCP-equivalent functions and never
  provider/source-generated tools, `exec`, or `wait`.
- `ALL_TOOLS` contains `list_catalog` and `search_catalog`, not `list_tables`
  and `search_tables`.
- `ALL_TOOLS` contains `feedback` when direct MCP feedback is enabled, and does
  not contain it when feedback is disabled.
- The `exec` description includes generated declarations for enabled nested
  functions, Coral SQL `SqlFunction` helpers, and generated `CoralSchema.Tables`
  table hints when the catalog schema budget permits.
- Generated SQL types support string/object input plus the tagged-template
  overload, default to a dynamic row type, and show
  `sql<{ n: number }>("SELECT 1 AS n")` only as an optional model-authored
  narrowing pattern.
- Generated SQL table schema hints expose known table column names and
  JavaScript-compatible value types when the description budget permits.
- The `exec` description includes SQL-first guidance and does not advertise
  separate nested helpers such as `rows`, `first`, `one`, or `scalar`.
- The `exec` description mentions DataFusion-backed SQL advantages:
  `information_schema`, `LIMIT 0` schema inspection, source-aware `LIMIT`
  pushdown, and registered JSON functions.
- The nested callable registry contains `sql`, not SQL helper aliases; the
  tagged-template overload is just another `sql` call signature.
- Direct MCP `sql`, `tools.sql({ sql, params })`, and the Code Mode
  tagged-template overload all route through the same bridge function and return
  equivalent structured results.
- Code Mode tagged-template SQL generates placeholders and params; tests assert
  dynamic values are not concatenated into SQL text.
- In code-mode-only mode, direct `tools/call` for `sql` is rejected while
  `tools.sql(...)` inside `exec` still works.
- `exec` accepts both an implicit async function body and a Cloudflare-style
  async arrow function expression.
- `exec` can run `return await tools.sql("SELECT 1 AS n");` and returns the
  structured SQL result as `structuredContent.result` with
  `structuredContent.status = "completed"`.
- `exec` can run an async arrow function that loops over a SQL-returned JSON
  object, builds an array of objects, and returns that array as structured
  output under `structuredContent.result`.
- `exec` returning `undefined` produces `{ "status": "completed" }` and does
  not invent a `result: null` value.
- `exec`/`wait` always use the outer MCP structured shape
  `{ status, result?, cell_id?, error? }`.
- `exec` can call `list_catalog`, `search_catalog`, `describe_table`, and
  `list_columns` with the same result shapes as direct MCP calls.
- `exec` can call `feedback` when feedback is enabled, using the same structured
  result shape as direct MCP.
- Long-running code yields and resumes through `wait`.
- `wait { terminate: true }` returns
  `{ "status": "terminated", "cell_id": "<id>" }`, releases the cell, and a
  subsequent `wait` returns the documented structured not-found error.
- A runaway script that exceeds the per-cell nested bridge-call limit receives a
  deterministic rejection without adding a nested-only SQL row cap.
- Runtime tests prove no ambient `fetch`, filesystem, process environment, or
  secret access exists in the sandbox.

For live MCP parity:

- Start a Coral MCP service with Code Mode enabled and at least one queryable
  source available.
- Run direct MCP `sql` with `SELECT 1 AS n`, then run MCP `exec` with
  `return await tools.sql("SELECT 1 AS n");`, and assert the returned
  `structuredContent.result` is equivalent to direct MCP `sql`.
- Run direct MCP parameterized `sql` with both positional and named params, then
  run Code Mode `tools.sql({ sql, params })` and tagged-template equivalents,
  asserting equivalent structured results.
- Run direct MCP `sql` and Code Mode `tools.sql` with a `LIMIT 0` query and
  assert both return equivalent `columns`, zero rows, and `row_count = 0`.
- Run MCP `exec` with a Cloudflare-style async arrow function that transforms
  SQL-returned JSON into an array of objects, and assert the array appears as
  `structuredContent.result` without requiring print-style helpers.
- Run MCP `exec` with a SQL-first script that uses one SQL query for filtering
  and projection, then returns a reshaped object containing `rows`, `columns`,
  and `row_count`.
- Run direct MCP `list_catalog`, `search_catalog`, `describe_table`, and
  `list_columns`, then run MCP `exec` calling the equivalent nested tools and
  assert equivalent structured results.
- Exercise `wait` against live nested SQL: call `exec` with a script that yields
  before completion, captures the returned `cell_id`, call MCP `wait`, and
  assert the terminal `wait` result has `status = "completed"` and contains the
  same SQL structured result under `structuredContent.result` as direct MCP
  `sql`.
- Exercise `wait { terminate: true }` against a live pending cell and assert the
  returned status is `terminated`, the cell is removed, and a subsequent `wait`
  returns the documented structured not-found error.

For code quality:

- `cargo fmt --all -- --check`
- `cargo clippy -p coral-mcp -p coral-code-mode -p coral-cli -p coral-engine -p coral-client --all-targets --all-features -- -D warnings`
- `make rust-checks`
- Idiomatic Rust and DataFusion. Clear boundaries: `coral-api` owns the
  transport contract, `coral-engine` owns query execution, `coral-client` owns
  Arrow IPC decode/render helpers, and `coral-mcp` owns the finite bridge and
  MCP projection.

Follow-up hardening criteria, intentionally out of this first stack:

- concurrent nested-call budgets;
- aggregate content/structured-output byte budgets;
- feedback-specific per-cell budgets;
- max pending cells;
- wall-clock TTL cleanup;
- stored-value byte budgets;
- structured `CoralToolError` objects inside JavaScript catches rather than
  string promise rejections;
- direct trace assertions for nested versus direct MCP calls.

## No Remaining Open Questions

The current design decisions are specific enough to implement the first stacked
PRs. Reopen the design only if implementation proves one of the resolved
decisions wrong.
