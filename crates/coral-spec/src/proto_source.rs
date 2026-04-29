//! Conversion between authored source YAML and generated source-manifest proto.

use serde_json::{Map, Number, Value};

use crate::common::parse_manifest_data_type;
use crate::proto::v1 as specv1;
use crate::{ManifestError, Result};

const SOURCE_API_VERSION: &str = "coral.withcoral.com/v1alpha1";

pub(crate) fn source_manifest_proto_from_yaml(raw: &str) -> Result<specv1::SourceManifest> {
    let value: Value = serde_yaml::from_str(raw).map_err(ManifestError::parse_yaml)?;
    source_manifest_proto_from_value(value)
}

pub(crate) fn source_manifest_proto_from_value(value: Value) -> Result<specv1::SourceManifest> {
    parse_source_manifest(value)
}

pub(crate) fn validate_source_manifest_proto(manifest: &specv1::SourceManifest) -> Result<()> {
    if !manifest.api_version.is_empty() && manifest.api_version != SOURCE_API_VERSION {
        return Err(ManifestError::validation(format!(
            "source manifest api_version '{}' is unsupported",
            manifest.api_version
        )));
    }
    if !manifest.kind.is_empty() && manifest.kind != "Source" {
        return Err(ManifestError::validation(format!(
            "source manifest kind '{}' is unsupported",
            manifest.kind
        )));
    }
    if manifest.name.trim().is_empty() {
        return Err(ManifestError::validation(
            "source manifest must define a non-empty name",
        ));
    }
    if manifest.version.trim().is_empty() {
        return Err(ManifestError::validation(
            "source manifest must define a non-empty version",
        ));
    }
    if manifest.dsl_version != 3 {
        return Err(ManifestError::validation(format!(
            "source manifest dsl_version must be 3, got {}",
            manifest.dsl_version
        )));
    }
    if source_backend(manifest.backend)? == specv1::SourceBackend::Unspecified {
        return Err(ManifestError::validation(
            "source manifest must define backend",
        ));
    }
    if manifest.tables.is_empty() {
        return Err(ManifestError::validation(
            "source manifest must define at least one table",
        ));
    }
    validate_input_bindings(&manifest.inputs)?;
    if let Some(auth) = &manifest.auth {
        validate_auth_spec(auth)?;
    }
    if let Some(rate_limit) = &manifest.rate_limit {
        validate_rate_limit_spec(rate_limit)?;
    }
    for table in &manifest.tables {
        validate_table_spec(table)?;
    }
    Ok(())
}

fn validate_input_bindings(inputs: &[specv1::SourceInputBinding]) -> Result<()> {
    for input in inputs {
        ensure_non_empty(&input.key, "source manifest input key")?;
        let spec = input.input.as_ref().ok_or_else(|| {
            ManifestError::validation(format!(
                "source manifest input '{}' is missing input spec",
                input.key
            ))
        })?;
        let kind = source_input_kind(spec.kind)?;
        if kind == specv1::SourceInputKind::Unspecified {
            return Err(ManifestError::validation(format!(
                "source manifest input '{}' is missing kind",
                input.key
            )));
        }
        if kind == specv1::SourceInputKind::Secret && spec.default_value.is_some() {
            return Err(ManifestError::validation(format!(
                "source manifest secret input '{}' must not define default",
                input.key
            )));
        }
    }
    Ok(())
}

fn validate_auth_spec(auth: &specv1::AuthSpec) -> Result<()> {
    validate_headers(&auth.headers)
}

fn validate_rate_limit_spec(rate_limit: &specv1::RateLimitSpec) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for status in &rate_limit.extra_statuses {
        if !(400..=599).contains(status) {
            return Err(ManifestError::validation(format!(
                "source manifest rate_limit extra status {status} is outside 400..599"
            )));
        }
        if !seen.insert(status) {
            return Err(ManifestError::validation(format!(
                "source manifest rate_limit repeats extra status {status}"
            )));
        }
    }
    if let Some(header) = &rate_limit.retry_after_header {
        ensure_non_empty(header, "source manifest rate_limit.retry_after_header")?;
    }
    if let Some(header) = &rate_limit.remaining_header {
        ensure_non_empty(header, "source manifest rate_limit.remaining_header")?;
    }
    if let Some(header) = &rate_limit.reset_header {
        ensure_non_empty(header, "source manifest rate_limit.reset_header")?;
    }
    Ok(())
}

fn validate_table_spec(table: &specv1::TableSpec) -> Result<()> {
    ensure_non_empty(&table.name, "source manifest table name")?;
    ensure_non_empty(&table.description, "source manifest table description")?;
    for filter in &table.filters {
        ensure_non_empty(&filter.name, "source manifest filter name")?;
        let _ = filter_mode(filter.mode)?;
    }
    if let Some(limit) = table.fetch_limit_default {
        ensure_positive_u64(limit, "source manifest table fetch_limit_default")?;
    }
    if let Some(request) = &table.request {
        validate_request_spec(request, true)?;
    }
    for route in &table.requests {
        validate_request_route_spec(route)?;
    }
    if let Some(response) = &table.response {
        validate_response_spec(response)?;
    }
    if let Some(pagination) = &table.pagination {
        validate_pagination_spec(pagination)?;
    }
    if let Some(source) = &table.source {
        validate_file_source_spec(source)?;
    }
    for column in &table.columns {
        validate_column_spec(column)?;
    }
    Ok(())
}

fn validate_request_route_spec(route: &specv1::RequestRouteSpec) -> Result<()> {
    ensure_non_empty_items(
        &route.when_filters,
        "source manifest request route when_filters",
    )?;
    if route.when_filters.is_empty() {
        return Err(ManifestError::validation(
            "source manifest request route when_filters must contain at least one item",
        ));
    }
    if let Some(request) = &route.request {
        validate_request_spec(request, false)?;
    }
    Ok(())
}

fn validate_request_spec(request: &specv1::RequestSpec, require_path: bool) -> Result<()> {
    let _ = http_method(request.method)?;
    if require_path {
        ensure_non_empty(&request.path, "source manifest request path")?;
    }
    for param in &request.query {
        ensure_non_empty(&param.name, "source manifest query param name")?;
        validate_value_source(param.value.as_ref(), "source manifest query param")?;
    }
    for field in &request.body {
        ensure_non_empty_items(&field.path, "source manifest body field path")?;
        if field.path.is_empty() {
            return Err(ManifestError::validation(
                "source manifest body field path must contain at least one item",
            ));
        }
        validate_value_source(field.value.as_ref(), "source manifest body field")?;
    }
    validate_headers(&request.headers)
}

fn validate_headers(headers: &[specv1::HeaderSpec]) -> Result<()> {
    for header in headers {
        ensure_non_empty(&header.name, "source manifest header name")?;
        validate_value_source(header.value.as_ref(), "source manifest header")?;
    }
    Ok(())
}

fn validate_value_source(source: Option<&specv1::ValueSource>, context: &str) -> Result<()> {
    let source = source
        .ok_or_else(|| ManifestError::validation(format!("{context} is missing value source")))?;
    let kind = source.kind.as_ref().ok_or_else(|| {
        ManifestError::validation(format!("{context} is missing value source kind"))
    })?;
    match kind {
        specv1::value_source::Kind::Literal(value) => {
            let _ = json_value(&value.json)?;
        }
        specv1::value_source::Kind::Filter(value) => {
            ensure_non_empty(&value.key, &format!("{context} key"))?;
            if let Some(default_json) = &value.default_json {
                let _ = json_value(default_json)?;
            }
        }
        specv1::value_source::Kind::FilterInt(value) => {
            ensure_non_empty(&value.key, &format!("{context} key"))?;
        }
        specv1::value_source::Kind::Input(value) => {
            ensure_non_empty(&value.key, &format!("{context} key"))?;
        }
        specv1::value_source::Kind::State(value) => {
            ensure_non_empty(&value.key, &format!("{context} key"))?;
        }
        specv1::value_source::Kind::Template(_)
        | specv1::value_source::Kind::NowEpochMinusSeconds(_) => {}
    }
    Ok(())
}

fn validate_response_spec(response: &specv1::ResponseSpec) -> Result<()> {
    ensure_non_empty_items(&response.rows_path, "source manifest response rows_path")?;
    ensure_non_empty_items(&response.ok_path, "source manifest response ok_path")?;
    ensure_non_empty_items(&response.error_path, "source manifest response error_path")?;
    let _ = row_strategy(response.row_strategy)?;
    Ok(())
}

fn validate_pagination_spec(pagination: &specv1::PaginationSpec) -> Result<()> {
    let _ = pagination_mode(pagination.mode)?;
    if let Some(page_size) = &pagination.page_size {
        ensure_positive_u64(page_size.default_size, "source manifest page_size default")?;
        ensure_positive_u64(page_size.max, "source manifest page_size max")?;
        if let Some(query_param) = &page_size.query_param {
            ensure_non_empty(query_param, "source manifest page_size query_param")?;
        }
        ensure_non_empty_items(&page_size.body_path, "source manifest page_size body_path")?;
    }
    if let Some(cursor_param) = &pagination.cursor_param {
        ensure_non_empty(cursor_param, "source manifest pagination cursor_param")?;
    }
    ensure_non_empty_items(
        &pagination.cursor_body_path,
        "source manifest pagination cursor_body_path",
    )?;
    ensure_non_empty_items(
        &pagination.response_cursor_path,
        "source manifest pagination response_cursor_path",
    )?;
    if let Some(page_param) = &pagination.page_param {
        ensure_non_empty(page_param, "source manifest pagination page_param")?;
    }
    if let Some(page_step) = pagination.page_step {
        ensure_positive_i64(page_step, "source manifest pagination page_step")?;
    }
    if let Some(offset_param) = &pagination.offset_param {
        ensure_non_empty(offset_param, "source manifest pagination offset_param")?;
    }
    if let Some(offset_step) = pagination.offset_step {
        ensure_positive_i64(offset_step, "source manifest pagination offset_step")?;
    }
    if let Some(max_pages) = pagination.max_pages {
        ensure_positive_u64(max_pages, "source manifest pagination max_pages")?;
    }
    Ok(())
}

fn validate_file_source_spec(source: &specv1::FileSourceSpec) -> Result<()> {
    ensure_non_empty(&source.location, "source manifest source location")?;
    if let Some(glob) = &source.glob {
        ensure_non_empty(glob, "source manifest source glob")?;
    }
    for partition in &source.partitions {
        ensure_non_empty(&partition.name, "source manifest partition name")?;
        ensure_non_empty(&partition.data_type, "source manifest partition type")?;
        let _ = parse_manifest_data_type(&partition.data_type)?;
    }
    Ok(())
}

fn validate_column_spec(column: &specv1::ColumnSpec) -> Result<()> {
    ensure_non_empty(&column.name, "source manifest column name")?;
    ensure_non_empty(&column.data_type, "source manifest column type")?;
    let _ = parse_manifest_data_type(&column.data_type)?;
    if let Some(expr) = &column.expr {
        validate_expr_spec(expr)?;
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "Expression validation mirrors the generated source manifest protobuf."
)]
fn validate_expr_spec(expr: &specv1::ExprSpec) -> Result<()> {
    let kind = expr
        .kind
        .as_ref()
        .ok_or_else(|| ManifestError::validation("source manifest expression is missing kind"))?;
    match kind {
        specv1::expr_spec::Kind::Path(expr) => {
            ensure_non_empty_items(&expr.path, "source manifest path expression path")?;
        }
        specv1::expr_spec::Kind::Coalesce(expr) => {
            if expr.exprs.is_empty() {
                return Err(ManifestError::validation(
                    "source manifest coalesce expression exprs must contain at least one item",
                ));
            }
            for nested in &expr.exprs {
                validate_expr_spec(nested)?;
            }
        }
        specv1::expr_spec::Kind::FromFilter(expr) => {
            ensure_non_empty(&expr.key, "source manifest from_filter expression key")?;
        }
        specv1::expr_spec::Kind::Literal(expr) => {
            let _ = json_value(&expr.json)?;
        }
        specv1::expr_spec::Kind::JoinArray(expr) => {
            ensure_non_empty_items(&expr.path, "source manifest join_array expression path")?;
        }
        specv1::expr_spec::Kind::TagValue(expr) => {
            ensure_non_empty_items(&expr.path, "source manifest tag_value expression path")?;
            ensure_non_empty(&expr.key, "source manifest tag_value expression key")?;
        }
        specv1::expr_spec::Kind::IfPresent(expr) => {
            validate_expr_spec(required_expr(expr.check.as_deref(), "if_present check")?)?;
        }
        specv1::expr_spec::Kind::JoinTagValues(expr) => {
            ensure_non_empty_items(
                &expr.path,
                "source manifest join_tag_values expression path",
            )?;
            ensure_non_empty(&expr.key, "source manifest join_tag_values expression key")?;
        }
        specv1::expr_spec::Kind::FirstArrayItemPath(expr) => {
            ensure_non_empty_items(
                &expr.path,
                "source manifest first_array_item_path expression path",
            )?;
            ensure_non_empty_items(
                &expr.item_path,
                "source manifest first_array_item_path expression item_path",
            )?;
        }
        specv1::expr_spec::Kind::ObjectFilterPath(expr) => {
            ensure_non_empty_items(
                &expr.path,
                "source manifest object_filter_path expression path",
            )?;
            ensure_non_empty(
                &expr.filter_key,
                "source manifest object_filter_path expression filter_key",
            )?;
            ensure_non_empty_items(
                &expr.item_path,
                "source manifest object_filter_path expression item_path",
            )?;
        }
        specv1::expr_spec::Kind::NullValue(_) | specv1::expr_spec::Kind::CurrentRow(_) => {}
        specv1::expr_spec::Kind::FormatTimestamp(expr) => {
            validate_expr_spec(required_expr(
                expr.expr.as_deref(),
                "format_timestamp expr",
            )?)?;
            let _ = timestamp_input(expr.input)?;
        }
        specv1::expr_spec::Kind::Replace(expr) => {
            validate_expr_spec(required_expr(expr.expr.as_deref(), "replace expr")?)?;
            ensure_non_empty(&expr.from, "source manifest replace expression from")?;
        }
        specv1::expr_spec::Kind::Template(expr) => {
            ensure_non_empty(
                &expr.template,
                "source manifest template expression template",
            )?;
            for value in expr.values.values() {
                validate_expr_spec(value)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn source_manifest_proto_to_value(manifest: &specv1::SourceManifest) -> Result<Value> {
    validate_source_manifest_proto(manifest)?;

    let mut object = Map::new();
    object.insert(
        "dsl_version".to_string(),
        json_u64(u64::from(manifest.dsl_version)),
    );
    object.insert("name".to_string(), Value::String(manifest.name.clone()));
    object.insert(
        "version".to_string(),
        Value::String(manifest.version.clone()),
    );
    if !manifest.description.is_empty() {
        object.insert(
            "description".to_string(),
            Value::String(manifest.description.clone()),
        );
    }
    if !manifest.test_queries.is_empty() {
        object.insert(
            "test_queries".to_string(),
            string_array_value(&manifest.test_queries),
        );
    }
    object.insert(
        "backend".to_string(),
        Value::String(backend_to_manifest_value(source_backend(manifest.backend)?).to_string()),
    );
    if !manifest.inputs.is_empty() {
        object.insert("inputs".to_string(), inputs_to_value(&manifest.inputs)?);
    }
    if !manifest.base_url.is_empty() {
        object.insert(
            "base_url".to_string(),
            Value::String(manifest.base_url.clone()),
        );
    }
    if let Some(auth) = &manifest.auth {
        let value = auth_to_value(auth)?;
        if !value.as_object().is_some_and(Map::is_empty) {
            object.insert("auth".to_string(), value);
        }
    }
    if let Some(rate_limit) = &manifest.rate_limit {
        let value = rate_limit_to_value(rate_limit);
        if !value.as_object().is_some_and(Map::is_empty) {
            object.insert("rate_limit".to_string(), value);
        }
    }
    object.insert(
        "tables".to_string(),
        Value::Array(
            manifest
                .tables
                .iter()
                .map(table_to_value)
                .collect::<Result<Vec<_>>>()?,
        ),
    );
    Ok(Value::Object(object))
}

fn parse_source_manifest(value: Value) -> Result<specv1::SourceManifest> {
    let map = object(value, "source manifest")?;
    let mut manifest = specv1::SourceManifest::default();
    for (key, value) in map {
        match key.as_str() {
            "api_version" => manifest.api_version = string(value, "source manifest api_version")?,
            "kind" => manifest.kind = string(value, "source manifest kind")?,
            "name" => manifest.name = string(value, "source manifest name")?,
            "version" => manifest.version = string(value, "source manifest version")?,
            "dsl_version" => {
                manifest.dsl_version = u32_number(value, "source manifest dsl_version")?;
            }
            "backend" => manifest.backend = parse_backend(value)? as i32,
            "description" => {
                manifest.description = string(value, "source manifest description")?;
            }
            "test_queries" => {
                manifest.test_queries = string_array(value, "source manifest test_queries")?;
            }
            "inputs" => manifest.inputs = parse_inputs(value)?,
            "base_url" => manifest.base_url = string(value, "source manifest base_url")?,
            "auth" => manifest.auth = Some(parse_auth(value)?),
            "rate_limit" => manifest.rate_limit = Some(parse_rate_limit(value)?),
            "tables" => manifest.tables = parse_tables(value)?,
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest has unknown field '{other}'"
                )));
            }
        }
    }
    validate_source_manifest_proto(&manifest)?;
    Ok(manifest)
}

fn parse_backend(value: Value) -> Result<specv1::SourceBackend> {
    match string(value, "source manifest backend")?.as_str() {
        "http" => Ok(specv1::SourceBackend::Http),
        "parquet" => Ok(specv1::SourceBackend::Parquet),
        "jsonl" => Ok(specv1::SourceBackend::Jsonl),
        other => Err(ManifestError::validation(format!(
            "source manifest backend '{other}' is unsupported"
        ))),
    }
}

fn parse_inputs(value: Value) -> Result<Vec<specv1::SourceInputBinding>> {
    let map = object(value, "source manifest inputs")?;
    let mut inputs = Vec::with_capacity(map.len());
    for (key, value) in map {
        inputs.push(specv1::SourceInputBinding {
            key,
            input: Some(parse_input(value)?),
        });
    }
    Ok(inputs)
}

fn parse_input(value: Value) -> Result<specv1::SourceInput> {
    let mut map = object(value, "source manifest input")?;
    let mut input = specv1::SourceInput::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "kind" => {
                input.kind = match string(value, "source manifest input kind")?.as_str() {
                    "variable" => specv1::SourceInputKind::Variable as i32,
                    "secret" => specv1::SourceInputKind::Secret as i32,
                    other => {
                        return Err(ManifestError::validation(format!(
                            "source manifest input kind '{other}' is unsupported"
                        )));
                    }
                };
            }
            "default" => {
                input.default_value = Some(string(value, "source manifest input default")?);
            }
            "required" => {
                input.required = Some(bool_value(value, "source manifest input required")?);
            }
            "hint" => input.hint = string(value, "source manifest input hint")?,
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest input has unknown field '{other}'"
                )));
            }
        }
    }
    if source_input_kind(input.kind)? == specv1::SourceInputKind::Unspecified {
        return Err(ManifestError::validation(
            "source manifest input is missing kind",
        ));
    }
    Ok(input)
}

fn parse_auth(value: Value) -> Result<specv1::AuthSpec> {
    let mut map = object(value, "source manifest auth")?;
    let mut auth = specv1::AuthSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "headers" => auth.headers = parse_headers(value)?,
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest auth has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(auth)
}

fn parse_rate_limit(value: Value) -> Result<specv1::RateLimitSpec> {
    let mut map = object(value, "source manifest rate_limit")?;
    let mut rate_limit = specv1::RateLimitSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "extra_statuses" => {
                let mut seen = std::collections::BTreeSet::new();
                for status in u32_array(value, "source manifest rate_limit.extra_statuses")? {
                    if !(400..=599).contains(&status) {
                        return Err(ManifestError::validation(format!(
                            "source manifest rate_limit extra status {status} is outside 400..599"
                        )));
                    }
                    if !seen.insert(status) {
                        return Err(ManifestError::validation(format!(
                            "source manifest rate_limit repeats extra status {status}"
                        )));
                    }
                    rate_limit.extra_statuses.push(status);
                }
            }
            "retry_after_header" => {
                rate_limit.retry_after_header = Some(non_empty_string(
                    value,
                    "source manifest rate_limit.retry_after_header",
                )?);
            }
            "remaining_header" => {
                rate_limit.remaining_header = Some(non_empty_string(
                    value,
                    "source manifest rate_limit.remaining_header",
                )?);
            }
            "reset_header" => {
                rate_limit.reset_header = Some(non_empty_string(
                    value,
                    "source manifest rate_limit.reset_header",
                )?);
            }
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest rate_limit has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(rate_limit)
}

fn parse_tables(value: Value) -> Result<Vec<specv1::TableSpec>> {
    array(value, "source manifest tables")?
        .into_iter()
        .map(parse_table)
        .collect()
}

fn parse_table(value: Value) -> Result<specv1::TableSpec> {
    let mut map = object(value, "source manifest table")?;
    let mut table = specv1::TableSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "name" => table.name = string(value, "source manifest table name")?,
            "description" => {
                table.description = string(value, "source manifest table description")?;
            }
            "guide" => table.guide = string(value, "source manifest table guide")?,
            "filters" => table.filters = parse_filters(value)?,
            "fetch_limit_default" => {
                table.fetch_limit_default = Some(u64_number(
                    value,
                    "source manifest table fetch_limit_default",
                )?);
            }
            "request" => table.request = Some(parse_request(value)?),
            "requests" => table.requests = parse_request_routes(value)?,
            "response" => table.response = Some(parse_response(value)?),
            "pagination" => table.pagination = Some(parse_pagination(value)?),
            "source" => table.source = Some(parse_file_source(value)?),
            "columns" => table.columns = parse_columns(value)?,
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest table has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(table)
}

fn parse_filters(value: Value) -> Result<Vec<specv1::FilterSpec>> {
    array(value, "source manifest filters")?
        .into_iter()
        .map(parse_filter)
        .collect()
}

fn parse_filter(value: Value) -> Result<specv1::FilterSpec> {
    let mut map = object(value, "source manifest filter")?;
    let mut filter = specv1::FilterSpec {
        mode: specv1::FilterMode::Equality as i32,
        ..specv1::FilterSpec::default()
    };
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "name" => filter.name = string(value, "source manifest filter name")?,
            "required" => filter.required = bool_value(value, "source manifest filter required")?,
            "mode" => {
                filter.mode = match string(value, "source manifest filter mode")?.as_str() {
                    "equality" => specv1::FilterMode::Equality as i32,
                    "search" => specv1::FilterMode::Search as i32,
                    "contains" => specv1::FilterMode::Contains as i32,
                    other => {
                        return Err(ManifestError::validation(format!(
                            "source manifest filter mode '{other}' is unsupported"
                        )));
                    }
                };
            }
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest filter has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(filter)
}

fn parse_request_routes(value: Value) -> Result<Vec<specv1::RequestRouteSpec>> {
    array(value, "source manifest request routes")?
        .into_iter()
        .map(parse_request_route)
        .collect()
}

fn parse_request_route(value: Value) -> Result<specv1::RequestRouteSpec> {
    let mut map = object(value, "source manifest request route")?;
    let when_filters = match map.remove("when_filters") {
        Some(value) => string_array(value, "source manifest request route when_filters")?,
        None => {
            return Err(ManifestError::validation(
                "source manifest request route is missing when_filters",
            ));
        }
    };
    Ok(specv1::RequestRouteSpec {
        when_filters,
        request: Some(parse_request_map(map, "source manifest request route")?),
    })
}

fn parse_request(value: Value) -> Result<specv1::RequestSpec> {
    let map = object(value, "source manifest request")?;
    parse_request_map(map, "source manifest request")
}

fn parse_request_map(mut map: Map<String, Value>, context: &str) -> Result<specv1::RequestSpec> {
    let mut request = specv1::RequestSpec {
        method: specv1::HttpMethod::Get as i32,
        ..specv1::RequestSpec::default()
    };
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "method" => {
                request.method = match string(value, &format!("{context} method"))?.as_str() {
                    "GET" => specv1::HttpMethod::Get as i32,
                    "POST" => specv1::HttpMethod::Post as i32,
                    other => {
                        return Err(ManifestError::validation(format!(
                            "{context} method '{other}' is unsupported"
                        )));
                    }
                };
            }
            "path" => request.path = string(value, &format!("{context} path"))?,
            "query" => request.query = parse_query_params(value)?,
            "body" => request.body = parse_body_fields(value)?,
            "headers" => request.headers = parse_headers(value)?,
            other => {
                return Err(ManifestError::validation(format!(
                    "{context} has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(request)
}

fn parse_query_params(value: Value) -> Result<Vec<specv1::QueryParamSpec>> {
    array(value, "source manifest query params")?
        .into_iter()
        .map(parse_query_param)
        .collect()
}

fn parse_query_param(value: Value) -> Result<specv1::QueryParamSpec> {
    let mut map = object(value, "source manifest query param")?;
    let name = required_string_field(&mut map, "name", "source manifest query param")?;
    let explode = map
        .remove("explode")
        .map(|value| bool_value(value, "source manifest query param explode"))
        .transpose()?;
    Ok(specv1::QueryParamSpec {
        name,
        value: Some(parse_value_source_map(map, "source manifest query param")?),
        explode,
    })
}

fn parse_body_fields(value: Value) -> Result<Vec<specv1::BodyFieldSpec>> {
    array(value, "source manifest body fields")?
        .into_iter()
        .map(parse_body_field)
        .collect()
}

fn parse_body_field(value: Value) -> Result<specv1::BodyFieldSpec> {
    let mut map = object(value, "source manifest body field")?;
    let path = match map.remove("path") {
        Some(value) => string_array(value, "source manifest body field path")?,
        None => {
            return Err(ManifestError::validation(
                "source manifest body field is missing path",
            ));
        }
    };
    Ok(specv1::BodyFieldSpec {
        path,
        value: Some(parse_value_source_map(map, "source manifest body field")?),
    })
}

fn parse_headers(value: Value) -> Result<Vec<specv1::HeaderSpec>> {
    array(value, "source manifest headers")?
        .into_iter()
        .map(parse_header)
        .collect()
}

fn parse_header(value: Value) -> Result<specv1::HeaderSpec> {
    let mut map = object(value, "source manifest header")?;
    let name = required_string_field(&mut map, "name", "source manifest header")?;
    Ok(specv1::HeaderSpec {
        name,
        value: Some(parse_value_source_map(map, "source manifest header")?),
    })
}

fn parse_value_source_map(
    mut map: Map<String, Value>,
    context: &str,
) -> Result<specv1::ValueSource> {
    let Some(from) = map.remove("from") else {
        return Err(ManifestError::validation(format!(
            "{context} is missing from"
        )));
    };
    let from = string(from, &format!("{context} from"))?;
    let kind = match from.as_str() {
        "template" => {
            let template = required_string_field(&mut map, "template", context)?;
            specv1::value_source::Kind::Template(specv1::TemplateValue { template })
        }
        "literal" => {
            let Some(value) = map.remove("value") else {
                return Err(ManifestError::validation(format!(
                    "{context} is missing value"
                )));
            };
            specv1::value_source::Kind::Literal(specv1::LiteralValue {
                json: json_string(&value)?,
            })
        }
        "filter" => {
            let key = required_string_field(&mut map, "key", context)?;
            let default_json = map
                .remove("default")
                .map(|value| json_string(&value))
                .transpose()?;
            specv1::value_source::Kind::Filter(specv1::FilterValue { key, default_json })
        }
        "filter_int" => {
            let key = required_string_field(&mut map, "key", context)?;
            let default_value = map
                .remove("default")
                .map(|value| i64_number(value, &format!("{context} default")))
                .transpose()?;
            specv1::value_source::Kind::FilterInt(specv1::FilterIntValue { key, default_value })
        }
        "input" => {
            let key = required_string_field(&mut map, "key", context)?;
            specv1::value_source::Kind::Input(specv1::InputValue { key })
        }
        "state" => {
            let key = required_string_field(&mut map, "key", context)?;
            specv1::value_source::Kind::State(specv1::StateValue { key })
        }
        "now_epoch_minus_seconds" => {
            let Some(seconds) = map.remove("seconds") else {
                return Err(ManifestError::validation(format!(
                    "{context} is missing seconds"
                )));
            };
            specv1::value_source::Kind::NowEpochMinusSeconds(specv1::NowEpochMinusSecondsValue {
                seconds: i64_number(seconds, &format!("{context} seconds"))?,
            })
        }
        other => {
            return Err(ManifestError::validation(format!(
                "{context} from '{other}' is unsupported"
            )));
        }
    };
    reject_unknown(&map, context)?;
    Ok(specv1::ValueSource { kind: Some(kind) })
}

fn parse_response(value: Value) -> Result<specv1::ResponseSpec> {
    let mut map = object(value, "source manifest response")?;
    let mut response = specv1::ResponseSpec {
        row_strategy: specv1::RowStrategy::Direct as i32,
        ..specv1::ResponseSpec::default()
    };
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "rows_path" => {
                response.rows_path = string_array(value, "source manifest response rows_path")?;
            }
            "ok_path" => {
                response.ok_path = string_array(value, "source manifest response ok_path")?;
            }
            "error_path" => {
                response.error_path = string_array(value, "source manifest response error_path")?;
            }
            "allow_404_empty" => {
                response.allow_404_empty =
                    bool_value(value, "source manifest response allow_404_empty")?;
            }
            "row_strategy" => {
                response.row_strategy =
                    match string(value, "source manifest response row_strategy")?.as_str() {
                        "direct" => specv1::RowStrategy::Direct as i32,
                        "series_point_list" => specv1::RowStrategy::SeriesPointList as i32,
                        "dict_entries" => specv1::RowStrategy::DictEntries as i32,
                        other => {
                            return Err(ManifestError::validation(format!(
                                "source manifest response row_strategy '{other}' is unsupported"
                            )));
                        }
                    };
            }
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest response has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(response)
}

fn parse_pagination(value: Value) -> Result<specv1::PaginationSpec> {
    let mut map = object(value, "source manifest pagination")?;
    let mut pagination = specv1::PaginationSpec {
        mode: specv1::PaginationMode::None as i32,
        ..specv1::PaginationSpec::default()
    };
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "mode" => {
                pagination.mode = match string(value, "source manifest pagination mode")?.as_str() {
                    "none" => specv1::PaginationMode::None as i32,
                    "auto" => specv1::PaginationMode::Auto as i32,
                    "cursor_query" => specv1::PaginationMode::CursorQuery as i32,
                    "cursor_body" => specv1::PaginationMode::CursorBody as i32,
                    "page" => specv1::PaginationMode::Page as i32,
                    "offset" => specv1::PaginationMode::Offset as i32,
                    "link_header" => specv1::PaginationMode::LinkHeader as i32,
                    other => {
                        return Err(ManifestError::validation(format!(
                            "source manifest pagination mode '{other}' is unsupported"
                        )));
                    }
                };
            }
            "page_size" => pagination.page_size = Some(parse_page_size(value)?),
            "cursor_param" => {
                pagination.cursor_param = Some(non_empty_string(
                    value,
                    "source manifest pagination cursor_param",
                )?);
            }
            "cursor_body_path" => {
                pagination.cursor_body_path =
                    string_array(value, "source manifest pagination cursor_body_path")?;
            }
            "response_cursor_path" => {
                pagination.response_cursor_path =
                    string_array(value, "source manifest pagination response_cursor_path")?;
            }
            "page_param" => {
                pagination.page_param = Some(non_empty_string(
                    value,
                    "source manifest pagination page_param",
                )?);
            }
            "page_start" => {
                pagination.page_start = i64_number(value, "source manifest pagination page_start")?;
            }
            "page_step" => {
                pagination.page_step =
                    Some(i64_number(value, "source manifest pagination page_step")?);
            }
            "offset_param" => {
                pagination.offset_param = Some(non_empty_string(
                    value,
                    "source manifest pagination offset_param",
                )?);
            }
            "offset_start" => {
                pagination.offset_start =
                    i64_number(value, "source manifest pagination offset_start")?;
            }
            "offset_step" => {
                pagination.offset_step =
                    Some(i64_number(value, "source manifest pagination offset_step")?);
            }
            "link_header_require_results" => {
                pagination.link_header_require_results = bool_value(
                    value,
                    "source manifest pagination link_header_require_results",
                )?;
            }
            "max_pages" => {
                pagination.max_pages =
                    Some(u64_number(value, "source manifest pagination max_pages")?);
            }
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest pagination has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(pagination)
}

fn parse_page_size(value: Value) -> Result<specv1::PageSizeSpec> {
    let mut map = object(value, "source manifest page_size")?;
    let mut page_size = specv1::PageSizeSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "default" => {
                page_size.default_size = u64_number(value, "source manifest page_size default")?;
            }
            "max" => page_size.max = u64_number(value, "source manifest page_size max")?,
            "query_param" => {
                page_size.query_param = Some(non_empty_string(
                    value,
                    "source manifest page_size query_param",
                )?);
            }
            "body_path" => {
                page_size.body_path = string_array(value, "source manifest page_size body_path")?;
            }
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest page_size has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(page_size)
}

fn parse_file_source(value: Value) -> Result<specv1::FileSourceSpec> {
    let mut map = object(value, "source manifest file source")?;
    let mut source = specv1::FileSourceSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "location" => source.location = string(value, "source manifest source location")?,
            "glob" => {
                source.glob = Some(non_empty_string(value, "source manifest source glob")?);
            }
            "partitions" => source.partitions = parse_partitions(value)?,
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest source has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(source)
}

fn parse_partitions(value: Value) -> Result<Vec<specv1::PartitionColumnSpec>> {
    array(value, "source manifest partitions")?
        .into_iter()
        .map(parse_partition)
        .collect()
}

fn parse_partition(value: Value) -> Result<specv1::PartitionColumnSpec> {
    let mut map = object(value, "source manifest partition")?;
    let mut partition = specv1::PartitionColumnSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "name" => partition.name = string(value, "source manifest partition name")?,
            "type" => partition.data_type = string(value, "source manifest partition type")?,
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest partition has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(partition)
}

fn parse_columns(value: Value) -> Result<Vec<specv1::ColumnSpec>> {
    array(value, "source manifest columns")?
        .into_iter()
        .map(parse_column)
        .collect()
}

fn parse_column(value: Value) -> Result<specv1::ColumnSpec> {
    let mut map = object(value, "source manifest column")?;
    let mut column = specv1::ColumnSpec::default();
    while let Some((key, value)) = pop_first(&mut map) {
        match key.as_str() {
            "name" => column.name = string(value, "source manifest column name")?,
            "type" => column.data_type = string(value, "source manifest column type")?,
            "nullable" => {
                column.nullable = Some(bool_value(value, "source manifest column nullable")?);
            }
            "virtual" => column.r#virtual = bool_value(value, "source manifest column virtual")?,
            "description" => {
                column.description = string(value, "source manifest column description")?;
            }
            "expr" => column.expr = Some(parse_expr(value)?),
            other => {
                return Err(ManifestError::validation(format!(
                    "source manifest column has unknown field '{other}'"
                )));
            }
        }
    }
    Ok(column)
}

#[allow(
    clippy::too_many_lines,
    reason = "Expression conversion is a direct exhaustiveness mapping from the source manifest proto."
)]
fn parse_expr(value: Value) -> Result<specv1::ExprSpec> {
    let mut map = object(value, "source manifest expression")?;
    let Some(kind) = map.remove("kind") else {
        return Err(ManifestError::validation(
            "source manifest expression is missing kind",
        ));
    };
    let kind = string(kind, "source manifest expression kind")?;
    let proto_kind = match kind.as_str() {
        "path" => specv1::expr_spec::Kind::Path(specv1::PathExpr {
            path: required_string_array_field(&mut map, "path", "source manifest path expression")?,
        }),
        "coalesce" => specv1::expr_spec::Kind::Coalesce(specv1::CoalesceExpr {
            exprs: required_expr_array_field(
                &mut map,
                "exprs",
                "source manifest coalesce expression",
            )?,
        }),
        "from_filter" => specv1::expr_spec::Kind::FromFilter(specv1::FromFilterExpr {
            key: required_string_field(&mut map, "key", "source manifest from_filter expression")?,
        }),
        "literal" => {
            let Some(value) = map.remove("value") else {
                return Err(ManifestError::validation(
                    "source manifest literal expression is missing value",
                ));
            };
            specv1::expr_spec::Kind::Literal(specv1::LiteralExpr {
                json: json_string(&value)?,
            })
        }
        "null" => specv1::expr_spec::Kind::NullValue(specv1::NullExpr {}),
        "join_array" => {
            let path = required_string_array_field(
                &mut map,
                "path",
                "source manifest join_array expression",
            )?;
            let separator = map
                .remove("separator")
                .map(|value| string(value, "source manifest join_array separator"))
                .transpose()?;
            specv1::expr_spec::Kind::JoinArray(specv1::JoinArrayExpr { path, separator })
        }
        "tag_value" => {
            let path = required_string_array_field(
                &mut map,
                "path",
                "source manifest tag_value expression",
            )?;
            let key =
                required_string_field(&mut map, "key", "source manifest tag_value expression")?;
            let key_field = map
                .remove("key_field")
                .map(|value| string(value, "source manifest tag_value key_field"))
                .transpose()?;
            let value_field = map
                .remove("value_field")
                .map(|value| string(value, "source manifest tag_value value_field"))
                .transpose()?;
            specv1::expr_spec::Kind::TagValue(specv1::TagValueExpr {
                path,
                key,
                key_field,
                value_field,
            })
        }
        "if_present" => {
            let Some(check) = map.remove("check") else {
                return Err(ManifestError::validation(
                    "source manifest if_present expression is missing check",
                ));
            };
            let then_value = required_string_field(
                &mut map,
                "then_value",
                "source manifest if_present expression",
            )?;
            specv1::expr_spec::Kind::IfPresent(Box::new(specv1::IfPresentExpr {
                check: Some(Box::new(parse_expr(check)?)),
                then_value,
            }))
        }
        "join_tag_values" => {
            let path = required_string_array_field(
                &mut map,
                "path",
                "source manifest join_tag_values expression",
            )?;
            let key = required_string_field(
                &mut map,
                "key",
                "source manifest join_tag_values expression",
            )?;
            let key_field = map
                .remove("key_field")
                .map(|value| string(value, "source manifest join_tag_values key_field"))
                .transpose()?;
            let value_field = map
                .remove("value_field")
                .map(|value| string(value, "source manifest join_tag_values value_field"))
                .transpose()?;
            let separator = map
                .remove("separator")
                .map(|value| string(value, "source manifest join_tag_values separator"))
                .transpose()?;
            specv1::expr_spec::Kind::JoinTagValues(specv1::JoinTagValuesExpr {
                path,
                key,
                key_field,
                value_field,
                separator,
            })
        }
        "first_array_item_path" => {
            let path = required_string_array_field(
                &mut map,
                "path",
                "source manifest first_array_item_path expression",
            )?;
            let item_path = required_string_array_field(
                &mut map,
                "item_path",
                "source manifest first_array_item_path expression",
            )?;
            specv1::expr_spec::Kind::FirstArrayItemPath(specv1::FirstArrayItemPathExpr {
                path,
                item_path,
            })
        }
        "object_filter_path" => {
            let path = required_string_array_field(
                &mut map,
                "path",
                "source manifest object_filter_path expression",
            )?;
            let filter_key = required_string_field(
                &mut map,
                "filter_key",
                "source manifest object_filter_path expression",
            )?;
            let item_path = required_string_array_field(
                &mut map,
                "item_path",
                "source manifest object_filter_path expression",
            )?;
            specv1::expr_spec::Kind::ObjectFilterPath(specv1::ObjectFilterPathExpr {
                path,
                filter_key,
                item_path,
            })
        }
        "current_row" => specv1::expr_spec::Kind::CurrentRow(specv1::CurrentRowExpr {}),
        "format_timestamp" => {
            let Some(expr) = map.remove("expr") else {
                return Err(ManifestError::validation(
                    "source manifest format_timestamp expression is missing expr",
                ));
            };
            let input = match map.remove("input") {
                Some(value) => {
                    match string(value, "source manifest format_timestamp input")?.as_str() {
                        "seconds" => specv1::TimestampInput::Seconds,
                        "milliseconds" => specv1::TimestampInput::Milliseconds,
                        other => {
                            return Err(ManifestError::validation(format!(
                                "source manifest format_timestamp input '{other}' is unsupported"
                            )));
                        }
                    }
                }
                None => specv1::TimestampInput::Seconds,
            };
            specv1::expr_spec::Kind::FormatTimestamp(Box::new(specv1::FormatTimestampExpr {
                expr: Some(Box::new(parse_expr(expr)?)),
                input: input as i32,
            }))
        }
        "replace" => {
            let Some(expr) = map.remove("expr") else {
                return Err(ManifestError::validation(
                    "source manifest replace expression is missing expr",
                ));
            };
            let from =
                required_string_field(&mut map, "from", "source manifest replace expression")?;
            let to = required_string_field(&mut map, "to", "source manifest replace expression")?;
            specv1::expr_spec::Kind::Replace(Box::new(specv1::ReplaceExpr {
                expr: Some(Box::new(parse_expr(expr)?)),
                from,
                to,
            }))
        }
        "template" => {
            let template =
                required_string_field(&mut map, "template", "source manifest template expression")?;
            let values = match map.remove("values") {
                Some(value) => parse_expr_map(value, "source manifest template expression values")?,
                None => {
                    return Err(ManifestError::validation(
                        "source manifest template expression is missing values",
                    ));
                }
            };
            specv1::expr_spec::Kind::Template(specv1::TemplateExpr { template, values })
        }
        other => {
            return Err(ManifestError::validation(format!(
                "source manifest expression kind '{other}' is unsupported"
            )));
        }
    };
    reject_unknown(&map, "source manifest expression")?;
    Ok(specv1::ExprSpec {
        kind: Some(proto_kind),
    })
}

fn parse_expr_map(
    value: Value,
    context: &str,
) -> Result<std::collections::HashMap<String, specv1::ExprSpec>> {
    object(value, context)?
        .into_iter()
        .map(|(key, value)| parse_expr(value).map(|expr| (key, expr)))
        .collect()
}

fn inputs_to_value(inputs: &[specv1::SourceInputBinding]) -> Result<Value> {
    let mut object = Map::new();
    for input in inputs {
        let spec = input.input.as_ref().ok_or_else(|| {
            ManifestError::validation(format!(
                "source manifest input '{}' is missing input spec",
                input.key
            ))
        })?;
        let mut input_object = Map::new();
        input_object.insert(
            "kind".to_string(),
            Value::String(input_kind_to_manifest_value(source_input_kind(spec.kind)?).to_string()),
        );
        if let Some(default_value) = &spec.default_value {
            input_object.insert("default".to_string(), Value::String(default_value.clone()));
        }
        if let Some(required) = spec.required {
            input_object.insert("required".to_string(), Value::Bool(required));
        }
        if !spec.hint.is_empty() {
            input_object.insert("hint".to_string(), Value::String(spec.hint.clone()));
        }
        object.insert(input.key.clone(), Value::Object(input_object));
    }
    Ok(Value::Object(object))
}

fn auth_to_value(auth: &specv1::AuthSpec) -> Result<Value> {
    let mut object = Map::new();
    if !auth.headers.is_empty() {
        object.insert(
            "headers".to_string(),
            Value::Array(
                auth.headers
                    .iter()
                    .map(header_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    Ok(Value::Object(object))
}

fn rate_limit_to_value(rate_limit: &specv1::RateLimitSpec) -> Value {
    let mut object = Map::new();
    if !rate_limit.extra_statuses.is_empty() {
        object.insert(
            "extra_statuses".to_string(),
            Value::Array(
                rate_limit
                    .extra_statuses
                    .iter()
                    .map(|status| json_u64(u64::from(*status)))
                    .collect(),
            ),
        );
    }
    if let Some(header) = &rate_limit.retry_after_header {
        object.insert(
            "retry_after_header".to_string(),
            Value::String(header.clone()),
        );
    }
    if let Some(header) = &rate_limit.remaining_header {
        object.insert(
            "remaining_header".to_string(),
            Value::String(header.clone()),
        );
    }
    if let Some(header) = &rate_limit.reset_header {
        object.insert("reset_header".to_string(), Value::String(header.clone()));
    }
    Value::Object(object)
}

fn table_to_value(table: &specv1::TableSpec) -> Result<Value> {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(table.name.clone()));
    object.insert(
        "description".to_string(),
        Value::String(table.description.clone()),
    );
    if !table.guide.is_empty() {
        object.insert("guide".to_string(), Value::String(table.guide.clone()));
    }
    if !table.filters.is_empty() {
        object.insert(
            "filters".to_string(),
            Value::Array(
                table
                    .filters
                    .iter()
                    .map(filter_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    if let Some(limit) = table.fetch_limit_default {
        object.insert("fetch_limit_default".to_string(), json_u64(limit));
    }
    if let Some(request) = &table.request {
        object.insert("request".to_string(), request_to_value(request)?);
    }
    if !table.requests.is_empty() {
        object.insert(
            "requests".to_string(),
            Value::Array(
                table
                    .requests
                    .iter()
                    .map(request_route_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    if let Some(response) = &table.response {
        let value = response_to_value(response)?;
        if !value.as_object().is_some_and(Map::is_empty) {
            object.insert("response".to_string(), value);
        }
    }
    if let Some(pagination) = &table.pagination {
        let value = pagination_to_value(pagination)?;
        if !value.as_object().is_some_and(Map::is_empty) {
            object.insert("pagination".to_string(), value);
        }
    }
    if let Some(source) = &table.source {
        object.insert("source".to_string(), file_source_to_value(source));
    }
    if !table.columns.is_empty() {
        object.insert(
            "columns".to_string(),
            Value::Array(
                table
                    .columns
                    .iter()
                    .map(column_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    Ok(Value::Object(object))
}

fn filter_to_value(filter: &specv1::FilterSpec) -> Result<Value> {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(filter.name.clone()));
    if filter.required {
        object.insert("required".to_string(), Value::Bool(true));
    }
    let mode = filter_mode(filter.mode)?;
    if mode != specv1::FilterMode::Equality {
        object.insert(
            "mode".to_string(),
            Value::String(filter_mode_to_manifest_value(mode).to_string()),
        );
    }
    Ok(Value::Object(object))
}

fn request_route_to_value(route: &specv1::RequestRouteSpec) -> Result<Value> {
    let mut object = match route.request.as_ref() {
        Some(request) => request_to_value(request)?,
        None => Value::Object(Map::new()),
    };
    let object = object
        .as_object_mut()
        .expect("request_to_value must return object");
    object.insert(
        "when_filters".to_string(),
        string_array_value(&route.when_filters),
    );
    Ok(Value::Object(object.clone()))
}

fn request_to_value(request: &specv1::RequestSpec) -> Result<Value> {
    let mut object = Map::new();
    let method = http_method(request.method)?;
    if method == specv1::HttpMethod::Post {
        object.insert("method".to_string(), Value::String("POST".to_string()));
    }
    if !request.path.is_empty() {
        object.insert("path".to_string(), Value::String(request.path.clone()));
    }
    if !request.query.is_empty() {
        object.insert(
            "query".to_string(),
            Value::Array(
                request
                    .query
                    .iter()
                    .map(query_param_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    if !request.body.is_empty() {
        object.insert(
            "body".to_string(),
            Value::Array(
                request
                    .body
                    .iter()
                    .map(body_field_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    if !request.headers.is_empty() {
        object.insert(
            "headers".to_string(),
            Value::Array(
                request
                    .headers
                    .iter()
                    .map(header_to_value)
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    Ok(Value::Object(object))
}

fn query_param_to_value(param: &specv1::QueryParamSpec) -> Result<Value> {
    let mut object = value_source_to_object(param.value.as_ref(), "query param")?;
    object.insert("name".to_string(), Value::String(param.name.clone()));
    if let Some(explode) = param.explode {
        object.insert("explode".to_string(), Value::Bool(explode));
    }
    Ok(Value::Object(object))
}

fn body_field_to_value(field: &specv1::BodyFieldSpec) -> Result<Value> {
    let mut object = value_source_to_object(field.value.as_ref(), "body field")?;
    object.insert("path".to_string(), string_array_value(&field.path));
    Ok(Value::Object(object))
}

fn header_to_value(header: &specv1::HeaderSpec) -> Result<Value> {
    let mut object = value_source_to_object(header.value.as_ref(), "header")?;
    object.insert("name".to_string(), Value::String(header.name.clone()));
    Ok(Value::Object(object))
}

fn value_source_to_object(
    source: Option<&specv1::ValueSource>,
    context: &str,
) -> Result<Map<String, Value>> {
    let source = source.ok_or_else(|| {
        ManifestError::validation(format!("source manifest {context} is missing value source"))
    })?;
    let kind = source.kind.as_ref().ok_or_else(|| {
        ManifestError::validation(format!(
            "source manifest {context} is missing value source kind"
        ))
    })?;
    let mut object = Map::new();
    match kind {
        specv1::value_source::Kind::Template(value) => {
            object.insert("from".to_string(), Value::String("template".to_string()));
            object.insert(
                "template".to_string(),
                Value::String(value.template.clone()),
            );
        }
        specv1::value_source::Kind::Literal(value) => {
            object.insert("from".to_string(), Value::String("literal".to_string()));
            object.insert("value".to_string(), json_value(&value.json)?);
        }
        specv1::value_source::Kind::Filter(value) => {
            object.insert("from".to_string(), Value::String("filter".to_string()));
            object.insert("key".to_string(), Value::String(value.key.clone()));
            if let Some(default_json) = &value.default_json {
                object.insert("default".to_string(), json_value(default_json)?);
            }
        }
        specv1::value_source::Kind::FilterInt(value) => {
            object.insert("from".to_string(), Value::String("filter_int".to_string()));
            object.insert("key".to_string(), Value::String(value.key.clone()));
            if let Some(default_value) = value.default_value {
                object.insert("default".to_string(), json_i64(default_value));
            }
        }
        specv1::value_source::Kind::Input(value) => {
            object.insert("from".to_string(), Value::String("input".to_string()));
            object.insert("key".to_string(), Value::String(value.key.clone()));
        }
        specv1::value_source::Kind::State(value) => {
            object.insert("from".to_string(), Value::String("state".to_string()));
            object.insert("key".to_string(), Value::String(value.key.clone()));
        }
        specv1::value_source::Kind::NowEpochMinusSeconds(value) => {
            object.insert(
                "from".to_string(),
                Value::String("now_epoch_minus_seconds".to_string()),
            );
            object.insert("seconds".to_string(), json_i64(value.seconds));
        }
    }
    Ok(object)
}

fn response_to_value(response: &specv1::ResponseSpec) -> Result<Value> {
    let mut object = Map::new();
    if !response.rows_path.is_empty() {
        object.insert(
            "rows_path".to_string(),
            string_array_value(&response.rows_path),
        );
    }
    if !response.ok_path.is_empty() {
        object.insert("ok_path".to_string(), string_array_value(&response.ok_path));
    }
    if !response.error_path.is_empty() {
        object.insert(
            "error_path".to_string(),
            string_array_value(&response.error_path),
        );
    }
    if response.allow_404_empty {
        object.insert("allow_404_empty".to_string(), Value::Bool(true));
    }
    let row_strategy = row_strategy(response.row_strategy)?;
    if row_strategy != specv1::RowStrategy::Direct {
        object.insert(
            "row_strategy".to_string(),
            Value::String(row_strategy_to_manifest_value(row_strategy).to_string()),
        );
    }
    Ok(Value::Object(object))
}

fn pagination_to_value(pagination: &specv1::PaginationSpec) -> Result<Value> {
    let mut object = Map::new();
    let mode = pagination_mode(pagination.mode)?;
    if mode != specv1::PaginationMode::None {
        object.insert(
            "mode".to_string(),
            Value::String(pagination_mode_to_manifest_value(mode).to_string()),
        );
    }
    if let Some(page_size) = &pagination.page_size {
        object.insert("page_size".to_string(), page_size_to_value(page_size));
    }
    if let Some(value) = &pagination.cursor_param {
        object.insert("cursor_param".to_string(), Value::String(value.clone()));
    }
    if !pagination.cursor_body_path.is_empty() {
        object.insert(
            "cursor_body_path".to_string(),
            string_array_value(&pagination.cursor_body_path),
        );
    }
    if !pagination.response_cursor_path.is_empty() {
        object.insert(
            "response_cursor_path".to_string(),
            string_array_value(&pagination.response_cursor_path),
        );
    }
    if let Some(value) = &pagination.page_param {
        object.insert("page_param".to_string(), Value::String(value.clone()));
    }
    if pagination.page_start != 0 {
        object.insert("page_start".to_string(), json_i64(pagination.page_start));
    }
    if let Some(value) = pagination.page_step {
        object.insert("page_step".to_string(), json_i64(value));
    }
    if let Some(value) = &pagination.offset_param {
        object.insert("offset_param".to_string(), Value::String(value.clone()));
    }
    if pagination.offset_start != 0 {
        object.insert(
            "offset_start".to_string(),
            json_i64(pagination.offset_start),
        );
    }
    if let Some(value) = pagination.offset_step {
        object.insert("offset_step".to_string(), json_i64(value));
    }
    if pagination.link_header_require_results {
        object.insert("link_header_require_results".to_string(), Value::Bool(true));
    }
    if let Some(value) = pagination.max_pages {
        object.insert("max_pages".to_string(), json_u64(value));
    }
    Ok(Value::Object(object))
}

fn page_size_to_value(page_size: &specv1::PageSizeSpec) -> Value {
    let mut object = Map::new();
    object.insert("default".to_string(), json_u64(page_size.default_size));
    object.insert("max".to_string(), json_u64(page_size.max));
    if let Some(value) = &page_size.query_param {
        object.insert("query_param".to_string(), Value::String(value.clone()));
    }
    if !page_size.body_path.is_empty() {
        object.insert(
            "body_path".to_string(),
            string_array_value(&page_size.body_path),
        );
    }
    Value::Object(object)
}

fn file_source_to_value(source: &specv1::FileSourceSpec) -> Value {
    let mut object = Map::new();
    object.insert(
        "location".to_string(),
        Value::String(source.location.clone()),
    );
    if let Some(glob) = &source.glob {
        object.insert("glob".to_string(), Value::String(glob.clone()));
    }
    if !source.partitions.is_empty() {
        object.insert(
            "partitions".to_string(),
            Value::Array(source.partitions.iter().map(partition_to_value).collect()),
        );
    }
    Value::Object(object)
}

fn partition_to_value(partition: &specv1::PartitionColumnSpec) -> Value {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(partition.name.clone()));
    object.insert(
        "type".to_string(),
        Value::String(partition.data_type.clone()),
    );
    Value::Object(object)
}

fn column_to_value(column: &specv1::ColumnSpec) -> Result<Value> {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(column.name.clone()));
    object.insert("type".to_string(), Value::String(column.data_type.clone()));
    if let Some(nullable) = column.nullable {
        object.insert("nullable".to_string(), Value::Bool(nullable));
    }
    if column.r#virtual {
        object.insert("virtual".to_string(), Value::Bool(true));
    }
    if !column.description.is_empty() {
        object.insert(
            "description".to_string(),
            Value::String(column.description.clone()),
        );
    }
    if let Some(expr) = &column.expr {
        object.insert("expr".to_string(), expr_to_value(expr)?);
    }
    Ok(Value::Object(object))
}

#[allow(
    clippy::too_many_lines,
    reason = "Expression conversion is a direct exhaustiveness mapping to the legacy backend validator input."
)]
fn expr_to_value(expr: &specv1::ExprSpec) -> Result<Value> {
    let kind = expr
        .kind
        .as_ref()
        .ok_or_else(|| ManifestError::validation("source manifest expression is missing kind"))?;
    let mut object = Map::new();
    match kind {
        specv1::expr_spec::Kind::Path(expr) => {
            object.insert("kind".to_string(), Value::String("path".to_string()));
            object.insert("path".to_string(), string_array_value(&expr.path));
        }
        specv1::expr_spec::Kind::Coalesce(expr) => {
            object.insert("kind".to_string(), Value::String("coalesce".to_string()));
            object.insert(
                "exprs".to_string(),
                Value::Array(
                    expr.exprs
                        .iter()
                        .map(expr_to_value)
                        .collect::<Result<Vec<_>>>()?,
                ),
            );
        }
        specv1::expr_spec::Kind::FromFilter(expr) => {
            object.insert("kind".to_string(), Value::String("from_filter".to_string()));
            object.insert("key".to_string(), Value::String(expr.key.clone()));
        }
        specv1::expr_spec::Kind::Literal(expr) => {
            object.insert("kind".to_string(), Value::String("literal".to_string()));
            object.insert("value".to_string(), json_value(&expr.json)?);
        }
        specv1::expr_spec::Kind::NullValue(_) => {
            object.insert("kind".to_string(), Value::String("null".to_string()));
        }
        specv1::expr_spec::Kind::JoinArray(expr) => {
            object.insert("kind".to_string(), Value::String("join_array".to_string()));
            object.insert("path".to_string(), string_array_value(&expr.path));
            if let Some(separator) = &expr.separator {
                object.insert("separator".to_string(), Value::String(separator.clone()));
            }
        }
        specv1::expr_spec::Kind::TagValue(expr) => {
            object.insert("kind".to_string(), Value::String("tag_value".to_string()));
            object.insert("path".to_string(), string_array_value(&expr.path));
            object.insert("key".to_string(), Value::String(expr.key.clone()));
            if let Some(value) = &expr.key_field {
                object.insert("key_field".to_string(), Value::String(value.clone()));
            }
            if let Some(value) = &expr.value_field {
                object.insert("value_field".to_string(), Value::String(value.clone()));
            }
        }
        specv1::expr_spec::Kind::IfPresent(expr) => {
            object.insert("kind".to_string(), Value::String("if_present".to_string()));
            object.insert(
                "check".to_string(),
                expr_to_value(required_expr(expr.check.as_deref(), "if_present check")?)?,
            );
            object.insert(
                "then_value".to_string(),
                Value::String(expr.then_value.clone()),
            );
        }
        specv1::expr_spec::Kind::JoinTagValues(expr) => {
            object.insert(
                "kind".to_string(),
                Value::String("join_tag_values".to_string()),
            );
            object.insert("path".to_string(), string_array_value(&expr.path));
            object.insert("key".to_string(), Value::String(expr.key.clone()));
            if let Some(value) = &expr.key_field {
                object.insert("key_field".to_string(), Value::String(value.clone()));
            }
            if let Some(value) = &expr.value_field {
                object.insert("value_field".to_string(), Value::String(value.clone()));
            }
            if let Some(value) = &expr.separator {
                object.insert("separator".to_string(), Value::String(value.clone()));
            }
        }
        specv1::expr_spec::Kind::FirstArrayItemPath(expr) => {
            object.insert(
                "kind".to_string(),
                Value::String("first_array_item_path".to_string()),
            );
            object.insert("path".to_string(), string_array_value(&expr.path));
            object.insert("item_path".to_string(), string_array_value(&expr.item_path));
        }
        specv1::expr_spec::Kind::ObjectFilterPath(expr) => {
            object.insert(
                "kind".to_string(),
                Value::String("object_filter_path".to_string()),
            );
            object.insert("path".to_string(), string_array_value(&expr.path));
            object.insert(
                "filter_key".to_string(),
                Value::String(expr.filter_key.clone()),
            );
            object.insert("item_path".to_string(), string_array_value(&expr.item_path));
        }
        specv1::expr_spec::Kind::CurrentRow(_) => {
            object.insert("kind".to_string(), Value::String("current_row".to_string()));
        }
        specv1::expr_spec::Kind::FormatTimestamp(expr) => {
            object.insert(
                "kind".to_string(),
                Value::String("format_timestamp".to_string()),
            );
            object.insert(
                "expr".to_string(),
                expr_to_value(required_expr(
                    expr.expr.as_deref(),
                    "format_timestamp expr",
                )?)?,
            );
            let input = timestamp_input(expr.input)?;
            if input != specv1::TimestampInput::Seconds {
                object.insert(
                    "input".to_string(),
                    Value::String(timestamp_input_to_manifest_value(input).to_string()),
                );
            }
        }
        specv1::expr_spec::Kind::Replace(expr) => {
            object.insert("kind".to_string(), Value::String("replace".to_string()));
            object.insert(
                "expr".to_string(),
                expr_to_value(required_expr(expr.expr.as_deref(), "replace expr")?)?,
            );
            object.insert("from".to_string(), Value::String(expr.from.clone()));
            object.insert("to".to_string(), Value::String(expr.to.clone()));
        }
        specv1::expr_spec::Kind::Template(expr) => {
            object.insert("kind".to_string(), Value::String("template".to_string()));
            object.insert("template".to_string(), Value::String(expr.template.clone()));
            let mut values = Map::new();
            for (key, value) in &expr.values {
                values.insert(key.clone(), expr_to_value(value)?);
            }
            object.insert("values".to_string(), Value::Object(values));
        }
    }
    Ok(Value::Object(object))
}

fn required_expr<'a>(
    expr: Option<&'a specv1::ExprSpec>,
    context: &str,
) -> Result<&'a specv1::ExprSpec> {
    expr.ok_or_else(|| {
        ManifestError::validation(format!("source manifest expression is missing {context}"))
    })
}

fn source_backend(value: i32) -> Result<specv1::SourceBackend> {
    specv1::SourceBackend::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest backend enum value {value} is invalid"
        ))
    })
}

fn source_input_kind(value: i32) -> Result<specv1::SourceInputKind> {
    specv1::SourceInputKind::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest input kind enum value {value} is invalid"
        ))
    })
}

fn filter_mode(value: i32) -> Result<specv1::FilterMode> {
    specv1::FilterMode::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest filter mode enum value {value} is invalid"
        ))
    })
}

fn http_method(value: i32) -> Result<specv1::HttpMethod> {
    specv1::HttpMethod::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest HTTP method enum value {value} is invalid"
        ))
    })
}

fn row_strategy(value: i32) -> Result<specv1::RowStrategy> {
    specv1::RowStrategy::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest row strategy enum value {value} is invalid"
        ))
    })
}

fn pagination_mode(value: i32) -> Result<specv1::PaginationMode> {
    specv1::PaginationMode::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest pagination mode enum value {value} is invalid"
        ))
    })
}

fn timestamp_input(value: i32) -> Result<specv1::TimestampInput> {
    specv1::TimestampInput::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest timestamp input enum value {value} is invalid"
        ))
    })
}

fn backend_to_manifest_value(value: specv1::SourceBackend) -> &'static str {
    match value {
        specv1::SourceBackend::Unspecified => "",
        specv1::SourceBackend::Http => "http",
        specv1::SourceBackend::Parquet => "parquet",
        specv1::SourceBackend::Jsonl => "jsonl",
    }
}

fn input_kind_to_manifest_value(value: specv1::SourceInputKind) -> &'static str {
    match value {
        specv1::SourceInputKind::Unspecified => "",
        specv1::SourceInputKind::Variable => "variable",
        specv1::SourceInputKind::Secret => "secret",
    }
}

fn filter_mode_to_manifest_value(value: specv1::FilterMode) -> &'static str {
    match value {
        specv1::FilterMode::Unspecified | specv1::FilterMode::Equality => "equality",
        specv1::FilterMode::Search => "search",
        specv1::FilterMode::Contains => "contains",
    }
}

fn row_strategy_to_manifest_value(value: specv1::RowStrategy) -> &'static str {
    match value {
        specv1::RowStrategy::Unspecified | specv1::RowStrategy::Direct => "direct",
        specv1::RowStrategy::SeriesPointList => "series_point_list",
        specv1::RowStrategy::DictEntries => "dict_entries",
    }
}

fn pagination_mode_to_manifest_value(value: specv1::PaginationMode) -> &'static str {
    match value {
        specv1::PaginationMode::Unspecified | specv1::PaginationMode::None => "none",
        specv1::PaginationMode::Auto => "auto",
        specv1::PaginationMode::CursorQuery => "cursor_query",
        specv1::PaginationMode::CursorBody => "cursor_body",
        specv1::PaginationMode::Page => "page",
        specv1::PaginationMode::Offset => "offset",
        specv1::PaginationMode::LinkHeader => "link_header",
    }
}

fn timestamp_input_to_manifest_value(value: specv1::TimestampInput) -> &'static str {
    match value {
        specv1::TimestampInput::Unspecified | specv1::TimestampInput::Seconds => "seconds",
        specv1::TimestampInput::Milliseconds => "milliseconds",
    }
}

fn object(value: Value, context: &str) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(ManifestError::validation(format!(
            "{context} must be a mapping"
        ))),
    }
}

fn array(value: Value, context: &str) -> Result<Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items),
        _ => Err(ManifestError::validation(format!(
            "{context} must be a list"
        ))),
    }
}

fn string(value: Value, context: &str) -> Result<String> {
    match value {
        Value::String(value) => Ok(value),
        _ => Err(ManifestError::validation(format!(
            "{context} must be a string"
        ))),
    }
}

fn non_empty_string(value: Value, context: &str) -> Result<String> {
    let value = string(value, context)?;
    if value.is_empty() {
        return Err(ManifestError::validation(format!(
            "{context} must not be empty"
        )));
    }
    Ok(value)
}

fn ensure_non_empty(value: &str, context: &str) -> Result<()> {
    if value.is_empty() {
        return Err(ManifestError::validation(format!(
            "{context} must not be empty"
        )));
    }
    Ok(())
}

fn ensure_non_empty_items(values: &[String], context: &str) -> Result<()> {
    for (index, value) in values.iter().enumerate() {
        if value.is_empty() {
            return Err(ManifestError::validation(format!(
                "{context}[{index}] must not be empty"
            )));
        }
    }
    Ok(())
}

fn ensure_positive_i64(value: i64, context: &str) -> Result<()> {
    if value <= 0 {
        return Err(ManifestError::validation(format!("{context} must be > 0")));
    }
    Ok(())
}

fn ensure_positive_u64(value: u64, context: &str) -> Result<()> {
    if value == 0 {
        return Err(ManifestError::validation(format!("{context} must be > 0")));
    }
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Parser helpers uniformly consume serde_json::Value while draining manifest maps."
)]
fn bool_value(value: Value, context: &str) -> Result<bool> {
    match value {
        Value::Bool(value) => Ok(value),
        _ => Err(ManifestError::validation(format!(
            "{context} must be a boolean"
        ))),
    }
}

fn i64_number(value: Value, context: &str) -> Result<i64> {
    match value {
        Value::Number(value) => value.as_i64().ok_or_else(|| {
            ManifestError::validation(format!("{context} must fit in a signed 64-bit integer"))
        }),
        _ => Err(ManifestError::validation(format!(
            "{context} must be an integer"
        ))),
    }
}

fn u64_number(value: Value, context: &str) -> Result<u64> {
    match value {
        Value::Number(value) => value.as_u64().ok_or_else(|| {
            ManifestError::validation(format!("{context} must fit in an unsigned 64-bit integer"))
        }),
        _ => Err(ManifestError::validation(format!(
            "{context} must be an integer"
        ))),
    }
}

fn u32_number(value: Value, context: &str) -> Result<u32> {
    let value = u64_number(value, context)?;
    u32::try_from(value).map_err(|_| {
        ManifestError::validation(format!("{context} must fit in an unsigned 32-bit integer"))
    })
}

fn string_array(value: Value, context: &str) -> Result<Vec<String>> {
    array(value, context)?
        .into_iter()
        .map(|item| string(item, context))
        .collect()
}

fn u32_array(value: Value, context: &str) -> Result<Vec<u32>> {
    array(value, context)?
        .into_iter()
        .map(|item| u32_number(item, context))
        .collect()
}

fn json_string(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(ManifestError::deserialize)
}

fn json_value(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(ManifestError::deserialize)
}

fn json_u64(value: u64) -> Value {
    Value::Number(Number::from(value))
}

fn json_i64(value: i64) -> Value {
    Value::Number(Number::from(value))
}

fn string_array_value(values: &[String]) -> Value {
    Value::Array(values.iter().cloned().map(Value::String).collect())
}

fn pop_first(map: &mut Map<String, Value>) -> Option<(String, Value)> {
    let key = map.keys().next().cloned()?;
    let value = map.remove(&key)?;
    Some((key, value))
}

fn required_string_field(map: &mut Map<String, Value>, key: &str, context: &str) -> Result<String> {
    let Some(value) = map.remove(key) else {
        return Err(ManifestError::validation(format!(
            "{context} is missing {key}"
        )));
    };
    string(value, &format!("{context} {key}"))
}

fn required_string_array_field(
    map: &mut Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Vec<String>> {
    let Some(value) = map.remove(key) else {
        return Err(ManifestError::validation(format!(
            "{context} is missing {key}"
        )));
    };
    string_array(value, &format!("{context} {key}"))
}

fn required_expr_array_field(
    map: &mut Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Vec<specv1::ExprSpec>> {
    let Some(value) = map.remove(key) else {
        return Err(ManifestError::validation(format!(
            "{context} is missing {key}"
        )));
    };
    array(value, &format!("{context} {key}"))?
        .into_iter()
        .map(parse_expr)
        .collect()
}

fn reject_unknown(map: &Map<String, Value>, context: &str) -> Result<()> {
    if let Some(key) = map.keys().next() {
        return Err(ManifestError::validation(format!(
            "{context} has unknown field '{key}'"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        source_manifest_proto_from_yaml, source_manifest_proto_to_value,
        validate_source_manifest_proto,
    };
    use crate::proto::v1 as specv1;

    #[test]
    fn parses_legacy_source_manifest_into_proto() {
        let manifest = source_manifest_proto_from_yaml(
            r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
auth:
  headers:
    - name: Authorization
      from: template
      template: Bearer {{input.API_TOKEN}}
inputs:
  API_TOKEN:
    kind: secret
tables:
  - name: messages
    description: Demo messages
    request:
      method: GET
      path: /messages
    response: {}
    columns:
      - name: id
        type: Utf8
",
        )
        .expect("manifest should parse");

        assert_eq!(manifest.name, "demo");
        assert_eq!(manifest.backend, specv1::SourceBackend::Http as i32);
        assert_eq!(manifest.inputs[0].key, "API_TOKEN");
        assert_eq!(manifest.tables[0].name, "messages");
    }

    #[test]
    fn accepts_kind_wrapped_source_manifest() {
        let manifest = source_manifest_proto_from_yaml(
            r"
api_version: coral.withcoral.com/v1alpha1
kind: Source
name: demo
version: 1.0.0
dsl_version: 3
backend: jsonl
tables:
  - name: messages
    description: Demo messages
    source:
      location: file:///tmp/demo/
    columns:
      - name: kind
        type: Utf8
",
        )
        .expect("manifest should parse");

        assert_eq!(manifest.api_version, "coral.withcoral.com/v1alpha1");
        assert_eq!(manifest.kind, "Source");
        let value = source_manifest_proto_to_value(&manifest).expect("convert to value");
        assert!(value.get("kind").is_none());
        assert_eq!(value["backend"], "jsonl");
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let error = source_manifest_proto_from_yaml(
            r"
name: demo
schema: legacy
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://example.com
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
",
        )
        .expect_err("unknown field should fail");

        assert_eq!(
            error.to_string(),
            "source manifest has unknown field 'schema'"
        );
    }

    #[test]
    fn rejects_non_source_kind() {
        let manifest = specv1::SourceManifest {
            api_version: "coral.withcoral.com/v1alpha1".to_string(),
            kind: "Provider".to_string(),
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            dsl_version: 3,
            backend: specv1::SourceBackend::Http as i32,
            tables: vec![specv1::TableSpec {
                name: "messages".to_string(),
                description: "Demo messages".to_string(),
                ..specv1::TableSpec::default()
            }],
            ..specv1::SourceManifest::default()
        };

        let error =
            validate_source_manifest_proto(&manifest).expect_err("provider kind should fail");

        assert_eq!(
            error.to_string(),
            "source manifest kind 'Provider' is unsupported"
        );
    }

    #[test]
    fn rejects_empty_table_description_from_proto() {
        let manifest = specv1::SourceManifest {
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            dsl_version: 3,
            backend: specv1::SourceBackend::Jsonl as i32,
            tables: vec![specv1::TableSpec {
                name: "messages".to_string(),
                source: Some(specv1::FileSourceSpec {
                    location: "file:///tmp/demo/".to_string(),
                    ..specv1::FileSourceSpec::default()
                }),
                columns: vec![specv1::ColumnSpec {
                    name: "kind".to_string(),
                    data_type: "Utf8".to_string(),
                    ..specv1::ColumnSpec::default()
                }],
                ..specv1::TableSpec::default()
            }],
            ..specv1::SourceManifest::default()
        };

        let error =
            validate_source_manifest_proto(&manifest).expect_err("table description is required");

        assert_eq!(
            error.to_string(),
            "source manifest table description must not be empty"
        );
    }

    #[test]
    fn rejects_zero_pagination_limit_from_proto() {
        let manifest = specv1::SourceManifest {
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            dsl_version: 3,
            backend: specv1::SourceBackend::Http as i32,
            base_url: "https://example.com".to_string(),
            tables: vec![specv1::TableSpec {
                name: "messages".to_string(),
                description: "Demo messages".to_string(),
                request: Some(specv1::RequestSpec {
                    method: specv1::HttpMethod::Get as i32,
                    path: "/messages".to_string(),
                    ..specv1::RequestSpec::default()
                }),
                pagination: Some(specv1::PaginationSpec {
                    max_pages: Some(0),
                    ..specv1::PaginationSpec::default()
                }),
                columns: vec![specv1::ColumnSpec {
                    name: "id".to_string(),
                    data_type: "Utf8".to_string(),
                    ..specv1::ColumnSpec::default()
                }],
                ..specv1::TableSpec::default()
            }],
            ..specv1::SourceManifest::default()
        };

        let error =
            validate_source_manifest_proto(&manifest).expect_err("max_pages must be positive");

        assert_eq!(
            error.to_string(),
            "source manifest pagination max_pages must be > 0"
        );
    }
}
