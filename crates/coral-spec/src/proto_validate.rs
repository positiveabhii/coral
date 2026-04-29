//! Contract validation for generated source-manifest proto values.

use crate::common::parse_manifest_data_type;
use crate::proto::v1 as specv1;
use crate::{ManifestError, Result};

const SOURCE_API_VERSION: &str = "coral.withcoral.com/v1alpha1";

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
    validate_headers(&manifest.request_headers)?;
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
    let kind = auth.kind.as_ref().ok_or_else(|| {
        ManifestError::validation("source manifest auth must declare a kind (basic/header/custom)")
    })?;
    match kind {
        specv1::auth_spec::Kind::Basic(spec) => {
            ensure_non_empty(&spec.username, "source manifest auth basic username")?;
            ensure_non_empty(&spec.password, "source manifest auth basic password")?;
            Ok(())
        }
        specv1::auth_spec::Kind::Header(spec) => validate_headers(&spec.headers),
        specv1::auth_spec::Kind::Custom(spec) => {
            ensure_non_empty(
                &spec.authenticator,
                "source manifest auth custom authenticator",
            )?;
            if !spec.config_json.is_empty() {
                let value = json_value(&spec.config_json)?;
                if !value.is_object() {
                    return Err(ManifestError::validation(
                        "source manifest custom auth config must be an object",
                    ));
                }
            }
            Ok(())
        }
    }
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
    if let Some(body) = request.body.as_ref() {
        validate_body(body)?;
    }
    validate_headers(&request.headers)
}

fn validate_body(body: &specv1::BodySpec) -> Result<()> {
    let Some(shape) = body.shape.as_ref() else {
        return Ok(());
    };
    match shape {
        specv1::body_spec::Shape::Json(json) => {
            for field in &json.fields {
                if field.path.is_empty() {
                    return Err(ManifestError::validation(
                        "source manifest body field path must contain at least one item",
                    ));
                }
                ensure_non_empty_items(&field.path, "source manifest body field path")?;
                validate_value_source(field.value.as_ref(), "source manifest body field")?;
            }
        }
        specv1::body_spec::Shape::Text(text) => {
            validate_value_source(text.content.as_ref(), "source manifest body content")?;
        }
    }
    Ok(())
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
        specv1::value_source::Kind::FilterBool(value) => {
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
    let _ = response_body_format(response.format)?;
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
        specv1::expr_spec::Kind::Null(_) | specv1::expr_spec::Kind::CurrentRow(_) => {}
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

fn response_body_format(value: i32) -> Result<specv1::ResponseBodyFormat> {
    specv1::ResponseBodyFormat::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest response body format enum value {value} is invalid"
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

fn required_expr<'a>(
    expr: Option<&'a specv1::ExprSpec>,
    context: &str,
) -> Result<&'a specv1::ExprSpec> {
    expr.ok_or_else(|| {
        ManifestError::validation(format!("source manifest expression is missing {context}"))
    })
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

fn json_value(raw: &str) -> Result<serde_json::Value> {
    serde_json::from_str(raw).map_err(ManifestError::deserialize)
}

#[cfg(test)]
mod tests {
    use super::validate_source_manifest_proto;
    use crate::proto::v1 as specv1;

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
