//! Serde glue for authored YAML that deserializes directly into generated proto.

use serde::de::{self, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use crate::proto::v1 as specv1;

pub(crate) fn source_backend<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "http" => Some(specv1::SourceBackend::Http as i32),
        "parquet" => Some(specv1::SourceBackend::Parquet as i32),
        "jsonl" => Some(specv1::SourceBackend::Jsonl as i32),
        _ => None,
    })
}

pub(crate) fn source_input_kind<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "variable" => Some(specv1::SourceInputKind::Variable as i32),
        "secret" => Some(specv1::SourceInputKind::Secret as i32),
        _ => None,
    })
}

pub(crate) fn filter_mode<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "equality" => Some(specv1::FilterMode::Equality as i32),
        "search" => Some(specv1::FilterMode::Search as i32),
        "contains" => Some(specv1::FilterMode::Contains as i32),
        _ => None,
    })
}

pub(crate) fn http_method<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "GET" => Some(specv1::HttpMethod::Get as i32),
        "POST" => Some(specv1::HttpMethod::Post as i32),
        _ => None,
    })
}

pub(crate) fn response_body_format<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "json" => Some(specv1::ResponseBodyFormat::Json as i32),
        "json_each_row" => Some(specv1::ResponseBodyFormat::JsonEachRow as i32),
        _ => None,
    })
}

pub(crate) fn row_strategy<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "direct" => Some(specv1::RowStrategy::Direct as i32),
        "series_point_list" => Some(specv1::RowStrategy::SeriesPointList as i32),
        "dict_entries" => Some(specv1::RowStrategy::DictEntries as i32),
        _ => None,
    })
}

pub(crate) fn pagination_mode<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "none" => Some(specv1::PaginationMode::None as i32),
        "auto" => Some(specv1::PaginationMode::Auto as i32),
        "cursor_query" => Some(specv1::PaginationMode::CursorQuery as i32),
        "cursor_body" => Some(specv1::PaginationMode::CursorBody as i32),
        "page" => Some(specv1::PaginationMode::Page as i32),
        "offset" => Some(specv1::PaginationMode::Offset as i32),
        "link_header" => Some(specv1::PaginationMode::LinkHeader as i32),
        _ => None,
    })
}

pub(crate) fn timestamp_input<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
{
    enum_value(deserializer, |name| match name {
        "seconds" => Some(specv1::TimestampInput::Seconds as i32),
        "milliseconds" => Some(specv1::TimestampInput::Milliseconds as i32),
        _ => None,
    })
}

fn enum_value<'de, D, F>(deserializer: D, named: F) -> Result<i32, D::Error>
where
    D: Deserializer<'de>,
    F: FnOnce(&str) -> Option<i32>,
{
    match Value::deserialize(deserializer)? {
        Value::String(name) => {
            named(&name).ok_or_else(|| D::Error::custom(format!("unsupported enum value '{name}'")))
        }
        Value::Number(number) => number
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| D::Error::custom("enum value must fit in a signed 32-bit integer")),
        _ => Err(D::Error::custom("enum value must be a string or integer")),
    }
}

pub(crate) fn source_inputs<'de, D>(
    deserializer: D,
) -> Result<Vec<specv1::SourceInputBinding>, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => {
            serde_json::from_value(Value::Array(items)).map_err(D::Error::custom)
        }
        Value::Object(inputs) => inputs
            .into_iter()
            .map(|(key, value)| {
                let input = serde_json::from_value(value).map_err(D::Error::custom)?;
                Ok(specv1::SourceInputBinding {
                    key,
                    input: Some(input),
                })
            })
            .collect(),
        _ => Err(D::Error::custom(
            "source manifest inputs must be a mapping or list",
        )),
    }
}

pub(crate) fn request_body<'de, D>(deserializer: D) -> Result<Option<specv1::BodySpec>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    body_from_value(value).map(Some)
}

pub(crate) fn json_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    serde_json::to_string(&value).map_err(D::Error::custom)
}

pub(crate) fn optional_json_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Value>::deserialize(deserializer)?
        .map(|value| serde_json::to_string(&value).map_err(D::Error::custom))
        .transpose()
}

impl<'de> Deserialize<'de> for specv1::BodySpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        body_from_value(Value::deserialize(deserializer)?)
    }
}

impl<'de> Deserialize<'de> for specv1::ExprSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self {
            kind: Some(specv1::expr_spec::Kind::deserialize(deserializer)?),
        })
    }
}

impl<'de> Deserialize<'de> for specv1::AuthSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut map = object(Value::deserialize(deserializer)?, "source manifest auth")
            .map_err(D::Error::custom)?;
        let auth_type = map
            .remove("type")
            .map(|value| string(value, "source manifest auth type"))
            .transpose()
            .map_err(D::Error::custom)?;
        let kind = match auth_type.as_deref() {
            Some("BasicAuth") => {
                let username = required_string(&mut map, "username", "source manifest auth")
                    .map_err(D::Error::custom)?;
                let password = required_string(&mut map, "password", "source manifest auth")
                    .map_err(D::Error::custom)?;
                reject_unknown(&map, "source manifest auth").map_err(D::Error::custom)?;
                specv1::auth_spec::Kind::Basic(specv1::BasicAuthSpec { username, password })
            }
            Some("CustomAuth") => {
                let spec = serde_json::from_value(Value::Object(map)).map_err(D::Error::custom)?;
                specv1::auth_spec::Kind::Custom(spec)
            }
            Some("HeaderAuth") | None => {
                let headers = match map.remove("headers") {
                    Some(value) => serde_json::from_value(value).map_err(D::Error::custom)?,
                    None => Vec::new(),
                };
                reject_unknown(&map, "source manifest auth").map_err(D::Error::custom)?;
                specv1::auth_spec::Kind::Header(specv1::HeaderAuthSpec { headers })
            }
            Some(other) => {
                return Err(D::Error::custom(format!(
                    "source manifest auth type '{other}' is unsupported"
                )));
            }
        };
        Ok(Self { kind: Some(kind) })
    }
}

impl<'de> Deserialize<'de> for specv1::RequestRouteSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut map = object(
            Value::deserialize(deserializer)?,
            "source manifest request route",
        )
        .map_err(D::Error::custom)?;
        let when_filters = map
            .remove("when_filters")
            .ok_or_else(|| {
                D::Error::custom("source manifest request route is missing when_filters")
            })
            .and_then(|value| serde_json::from_value(value).map_err(D::Error::custom))?;
        let request = serde_json::from_value(Value::Object(map)).map_err(D::Error::custom)?;
        Ok(Self {
            when_filters,
            request: Some(request),
        })
    }
}

impl<'de> Deserialize<'de> for specv1::CustomAuthSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut map = object(Value::deserialize(deserializer)?, "source manifest auth")
            .map_err(D::Error::custom)?;
        let authenticator = map
            .remove("authenticator")
            .ok_or_else(|| D::Error::custom("source manifest auth is missing authenticator"))
            .and_then(|value| {
                string(value, "source manifest auth authenticator").map_err(D::Error::custom)
            })?;
        let config_json = serde_json::to_string(&Value::Object(map)).map_err(D::Error::custom)?;
        Ok(Self {
            authenticator,
            config_json,
        })
    }
}

fn body_from_value<E>(value: Value) -> Result<specv1::BodySpec, E>
where
    E: de::Error,
{
    let shape = match value {
        Value::Array(items) => specv1::body_spec::Shape::Json(specv1::JsonBody {
            fields: serde_json::from_value(Value::Array(items)).map_err(E::custom)?,
        }),
        Value::Object(map) => body_from_object(map)?,
        _ => return Err(E::custom("source manifest body must be a list or mapping")),
    };
    Ok(specv1::BodySpec { shape: Some(shape) })
}

fn body_from_object<E>(mut map: Map<String, Value>) -> Result<specv1::body_spec::Shape, E>
where
    E: de::Error,
{
    let format = map
        .remove("format")
        .map(|value| string(value, "source manifest body format"))
        .transpose()
        .map_err(E::custom)?;
    match format.as_deref() {
        Some("json") | None => {
            let fields = match map.remove("fields") {
                Some(value) => serde_json::from_value(value).map_err(E::custom)?,
                None => Vec::new(),
            };
            reject_unknown(&map, "source manifest body").map_err(E::custom)?;
            Ok(specv1::body_spec::Shape::Json(specv1::JsonBody { fields }))
        }
        Some("text") => {
            let content = map
                .remove("content")
                .ok_or_else(|| E::custom("source manifest text body is missing content"))
                .and_then(|value| serde_json::from_value(value).map_err(E::custom))?;
            reject_unknown(&map, "source manifest body").map_err(E::custom)?;
            Ok(specv1::body_spec::Shape::Text(specv1::TextBody {
                content: Some(content),
            }))
        }
        Some(other) => Err(E::custom(format!(
            "source manifest body format '{other}' is unsupported"
        ))),
    }
}

fn object(value: Value, context: &str) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(format!("{context} must be a mapping")),
    }
}

fn string(value: Value, context: &str) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value),
        _ => Err(format!("{context} must be a string")),
    }
}

fn required_string(
    map: &mut Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<String, String> {
    let Some(value) = map.remove(key) else {
        return Err(format!("{context} is missing {key}"));
    };
    string(value, &format!("{context} {key}"))
}

fn reject_unknown(map: &Map<String, Value>, context: &str) -> Result<(), String> {
    if let Some(key) = map.keys().next() {
        return Err(format!("{context} has unknown field '{key}'"));
    }
    Ok(())
}
