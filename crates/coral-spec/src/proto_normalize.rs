//! Normalization from generated source-manifest proto into backend-owned models.

use std::collections::HashMap;

use serde_json::Value;

use crate::proto::v1 as specv1;
use crate::{
    AuthSpec, BodyFieldSpec, ColumnSpec, ExprSpec, FilterMode, FilterSpec, HeaderSpec, HttpMethod,
    ManifestError, PageSizeSpec, PaginationMode, PaginationSpec, ParsedTemplate, QueryParamSpec,
    RequestRouteSpec, RequestSpec, ResponseSpec, Result, RowStrategy, SourceManifestCommon,
    TableCommon, TimestampInput, ValueSourceSpec,
};

pub(crate) fn source_common_from_proto(manifest: &specv1::SourceManifest) -> SourceManifestCommon {
    SourceManifestCommon::new(
        manifest.dsl_version,
        manifest.name.clone(),
        manifest.version.clone(),
        manifest.description.clone(),
        manifest.test_queries.clone(),
    )
}

pub(crate) fn auth_from_proto(auth: Option<&specv1::AuthSpec>) -> Result<AuthSpec> {
    let Some(auth) = auth else {
        return Ok(AuthSpec::default());
    };
    Ok(AuthSpec {
        headers: headers_from_proto(&auth.headers)?,
    })
}

pub(crate) fn table_common_from_proto(table: &specv1::TableSpec) -> Result<TableCommon> {
    Ok(TableCommon::new(
        table.name.clone(),
        table.description.clone(),
        table.guide.clone(),
        filters_from_proto(&table.filters)?,
        table
            .fetch_limit_default
            .map(|limit| usize_from_u64(limit, "source manifest table fetch_limit_default"))
            .transpose()?,
        columns_from_proto(&table.columns)?,
    ))
}

pub(crate) fn request_from_proto(request: Option<&specv1::RequestSpec>) -> Result<RequestSpec> {
    match request {
        Some(request) => request_spec_from_proto(request),
        None => Ok(RequestSpec::default()),
    }
}

pub(crate) fn request_routes_from_proto(
    routes: &[specv1::RequestRouteSpec],
) -> Result<Vec<RequestRouteSpec>> {
    routes.iter().map(request_route_from_proto).collect()
}

pub(crate) fn response_from_proto(response: Option<&specv1::ResponseSpec>) -> Result<ResponseSpec> {
    let Some(response) = response else {
        return Ok(ResponseSpec::default());
    };
    Ok(ResponseSpec {
        rows_path: response.rows_path.clone(),
        ok_path: response.ok_path.clone(),
        error_path: response.error_path.clone(),
        allow_404_empty: response.allow_404_empty,
        row_strategy: row_strategy_from_proto(response.row_strategy)?,
    })
}

pub(crate) fn pagination_from_proto(
    pagination: Option<&specv1::PaginationSpec>,
) -> Result<PaginationSpec> {
    let Some(pagination) = pagination else {
        return Ok(PaginationSpec::default());
    };
    Ok(PaginationSpec {
        mode: pagination_mode_from_proto(pagination.mode)?,
        page_size: pagination
            .page_size
            .as_ref()
            .map(page_size_from_proto)
            .transpose()?,
        cursor_param: pagination.cursor_param.clone(),
        cursor_body_path: pagination.cursor_body_path.clone(),
        response_cursor_path: pagination.response_cursor_path.clone(),
        page_param: pagination.page_param.clone(),
        page_start: pagination.page_start,
        page_step: pagination.page_step.unwrap_or(1),
        offset_param: pagination.offset_param.clone(),
        offset_start: pagination.offset_start,
        offset_step: pagination.offset_step,
        link_header_require_results: pagination.link_header_require_results,
        max_pages: pagination
            .max_pages
            .map(|value| usize_from_u64(value, "source manifest pagination max_pages"))
            .transpose()?,
    })
}

pub(crate) fn columns_from_proto(columns: &[specv1::ColumnSpec]) -> Result<Vec<ColumnSpec>> {
    columns.iter().map(column_from_proto).collect()
}

fn filters_from_proto(filters: &[specv1::FilterSpec]) -> Result<Vec<FilterSpec>> {
    filters
        .iter()
        .map(|filter| {
            Ok(FilterSpec {
                name: filter.name.clone(),
                required: filter.required,
                mode: filter_mode_from_proto(filter.mode)?,
            })
        })
        .collect()
}

fn request_route_from_proto(route: &specv1::RequestRouteSpec) -> Result<RequestRouteSpec> {
    Ok(RequestRouteSpec {
        when_filters: route.when_filters.clone(),
        request: request_from_proto(route.request.as_ref())?,
    })
}

fn request_spec_from_proto(request: &specv1::RequestSpec) -> Result<RequestSpec> {
    Ok(RequestSpec {
        method: http_method_from_proto(request.method)?,
        path: ParsedTemplate::parse(&request.path)?,
        query: request
            .query
            .iter()
            .map(query_param_from_proto)
            .collect::<Result<Vec<_>>>()?,
        body: request
            .body
            .iter()
            .map(body_field_from_proto)
            .collect::<Result<Vec<_>>>()?,
        headers: headers_from_proto(&request.headers)?,
    })
}

fn query_param_from_proto(param: &specv1::QueryParamSpec) -> Result<QueryParamSpec> {
    Ok(QueryParamSpec {
        name: param.name.clone(),
        value: value_source_from_proto(param.value.as_ref(), "source manifest query param")?,
    })
}

fn body_field_from_proto(field: &specv1::BodyFieldSpec) -> Result<BodyFieldSpec> {
    Ok(BodyFieldSpec {
        path: field.path.clone(),
        value: value_source_from_proto(field.value.as_ref(), "source manifest body field")?,
    })
}

fn headers_from_proto(headers: &[specv1::HeaderSpec]) -> Result<Vec<HeaderSpec>> {
    headers.iter().map(header_from_proto).collect()
}

fn header_from_proto(header: &specv1::HeaderSpec) -> Result<HeaderSpec> {
    Ok(HeaderSpec {
        name: header.name.clone(),
        value: value_source_from_proto(header.value.as_ref(), "source manifest header")?,
    })
}

fn value_source_from_proto(
    source: Option<&specv1::ValueSource>,
    context: &str,
) -> Result<ValueSourceSpec> {
    let source = source
        .and_then(|source| source.kind.as_ref())
        .ok_or_else(|| ManifestError::validation(format!("{context} is missing value source")))?;
    match source {
        specv1::value_source::Kind::Template(value) => Ok(ValueSourceSpec::Template {
            template: ParsedTemplate::parse(&value.template)?,
        }),
        specv1::value_source::Kind::Literal(value) => Ok(ValueSourceSpec::Literal {
            value: json_value(&value.json)?,
        }),
        specv1::value_source::Kind::Filter(value) => Ok(ValueSourceSpec::Filter {
            key: value.key.clone(),
            default: value.default_json.as_deref().map(json_value).transpose()?,
        }),
        specv1::value_source::Kind::FilterInt(value) => Ok(ValueSourceSpec::FilterInt {
            key: value.key.clone(),
            default: value.default_value,
        }),
        specv1::value_source::Kind::Input(value) => Ok(ValueSourceSpec::Input {
            key: value.key.clone(),
        }),
        specv1::value_source::Kind::State(value) => Ok(ValueSourceSpec::State {
            key: value.key.clone(),
        }),
        specv1::value_source::Kind::NowEpochMinusSeconds(value) => {
            Ok(ValueSourceSpec::NowEpochMinusSeconds {
                seconds: value.seconds,
            })
        }
    }
}

fn page_size_from_proto(page_size: &specv1::PageSizeSpec) -> Result<PageSizeSpec> {
    Ok(PageSizeSpec {
        default: usize_from_u64(page_size.default_size, "source manifest page_size default")?,
        max: usize_from_u64(page_size.max, "source manifest page_size max")?,
        query_param: page_size.query_param.clone(),
        body_path: page_size.body_path.clone(),
    })
}

fn column_from_proto(column: &specv1::ColumnSpec) -> Result<ColumnSpec> {
    Ok(ColumnSpec {
        name: column.name.clone(),
        data_type: column.data_type.clone(),
        nullable: column.nullable.unwrap_or(true),
        r#virtual: column.r#virtual,
        description: column.description.clone(),
        expr: column.expr.as_ref().map(expr_from_proto).transpose()?,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "Expression normalization mirrors the generated source manifest protobuf."
)]
fn expr_from_proto(expr: &specv1::ExprSpec) -> Result<ExprSpec> {
    let kind = expr
        .kind
        .as_ref()
        .ok_or_else(|| ManifestError::validation("source manifest expression is missing kind"))?;
    match kind {
        specv1::expr_spec::Kind::Path(expr) => Ok(ExprSpec::Path {
            path: expr.path.clone(),
        }),
        specv1::expr_spec::Kind::Coalesce(expr) => Ok(ExprSpec::Coalesce {
            exprs: expr
                .exprs
                .iter()
                .map(expr_from_proto)
                .collect::<Result<Vec<_>>>()?,
        }),
        specv1::expr_spec::Kind::FromFilter(expr) => Ok(ExprSpec::FromFilter {
            key: expr.key.clone(),
        }),
        specv1::expr_spec::Kind::Literal(expr) => Ok(ExprSpec::Literal {
            value: json_value(&expr.json)?,
        }),
        specv1::expr_spec::Kind::NullValue(_) => Ok(ExprSpec::Null),
        specv1::expr_spec::Kind::JoinArray(expr) => Ok(ExprSpec::JoinArray {
            path: expr.path.clone(),
            separator: expr.separator.clone().unwrap_or_else(|| ",".to_string()),
        }),
        specv1::expr_spec::Kind::TagValue(expr) => Ok(ExprSpec::TagValue {
            path: expr.path.clone(),
            key: expr.key.clone(),
            key_field: expr.key_field.clone().unwrap_or_else(|| "key".to_string()),
            value_field: expr
                .value_field
                .clone()
                .unwrap_or_else(|| "value".to_string()),
        }),
        specv1::expr_spec::Kind::IfPresent(expr) => Ok(ExprSpec::IfPresent {
            check: Box::new(expr_from_proto(required_expr(
                expr.check.as_deref(),
                "if_present check",
            )?)?),
            then_value: expr.then_value.clone(),
        }),
        specv1::expr_spec::Kind::JoinTagValues(expr) => Ok(ExprSpec::JoinTagValues {
            path: expr.path.clone(),
            key: expr.key.clone(),
            key_field: expr.key_field.clone().unwrap_or_else(|| "key".to_string()),
            value_field: expr
                .value_field
                .clone()
                .unwrap_or_else(|| "value".to_string()),
            separator: expr.separator.clone().unwrap_or_else(|| ",".to_string()),
        }),
        specv1::expr_spec::Kind::FirstArrayItemPath(expr) => Ok(ExprSpec::FirstArrayItemPath {
            path: expr.path.clone(),
            item_path: expr.item_path.clone(),
        }),
        specv1::expr_spec::Kind::ObjectFilterPath(expr) => Ok(ExprSpec::ObjectFilterPath {
            path: expr.path.clone(),
            filter_key: expr.filter_key.clone(),
            item_path: expr.item_path.clone(),
        }),
        specv1::expr_spec::Kind::CurrentRow(_) => Ok(ExprSpec::CurrentRow),
        specv1::expr_spec::Kind::FormatTimestamp(expr) => Ok(ExprSpec::FormatTimestamp {
            expr: Box::new(expr_from_proto(required_expr(
                expr.expr.as_deref(),
                "format_timestamp expr",
            )?)?),
            input: timestamp_input_from_proto(expr.input)?,
        }),
        specv1::expr_spec::Kind::Replace(expr) => Ok(ExprSpec::Replace {
            expr: Box::new(expr_from_proto(required_expr(
                expr.expr.as_deref(),
                "replace expr",
            )?)?),
            from: expr.from.clone(),
            to: expr.to.clone(),
        }),
        specv1::expr_spec::Kind::Template(expr) => Ok(ExprSpec::Template {
            template: ParsedTemplate::parse(&expr.template)?,
            values: expr_map_from_proto(&expr.values)?,
        }),
    }
}

fn expr_map_from_proto(
    exprs: &HashMap<String, specv1::ExprSpec>,
) -> Result<HashMap<String, ExprSpec>> {
    exprs
        .iter()
        .map(|(key, value)| expr_from_proto(value).map(|expr| (key.clone(), expr)))
        .collect()
}

fn required_expr<'a>(
    expr: Option<&'a specv1::ExprSpec>,
    context: &str,
) -> Result<&'a specv1::ExprSpec> {
    expr.ok_or_else(|| {
        ManifestError::validation(format!("source manifest expression is missing {context}"))
    })
}

fn filter_mode_from_proto(value: i32) -> Result<FilterMode> {
    match specv1::FilterMode::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest filter mode enum value {value} is invalid"
        ))
    })? {
        specv1::FilterMode::Unspecified | specv1::FilterMode::Equality => Ok(FilterMode::Equality),
        specv1::FilterMode::Search => Ok(FilterMode::Search),
        specv1::FilterMode::Contains => Ok(FilterMode::Contains),
    }
}

fn http_method_from_proto(value: i32) -> Result<HttpMethod> {
    match specv1::HttpMethod::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest HTTP method enum value {value} is invalid"
        ))
    })? {
        specv1::HttpMethod::Unspecified | specv1::HttpMethod::Get => Ok(HttpMethod::GET),
        specv1::HttpMethod::Post => Ok(HttpMethod::POST),
    }
}

fn row_strategy_from_proto(value: i32) -> Result<RowStrategy> {
    match specv1::RowStrategy::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest row strategy enum value {value} is invalid"
        ))
    })? {
        specv1::RowStrategy::Unspecified | specv1::RowStrategy::Direct => Ok(RowStrategy::Direct),
        specv1::RowStrategy::SeriesPointList => Ok(RowStrategy::SeriesPointList),
        specv1::RowStrategy::DictEntries => Ok(RowStrategy::DictEntries),
    }
}

fn pagination_mode_from_proto(value: i32) -> Result<PaginationMode> {
    match specv1::PaginationMode::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest pagination mode enum value {value} is invalid"
        ))
    })? {
        specv1::PaginationMode::Unspecified | specv1::PaginationMode::None => {
            Ok(PaginationMode::None)
        }
        specv1::PaginationMode::Auto => Ok(PaginationMode::Auto),
        specv1::PaginationMode::CursorQuery => Ok(PaginationMode::CursorQuery),
        specv1::PaginationMode::CursorBody => Ok(PaginationMode::CursorBody),
        specv1::PaginationMode::Page => Ok(PaginationMode::Page),
        specv1::PaginationMode::Offset => Ok(PaginationMode::Offset),
        specv1::PaginationMode::LinkHeader => Ok(PaginationMode::LinkHeader),
    }
}

fn timestamp_input_from_proto(value: i32) -> Result<TimestampInput> {
    match specv1::TimestampInput::try_from(value).map_err(|_| {
        ManifestError::validation(format!(
            "source manifest timestamp input enum value {value} is invalid"
        ))
    })? {
        specv1::TimestampInput::Unspecified | specv1::TimestampInput::Seconds => {
            Ok(TimestampInput::Seconds)
        }
        specv1::TimestampInput::Milliseconds => Ok(TimestampInput::Milliseconds),
    }
}

fn json_value(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(ManifestError::deserialize)
}

fn usize_from_u64(value: u64, context: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| ManifestError::validation(format!("{context} exceeds supported usize range")))
}
