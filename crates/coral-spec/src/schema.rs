//! JSON Schema validation for source manifests.

#![allow(
    dead_code,
    reason = "Schema-facing manifest document types are compiled into JSON Schema, not instantiated."
)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use jsonschema::JSONSchema;
use schemars::{JsonSchema, Schema, schema_for};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::{ManifestError, Result, SourceBackend};

static SOURCE_SCHEMA: OnceLock<JSONSchema> = OnceLock::new();

pub(crate) fn validate_manifest_schema(manifest_json: &JsonValue) -> Result<()> {
    let validator = SOURCE_SCHEMA.get_or_init(|| {
        let schema_json: JsonValue =
            serde_json::from_str(include_str!("schema/source_manifest.schema.json"))
                .expect("embedded source schema must be valid JSON");
        JSONSchema::compile(&schema_json).expect("embedded source schema must compile")
    });
    if let Err(errors) = validator.validate(manifest_json) {
        let problems: Vec<String> = errors
            .take(8)
            .map(|error| {
                let path = error.instance_path.to_string();
                let location = if path.is_empty() { "/" } else { &path };
                format!("  {location}: {error}")
            })
            .collect();
        return Err(ManifestError::validation(format!(
            "source manifest failed schema validation:\n{}",
            problems.join("\n")
        )));
    }
    Ok(())
}

pub(crate) fn parse_manifest_backend(manifest_json: JsonValue) -> Result<SourceBackend> {
    let dispatch: SourceManifestDispatch =
        serde_json::from_value(manifest_json).map_err(ManifestError::deserialize)?;
    Ok(dispatch.backend)
}

/// Generate the canonical source manifest JSON Schema.
///
/// The schema is intentionally derived from schema-facing Rust types that model
/// the authored manifest document. Runtime-normalized validated structs stay
/// separate so validation-only concerns do not leak into engine-facing models.
#[must_use]
pub fn source_manifest_schema() -> Schema {
    let mut schema = schema_for!(SourceManifestDocument);
    schema.insert(
        "$id".to_string(),
        JsonValue::String("https://coral.local/source_manifest.schema.json".to_string()),
    );
    schema
}

/// Return the canonical source manifest JSON Schema as pretty-printed JSON.
///
/// # Errors
///
/// Returns a serde error only if the generated schema cannot be serialized.
pub fn source_manifest_schema_json() -> std::result::Result<String, serde_json::Error> {
    let mut raw = serde_json::to_string_pretty(&source_manifest_schema())?;
    raw.push('\n');
    Ok(raw)
}

#[derive(Debug, Deserialize)]
struct SourceManifestDispatch {
    backend: SourceBackend,
}

#[derive(JsonSchema)]
#[schemars(title = "Coral Source Manifest")]
#[serde(untagged)]
enum SourceManifestDocument {
    HttpWithTablesAndFunctions(HttpManifestWithTablesAndFunctions),
    HttpWithTables(HttpManifestWithTables),
    HttpWithFunctions(HttpManifestWithFunctions),
    Parquet(FileManifest<ParquetBackend>),
    Jsonl(FileManifest<JsonlBackend>),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct HttpManifestWithTablesAndFunctions {
    #[schemars(range(min = 3, max = 3))]
    dsl_version: u32,
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    test_queries: Vec<String>,
    backend: HttpBackend,
    #[serde(default)]
    inputs: Option<Inputs>,
    #[schemars(length(min = 1))]
    base_url: String,
    #[serde(default)]
    auth: Option<Auth>,
    #[serde(default)]
    request_headers: Vec<Header>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[schemars(length(min = 1))]
    tables: Vec<HttpTable>,
    #[schemars(length(min = 1))]
    functions: Vec<TableFunction>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct HttpManifestWithTables {
    #[schemars(range(min = 3, max = 3))]
    dsl_version: u32,
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    test_queries: Vec<String>,
    backend: HttpBackend,
    #[serde(default)]
    inputs: Option<Inputs>,
    #[schemars(length(min = 1))]
    base_url: String,
    #[serde(default)]
    auth: Option<Auth>,
    #[serde(default)]
    request_headers: Vec<Header>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[schemars(length(min = 1))]
    tables: Vec<HttpTable>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct HttpManifestWithFunctions {
    #[schemars(range(min = 3, max = 3))]
    dsl_version: u32,
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    test_queries: Vec<String>,
    backend: HttpBackend,
    #[serde(default)]
    inputs: Option<Inputs>,
    #[schemars(length(min = 1))]
    base_url: String,
    #[serde(default)]
    auth: Option<Auth>,
    #[serde(default)]
    request_headers: Vec<Header>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[schemars(length(min = 1))]
    functions: Vec<TableFunction>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct FileManifest<B: JsonSchema> {
    #[schemars(range(min = 3, max = 3))]
    dsl_version: u32,
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    test_queries: Vec<String>,
    backend: B,
    #[serde(default)]
    inputs: Option<Inputs>,
    #[schemars(length(min = 1))]
    tables: Vec<FileTable>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum HttpBackend {
    Http,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ParquetBackend {
    Parquet,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum JsonlBackend {
    Jsonl,
}

type Inputs = BTreeMap<String, Input>;

#[derive(JsonSchema)]
#[serde(untagged)]
enum Input {
    Variable(VariableInput),
    Secret(SecretInput),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct VariableInput {
    kind: VariableInputKind,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    hint: Option<String>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum VariableInputKind {
    Variable,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct SecretInput {
    kind: SecretInputKind,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    hint: Option<String>,
    #[serde(default)]
    credential: Option<Credential>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SecretInputKind {
    Secret,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Credential {
    #[schemars(length(min = 1))]
    methods: Vec<CredentialMethod>,
}

#[derive(JsonSchema)]
#[serde(untagged)]
enum CredentialMethod {
    SourceConfig(SourceConfigCredentialMethod),
    OAuth(OAuthCredentialMethod),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct SourceConfigCredentialMethod {
    #[serde(rename = "type")]
    kind: SourceConfigCredentialMethodKind,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SourceConfigCredentialMethodKind {
    SourceConfig,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthCredentialMethod {
    #[serde(rename = "type")]
    kind: OAuthCredentialMethodKind,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    description: Option<String>,
    oauth: OAuthCredential,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OAuthCredentialMethodKind {
    Oauth,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthCredential {
    flow: OAuthFlow,
    #[schemars(length(min = 1))]
    redirect_uri: String,
    endpoints: OAuthEndpoints,
    client: OAuthClient,
    #[serde(default)]
    scopes: Option<OAuthScopes>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthFlow {
    #[serde(rename = "type")]
    kind: OAuthFlowKind,
    pkce: OAuthPkce,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OAuthFlowKind {
    AuthorizationCode,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OAuthPkce {
    Required,
    Disabled,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthEndpoints {
    #[schemars(length(min = 1))]
    authorization_url: String,
    #[schemars(length(min = 1))]
    token_url: String,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthClient {
    id: OAuthClientId,
    #[serde(default)]
    secret: Option<OAuthClientSecret>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthClientId {
    #[serde(default)]
    #[schemars(length(min = 1))]
    default: Option<String>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    input: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthClientSecret {
    #[schemars(length(min = 1))]
    input: String,
    transport: OAuthClientSecretTransport,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OAuthClientSecretTransport {
    BasicAuth,
    RequestBody,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthScopes {
    scope: OAuthScope,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct OAuthScope {
    delimiter: OAuthScopeDelimiter,
    #[schemars(length(min = 1), inner(length(min = 1)))]
    values: Vec<String>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OAuthScopeDelimiter {
    Space,
    Comma,
}

#[derive(JsonSchema)]
#[serde(tag = "type")]
enum Auth {
    #[serde(rename = "BasicAuth")]
    Basic(BasicAuth),
    #[serde(rename = "HeaderAuth")]
    Header(HeaderAuth),
    #[serde(rename = "CustomAuth")]
    Custom(CustomAuth),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct BasicAuth {
    #[schemars(length(min = 1))]
    username: String,
    #[schemars(length(min = 1))]
    password: String,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct HeaderAuth {
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(JsonSchema)]
struct CustomAuth {
    #[schemars(length(min = 1))]
    authenticator: String,
    #[serde(flatten)]
    config: BTreeMap<String, JsonValue>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct RateLimit {
    #[serde(default)]
    #[schemars(inner(range(min = 400, max = 599)))]
    extra_statuses: Vec<u16>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    retry_after_header: Option<String>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    remaining_header: Option<String>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    reset_header: Option<String>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct HttpTable {
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    description: String,
    #[serde(default)]
    guide: String,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    fetch_limit_default: Option<usize>,
    #[serde(default)]
    search_limits: Option<SearchLimits>,
    #[serde(default)]
    detail_hints: Vec<DetailHint>,
    #[serde(default)]
    request: Option<Request>,
    #[serde(default)]
    requests: Vec<RequestRoute>,
    #[serde(default)]
    response: Option<Response>,
    #[serde(default)]
    pagination: Option<Pagination>,
    #[serde(default)]
    columns: Vec<Column>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct FileTable {
    #[schemars(length(min = 1))]
    name: String,
    #[schemars(length(min = 1))]
    description: String,
    #[serde(default)]
    guide: String,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    fetch_limit_default: Option<usize>,
    source: FileSource,
    #[serde(default)]
    columns: Vec<Column>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct FileSource {
    #[schemars(length(min = 1))]
    location: String,
    #[serde(default)]
    #[schemars(length(min = 1))]
    glob: Option<String>,
    #[serde(default)]
    partitions: Vec<PartitionColumn>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct PartitionColumn {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(rename = "type")]
    data_type: ManifestDataType,
}

#[derive(JsonSchema)]
enum ManifestDataType {
    Utf8,
    Int64,
    Boolean,
    Float64,
    Timestamp,
    Json,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Filter {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(rename = "type", default)]
    data_type: Option<ManifestDataType>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    mode: Option<FilterMode>,
    #[serde(default)]
    description: String,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum FilterMode {
    Equality,
    Search,
    Contains,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct SearchLimits {
    #[schemars(range(min = 1, max = 1000))]
    default_top_k: usize,
    #[schemars(range(min = 1, max = 1000))]
    max_top_k: usize,
    #[schemars(range(min = 1, max = 100))]
    max_calls_per_query: usize,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct DetailHint {
    #[schemars(length(min = 1))]
    table: String,
    #[schemars(length(min = 1))]
    search_result_column: String,
    #[schemars(length(min = 1))]
    detail_filter: String,
    #[schemars(length(min = 1))]
    purpose: String,
}

#[derive(JsonSchema)]
#[serde(untagged)]
enum TableFunction {
    Search(SearchTableFunction),
    ExplicitTable(ExplicitTableFunction),
    Default(DefaultTableFunction),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct SearchTableFunction {
    #[schemars(length(min = 1))]
    name: String,
    kind: SearchTableFunctionKind,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(range(min = 1))]
    fetch_limit_default: Option<usize>,
    search_limits: SearchLimits,
    #[serde(default)]
    detail_hints: Vec<DetailHint>,
    #[serde(default)]
    args: Vec<TableFunctionArg>,
    request: Request,
    #[serde(default)]
    response: Option<Response>,
    #[serde(default)]
    pagination: Option<Pagination>,
    #[serde(default)]
    columns: Vec<Column>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SearchTableFunctionKind {
    Search,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct ExplicitTableFunction {
    #[schemars(length(min = 1))]
    name: String,
    kind: ExplicitTableFunctionKind,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(range(min = 1))]
    fetch_limit_default: Option<usize>,
    #[serde(default)]
    search_limits: Option<SearchLimits>,
    #[serde(default)]
    detail_hints: Vec<DetailHint>,
    #[serde(default)]
    args: Vec<TableFunctionArg>,
    request: Request,
    #[serde(default)]
    response: Option<Response>,
    #[serde(default)]
    pagination: Option<Pagination>,
    #[serde(default)]
    columns: Vec<Column>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ExplicitTableFunctionKind {
    Table,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct DefaultTableFunction {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    #[schemars(range(min = 1))]
    fetch_limit_default: Option<usize>,
    #[serde(default)]
    search_limits: Option<SearchLimits>,
    #[serde(default)]
    detail_hints: Vec<DetailHint>,
    #[serde(default)]
    args: Vec<TableFunctionArg>,
    request: Request,
    #[serde(default)]
    response: Option<Response>,
    #[serde(default)]
    pagination: Option<Pagination>,
    #[serde(default)]
    columns: Vec<Column>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct TableFunctionArg {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    values: Vec<String>,
    bind: FunctionBinding,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct FunctionBinding {
    #[schemars(length(min = 1))]
    arg: String,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    method: Option<HttpMethod>,
    #[schemars(length(min = 1))]
    path: String,
    #[serde(default)]
    query: Vec<QueryParam>,
    #[serde(default)]
    body: Option<Body>,
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct RequestRoute {
    #[schemars(length(min = 1), inner(length(min = 1)))]
    when_filters: Vec<String>,
    #[serde(default)]
    method: Option<HttpMethod>,
    #[schemars(length(min = 1))]
    path: String,
    #[serde(default)]
    query: Vec<QueryParam>,
    #[serde(default)]
    body: Option<Body>,
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(JsonSchema)]
enum HttpMethod {
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
}

#[derive(JsonSchema)]
#[serde(untagged)]
enum Body {
    Fields(Vec<BodyField>),
    Json(JsonBody),
    Text(TextBody),
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct JsonBody {
    format: JsonBodyFormat,
    #[serde(default)]
    fields: Vec<BodyField>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum JsonBodyFormat {
    Json,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct TextBody {
    format: TextBodyFormat,
    content: ValueSource,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TextBodyFormat {
    Text,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct QueryParam {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(flatten)]
    value: ValueSource,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct BodyField {
    #[schemars(length(min = 1), inner(length(min = 1)))]
    path: Vec<String>,
    #[serde(flatten)]
    value: ValueSource,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Header {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(flatten)]
    value: ValueSource,
}

#[derive(JsonSchema)]
#[serde(tag = "from", rename_all = "snake_case")]
enum ValueSource {
    Template {
        template: String,
    },
    Literal {
        value: JsonValue,
    },
    Filter {
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        default: Option<JsonValue>,
    },
    FilterInt {
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        default: Option<i64>,
    },
    FilterBool {
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        default: Option<bool>,
    },
    FilterSplit {
        #[schemars(length(min = 1))]
        key: String,
        #[schemars(length(min = 1))]
        separator: String,
        part: usize,
    },
    FilterSplitInt {
        #[schemars(length(min = 1))]
        key: String,
        #[schemars(length(min = 1))]
        separator: String,
        part: usize,
    },
    Arg {
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        default: Option<JsonValue>,
    },
    ArgInt {
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        default: Option<i64>,
    },
    ArgBool {
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        default: Option<bool>,
    },
    ArgSplit {
        #[schemars(length(min = 1))]
        key: String,
        #[schemars(length(min = 1))]
        separator: String,
        part: usize,
    },
    ArgSplitInt {
        #[schemars(length(min = 1))]
        key: String,
        #[schemars(length(min = 1))]
        separator: String,
        part: usize,
    },
    Input {
        #[schemars(length(min = 1))]
        key: String,
    },
    State {
        #[schemars(length(min = 1))]
        key: String,
    },
    NowEpochMinusSeconds {
        seconds: i64,
    },
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Response {
    #[serde(default)]
    format: Option<ResponseBodyFormat>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    rows_path: Vec<String>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    ok_path: Vec<String>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    error_path: Vec<String>,
    #[serde(default)]
    allow_404_empty: Option<bool>,
    #[serde(default)]
    row_strategy: Option<RowStrategy>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ResponseBodyFormat {
    Json,
    JsonEachRow,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RowStrategy {
    Direct,
    SeriesPointList,
    DictEntries,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Pagination {
    #[serde(default)]
    mode: Option<PaginationMode>,
    #[serde(default)]
    page_size: Option<PageSize>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    cursor_param: Option<String>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    cursor_body_path: Vec<String>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    response_cursor_path: Vec<String>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    page_param: Option<String>,
    #[serde(default)]
    page_start: Option<i64>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    page_step: Option<i64>,
    #[serde(default)]
    #[schemars(length(min = 1))]
    offset_param: Option<String>,
    #[serde(default)]
    offset_start: Option<i64>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    offset_step: Option<i64>,
    #[serde(default)]
    link_header_require_results: Option<bool>,
    #[serde(default)]
    #[schemars(range(min = 1))]
    max_pages: Option<usize>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum PaginationMode {
    None,
    Auto,
    CursorQuery,
    CursorBody,
    Page,
    Offset,
    LinkHeader,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct PageSize {
    #[schemars(range(min = 1))]
    default: usize,
    #[schemars(range(min = 1))]
    max: usize,
    #[serde(default)]
    #[schemars(length(min = 1))]
    query_param: Option<String>,
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    body_path: Vec<String>,
}

#[derive(JsonSchema)]
#[schemars(deny_unknown_fields)]
struct Column {
    #[schemars(length(min = 1))]
    name: String,
    #[serde(rename = "type")]
    data_type: ManifestDataType,
    #[serde(default)]
    nullable: Option<bool>,
    #[serde(default)]
    #[serde(rename = "virtual")]
    r#virtual: Option<bool>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    expr: Option<Expr>,
}

#[derive(JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Expr {
    Path {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
    },
    Coalesce {
        #[schemars(length(min = 1))]
        exprs: Vec<Expr>,
    },
    FromFilter {
        #[schemars(length(min = 1))]
        key: String,
    },
    Literal {
        value: JsonValue,
    },
    Null,
    JoinArray {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
        #[serde(default)]
        separator: Option<String>,
    },
    JoinArrayPath {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
        #[schemars(inner(length(min = 1)))]
        item_path: Vec<String>,
        #[serde(default)]
        separator: Option<String>,
    },
    TagValue {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        key_field: Option<String>,
        #[serde(default)]
        value_field: Option<String>,
    },
    IfPresent {
        check: Box<Expr>,
        then_value: String,
    },
    JoinTagValues {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
        #[schemars(length(min = 1))]
        key: String,
        #[serde(default)]
        key_field: Option<String>,
        #[serde(default)]
        value_field: Option<String>,
        #[serde(default)]
        separator: Option<String>,
    },
    FirstArrayItemPath {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
        #[schemars(inner(length(min = 1)))]
        item_path: Vec<String>,
    },
    ObjectFilterPath {
        #[schemars(inner(length(min = 1)))]
        path: Vec<String>,
        #[schemars(length(min = 1))]
        filter_key: String,
        #[schemars(inner(length(min = 1)))]
        item_path: Vec<String>,
    },
    CurrentRow,
    FormatTimestamp {
        expr: Box<Expr>,
        #[serde(default)]
        input: Option<TimestampInput>,
    },
    Base64Decode {
        expr: Box<Expr>,
    },
    Replace {
        expr: Box<Expr>,
        #[schemars(length(min = 1))]
        from: String,
        to: String,
    },
    Template {
        #[schemars(length(min = 1))]
        template: String,
        values: BTreeMap<String, Expr>,
    },
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
enum TimestampInput {
    Seconds,
    Milliseconds,
    Iso8601,
}

#[cfg(test)]
mod tests {
    use serde_json::Value as JsonValue;

    use super::{source_manifest_schema_json, validate_manifest_schema};
    use crate::parser::parse_source_manifest_yaml;

    fn valid_http_manifest() -> &'static str {
        r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    request:
      method: GET
      path: /messages
"
    }

    fn manifest_json(raw: &str) -> JsonValue {
        serde_yaml::from_str(raw).expect("test manifest should parse as yaml")
    }

    fn assert_schema_failure(message: &str) {
        assert!(
            message.starts_with("source manifest failed schema validation:"),
            "{message}"
        );
        assert!(
            message.contains("is not valid under any of the schemas listed in the 'anyOf' keyword"),
            "{message}"
        );
    }

    #[test]
    fn validate_manifest_schema_accepts_valid_http_manifest() {
        let manifest = manifest_json(valid_http_manifest());
        validate_manifest_schema(&manifest).expect("valid manifest should pass schema validation");
    }

    #[test]
    fn checked_in_source_manifest_schema_is_fresh() {
        let generated = source_manifest_schema_json().expect("schema should serialize");
        assert_eq!(
            generated,
            include_str!("schema/source_manifest.schema.json"),
            "source manifest schema is stale; run `make source-schema-generate`"
        );
    }

    #[test]
    fn parse_source_manifest_yaml_accepts_http_table_search_metadata() {
        parse_source_manifest_yaml(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    filters:
      - name: query
        mode: search
      - name: id
    search_limits:
      default_top_k: 5
      max_top_k: 20
      max_calls_per_query: 2
    detail_hints:
      - table: messages
        search_result_column: id
        detail_filter: id
        purpose: Fetch the full message record.
    request:
      method: GET
      path: /messages
    columns:
      - name: id
        type: Utf8
",
        )
        .expect("HTTP table search metadata should pass full manifest parsing");
    }

    #[test]
    fn validate_manifest_schema_rejects_search_function_without_search_limits() {
        let manifest = manifest_json(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
functions:
  - name: search_messages
    kind: search
    request:
      method: GET
      path: /messages/search
",
        );
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("\"kind\":\"search\""), "{message}");
        assert!(!message.contains("search_limits"), "{message}");
    }

    #[test]
    fn validate_manifest_schema_rejects_unknown_filter_type() {
        let manifest = manifest_json(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    filters:
      - name: query
        type: Banana
    request:
      method: GET
      path: /messages
",
        );
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("Banana"), "{message}");
    }

    #[test]
    fn validate_manifest_schema_rejects_file_table_search_metadata() {
        let manifest = manifest_json(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: parquet
tables:
  - name: messages
    description: Demo messages
    source:
      location: file:///tmp/messages.parquet
    search_limits:
      default_top_k: 5
      max_top_k: 20
      max_calls_per_query: 2
    detail_hints: []
",
        );
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("search_limits"), "{message}");
        assert!(message.contains("detail_hints"), "{message}");
    }

    #[test]
    fn validate_manifest_schema_rejects_http_table_source() {
        let manifest = manifest_json(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    source:
      location: file:///tmp/messages.jsonl
    request:
      method: GET
      path: /messages
",
        );
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("source"), "{message}");
    }

    #[test]
    fn validate_manifest_schema_rejects_search_limits_above_cap() {
        let manifest = manifest_json(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    search_limits:
      default_top_k: 5
      max_top_k: 1001
      max_calls_per_query: 1
    request:
      method: GET
      path: /messages
",
        );
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("1001"), "{message}");
    }

    #[test]
    fn validate_manifest_schema_rejects_unknown_top_level_field() {
        let manifest = manifest_json(&format!("schema: legacy\n{}", valid_http_manifest()));
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("schema"), "{message}");
    }

    #[test]
    fn validate_manifest_schema_rejects_missing_backend() {
        let manifest = manifest_json(
            r"
name: demo
version: 1.0.0
dsl_version: 3
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    request:
      method: GET
      path: /messages
",
        );
        let error = validate_manifest_schema(&manifest).expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(!message.contains("\"backend\":\""), "{message}");
    }

    #[test]
    fn parse_source_manifest_yaml_surfaces_request_path_schema_errors() {
        let error = parse_source_manifest_yaml(
            r#"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    request:
      method: GET
      path: ""
"#,
        )
        .expect_err("schema validation should fail");
        let message = error.to_string();
        assert_schema_failure(&message);
        assert!(message.contains("\"path\":\"\""), "{message}");
    }
}
