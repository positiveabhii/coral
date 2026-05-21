#![allow(
    missing_docs,
    reason = "This module defines many field-heavy declarative source-spec types."
)]

//! Backend-owned manifest model and validation for HTTP sources.
//!
//! HTTP manifests describe request templating, response-row extraction, filter
//! binding, and pagination. These types are normalized and validated here, but
//! they are still engine-neutral; no runtime HTTP client or execution concerns
//! live in this crate.

use std::collections::{BTreeSet, HashSet};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    BodySpec, ColumnSpec, FilterSpec, HeaderSpec, HttpMethod, ManifestError, ManifestInputKind,
    ManifestInputSpec, PaginationSpec, ParsedTemplate, RelationCommon, RequestRouteSpec,
    RequestSpec, ResponseSpec, Result, SourceBackend, SourceManifestCommon,
    SourceTableFunctionSpec, TemplateNamespace, ValueSourceSpec,
    inputs::collect_source_inputs_value, validate::validate_template, validate_columns,
    validate_http_function, validate_http_function_names, validate_http_table,
    validate_relation_names, validate_test_queries,
};

/// Source-level authentication requirements for HTTP-backed source specs.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum AuthSpec {
    /// HTTP Basic authentication; runtime base64-encodes `username:password`.
    #[serde(rename = "BasicAuth")]
    BasicAuth(BasicAuthSpec),
    /// Declarative list of auth headers to attach to the request.
    #[serde(rename = "HeaderAuth")]
    HeaderAuth(HeaderAuthSpec),
    /// Dispatches auth header resolution to a runtime-registered authenticator.
    #[serde(rename = "CustomAuth")]
    CustomAuth(CustomAuthSpec),
}

impl Default for AuthSpec {
    fn default() -> Self {
        Self::HeaderAuth(HeaderAuthSpec::default())
    }
}

/// HTTP Basic authenticator with separate username and password templates.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasicAuthSpec {
    pub username: ParsedTemplate,
    pub password: ParsedTemplate,
}

/// Declarative authenticator that injects one or more headers.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeaderAuthSpec {
    #[serde(default)]
    pub headers: Vec<HeaderSpec>,
}

/// Dispatches to a runtime-registered request authenticator by name.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomAuthSpec {
    pub authenticator: String,
    #[serde(flatten)]
    pub config: Map<String, Value>,
}

/// Provider-specific response hints for classifying and delaying rate-limit retries.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RateLimitSpec {
    #[serde(default)]
    pub extra_statuses: Vec<u16>,
    #[serde(default)]
    pub retry_after_header: Option<String>,
    #[serde(default)]
    pub remaining_header: Option<String>,
    #[serde(default)]
    pub reset_header: Option<String>,
}

/// Validated top-level manifest for an HTTP-backed source.
#[derive(Debug, Clone)]
pub struct HttpSourceManifest {
    pub common: SourceManifestCommon,
    pub base_url: ParsedTemplate,
    pub auth: AuthSpec,
    pub request_headers: Vec<HeaderSpec>,
    pub rate_limit: RateLimitSpec,
    pub relations: Vec<HttpRelationSpec>,
    pub functions: Vec<SourceTableFunctionSpec>,
    pub declared_inputs: Vec<ManifestInputSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHttpSourceManifest {
    dsl_version: u32,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    test_queries: Vec<String>,
    backend: SourceBackend,
    #[serde(default)]
    base_url: ParsedTemplate,
    #[serde(default)]
    auth: AuthSpec,
    #[serde(default)]
    request_headers: Vec<HeaderSpec>,
    #[serde(default)]
    rate_limit: RateLimitSpec,
    #[serde(default)]
    inputs: Option<Value>,
    #[serde(default)]
    relations: Vec<RawHttpRelationSpec>,
    #[serde(default)]
    functions: Vec<SourceTableFunctionSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHttpRelationSpec {
    name: String,
    description: String,
    #[serde(default)]
    guide: String,
    #[serde(default)]
    read: Option<RawHttpRelationReadSpec>,
    #[serde(default)]
    filters: Vec<FilterSpec>,
    #[serde(default)]
    fetch_limit_default: Option<usize>,
    #[serde(default)]
    request: RequestSpec,
    #[serde(default)]
    requests: Vec<RequestRouteSpec>,
    #[serde(default)]
    response: ResponseSpec,
    #[serde(default)]
    pagination: PaginationSpec,
    #[serde(default)]
    columns: Vec<ColumnSpec>,
    #[serde(default)]
    insert: Option<RawHttpRelationWriteOperationSpec>,
    #[serde(default)]
    update: Option<RawHttpRelationWriteOperationSpec>,
    #[serde(default)]
    delete: Option<RawHttpRelationWriteOperationSpec>,
    #[serde(default)]
    truncate: Option<RawHttpRelationWriteOperationSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHttpRelationReadSpec {
    #[serde(default)]
    filters: Vec<FilterSpec>,
    #[serde(default)]
    fetch_limit_default: Option<usize>,
    #[serde(default)]
    request: RequestSpec,
    #[serde(default)]
    requests: Vec<RequestRouteSpec>,
    #[serde(default)]
    response: ResponseSpec,
    #[serde(default)]
    pagination: PaginationSpec,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRelationWriteInputSpec {
    #[serde(default)]
    columns: Vec<ColumnSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHttpRelationWriteOperationSpec {
    #[serde(default)]
    input: RawRelationWriteInputSpec,
    #[serde(default)]
    request: RequestSpec,
    #[serde(default)]
    response: ResponseSpec,
}

/// One validated HTTP relation read projection.
#[derive(Debug, Clone)]
pub struct HttpRelationReadSpec {
    pub request: RequestSpec,
    pub requests: Vec<RequestRouteSpec>,
    pub response: ResponseSpec,
    pub pagination: PaginationSpec,
}

/// Operation kind for one relation write declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpRelationWriteOperation {
    Insert,
    Update,
    Delete,
    Truncate,
}

impl HttpRelationWriteOperation {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Truncate => "truncate",
        }
    }
}

/// One validated HTTP relation write declaration.
#[derive(Debug, Clone)]
pub struct HttpRelationWriteOperationSpec {
    pub operation: HttpRelationWriteOperation,
    pub input_columns: Vec<ColumnSpec>,
    pub request: RequestSpec,
    pub response: ResponseSpec,
    pub key_columns: Vec<String>,
}

/// One validated HTTP relation declaration.
#[derive(Debug, Clone)]
pub struct HttpRelationSpec {
    pub common: RelationCommon,
    pub read: Option<HttpRelationReadSpec>,
    pub insert: Option<HttpRelationWriteOperationSpec>,
    pub update: Option<HttpRelationWriteOperationSpec>,
    pub delete: Option<HttpRelationWriteOperationSpec>,
    pub truncate: Option<HttpRelationWriteOperationSpec>,
}

impl HttpRelationSpec {
    #[must_use]
    /// Returns the stable relation name.
    pub fn name(&self) -> &str {
        &self.common.name
    }

    #[must_use]
    /// Returns the declared SQL filters that may influence request selection.
    pub fn filters(&self) -> &[FilterSpec] {
        &self.common.filters
    }

    #[must_use]
    /// Returns the declared output columns for this table.
    pub fn columns(&self) -> &[ColumnSpec] {
        &self.common.columns
    }

    #[must_use]
    /// Returns the default fetch limit declared by the manifest, if any.
    pub fn fetch_limit_default(&self) -> Option<usize> {
        self.common.fetch_limit_default
    }

    #[must_use]
    /// Returns the read projection, if this relation is readable.
    pub fn read(&self) -> Option<&HttpRelationReadSpec> {
        self.read.as_ref()
    }

    #[must_use]
    /// Selects the most specific request route that matches the provided
    /// filter set, or falls back to the default request.
    pub fn resolve_read_request(&self, provided_filters: &HashSet<String>) -> Option<&RequestSpec> {
        let read = self.read.as_ref()?;
        let mut best_match: Option<&RequestRouteSpec> = None;
        let mut best_specificity = 0usize;

        for route in &read.requests {
            if route
                .when_filters
                .iter()
                .all(|f| provided_filters.contains(f))
            {
                let specificity = route.when_filters.len();
                if best_match.is_none() || specificity > best_specificity {
                    best_match = Some(route);
                    best_specificity = specificity;
                }
            }
        }

        Some(best_match.map_or(&read.request, |route| &route.request))
    }
}

/// Compatibility alias for internal call sites that still use `DataFusion` table terminology.
pub type HttpTableSpec = HttpRelationSpec;

impl HttpSourceManifest {
    /// Returns the source secrets required by this manifest.
    ///
    /// In the new input model, every declared input with `kind: secret` is
    /// required because secrets cannot carry defaults.
    pub fn required_secret_names(&self) -> BTreeSet<String> {
        self.declared_inputs
            .iter()
            .filter(|input| input.kind == ManifestInputKind::Secret)
            .map(|input| input.key.clone())
            .collect()
    }
}

fn validate_write_operation(
    schema: &str,
    relation: &str,
    relation_columns: &[ColumnSpec],
    operation: HttpRelationWriteOperation,
    raw: Option<RawHttpRelationWriteOperationSpec>,
) -> Result<Option<HttpRelationWriteOperationSpec>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let context = format!("{schema}.{relation} {}", operation.as_str());
    if raw.request.path.raw().trim().is_empty() {
        return Err(ManifestError::validation(format!(
            "{context} has an empty request.path"
        )));
    }
    if raw.request.method == HttpMethod::GET {
        return Err(ManifestError::validation(format!(
            "{context} must use a non-GET request method"
        )));
    }

    validate_write_request_bindings(&raw.request, &context)?;
    let key_columns = derived_key_columns(&raw.request);
    let relation_column_names = relation_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();

    for key in &key_columns {
        if !relation_column_names.contains(key.as_str()) {
            return Err(ManifestError::validation(format!(
                "{context} request references unknown key column '{key}'"
            )));
        }
    }

    if matches!(
        operation,
        HttpRelationWriteOperation::Update | HttpRelationWriteOperation::Delete
    ) && key_columns.is_empty()
    {
        return Err(ManifestError::validation(format!(
            "{context} must reference at least one {{key.*}} target column"
        )));
    }

    let input_columns = raw.input.columns;
    if matches!(
        operation,
        HttpRelationWriteOperation::Insert | HttpRelationWriteOperation::Update
    ) && input_columns.is_empty()
    {
        return Err(ManifestError::validation(format!(
            "{context} must declare input.columns"
        )));
    }

    if !input_columns.is_empty() {
        validate_columns(
            &input_columns,
            schema,
            &format!("{relation} {}", operation.as_str()),
        )?;
    }
    let key_column_names = key_columns
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    for column in &input_columns {
        if !relation_column_names.contains(column.name.as_str()) {
            return Err(ManifestError::validation(format!(
                "{context} input column '{}' is not a relation column",
                column.name
            )));
        }
        if matches!(operation, HttpRelationWriteOperation::Update)
            && key_column_names.contains(column.name.as_str())
        {
            return Err(ManifestError::validation(format!(
                "{context} input column '{}' is a target key and cannot be assigned",
                column.name
            )));
        }
    }

    Ok(Some(HttpRelationWriteOperationSpec {
        operation,
        input_columns,
        request: raw.request,
        response: raw.response,
        key_columns,
    }))
}

fn validate_write_request_bindings(request: &RequestSpec, context: &str) -> Result<()> {
    validate_write_template(&request.path, context)?;
    for header in &request.headers {
        validate_write_value_source(
            &header.value,
            &format!("{context} request header '{}'", header.name),
        )?;
    }
    for param in &request.query {
        validate_write_value_source(
            &param.value,
            &format!("{context} query param '{}'", param.name),
        )?;
    }
    match &request.body {
        BodySpec::Json { fields } => {
            for field in fields {
                validate_write_value_source(
                    &field.value,
                    &format!("{context} request body path '{}'", field.path.join(".")),
                )?;
            }
        }
        BodySpec::Text { content } => {
            validate_write_value_source(content, &format!("{context} request body text"))?;
        }
    }
    Ok(())
}

fn validate_write_value_source(source: &ValueSourceSpec, context: &str) -> Result<()> {
    match source {
        ValueSourceSpec::Template { template } => validate_write_template(template, context),
        ValueSourceSpec::Filter { key, .. }
        | ValueSourceSpec::FilterInt { key, .. }
        | ValueSourceSpec::FilterBool { key, .. }
        | ValueSourceSpec::FilterSplit { key, .. }
        | ValueSourceSpec::FilterSplitInt { key, .. } => Err(ManifestError::validation(format!(
            "{context} uses read filter '{key}' inside a write request"
        ))),
        ValueSourceSpec::Arg { key, .. }
        | ValueSourceSpec::ArgInt { key, .. }
        | ValueSourceSpec::ArgBool { key, .. } => Err(ManifestError::validation(format!(
            "{context} uses function argument '{key}' inside a relation write request"
        ))),
        ValueSourceSpec::Literal { .. }
        | ValueSourceSpec::Input { .. }
        | ValueSourceSpec::State { .. }
        | ValueSourceSpec::NowEpochMinusSeconds { .. } => Ok(()),
    }
}

fn validate_write_template(template: &ParsedTemplate, context: &str) -> Result<()> {
    for token in template.tokens() {
        match token.namespace() {
            TemplateNamespace::Key | TemplateNamespace::Input | TemplateNamespace::State => {}
            TemplateNamespace::Filter => {
                return Err(ManifestError::validation(format!(
                    "{context} uses read filter token '{}' inside a write request",
                    token.raw()
                )));
            }
            TemplateNamespace::Arg => {
                return Err(ManifestError::validation(format!(
                    "{context} uses function argument token '{}' inside a relation write request",
                    token.raw()
                )));
            }
            TemplateNamespace::Expr | TemplateNamespace::Other(_) => {
                return Err(ManifestError::validation(format!(
                    "{context} uses unsupported write template token '{}'",
                    token.raw()
                )));
            }
        }
    }
    Ok(())
}

fn derived_key_columns(request: &RequestSpec) -> Vec<String> {
    let mut keys = BTreeSet::new();
    collect_template_key_columns(&request.path, &mut keys);
    for header in &request.headers {
        collect_value_source_key_columns(&header.value, &mut keys);
    }
    for param in &request.query {
        collect_value_source_key_columns(&param.value, &mut keys);
    }
    match &request.body {
        BodySpec::Json { fields } => {
            for field in fields {
                collect_value_source_key_columns(&field.value, &mut keys);
            }
        }
        BodySpec::Text { content } => collect_value_source_key_columns(content, &mut keys),
    }
    keys.into_iter().collect()
}

fn collect_value_source_key_columns(source: &ValueSourceSpec, keys: &mut BTreeSet<String>) {
    if let ValueSourceSpec::Template { template } = source {
        collect_template_key_columns(template, keys);
    }
}

fn collect_template_key_columns(template: &ParsedTemplate, keys: &mut BTreeSet<String>) {
    for token in template.tokens() {
        if token.namespace() == &TemplateNamespace::Key {
            keys.insert(token.key().to_string());
        }
    }
}

impl RawHttpRelationSpec {
    fn into_validated(self, schema: &str) -> Result<HttpRelationSpec> {
        let read = self.read.unwrap_or(RawHttpRelationReadSpec {
            filters: self.filters,
            fetch_limit_default: self.fetch_limit_default,
            request: self.request,
            requests: self.requests,
            response: self.response,
            pagination: self.pagination,
        });

        let has_read = !read.request.path.raw().trim().is_empty();
        if has_read {
            validate_http_table(
                schema,
                &self.name,
                &read.filters,
                &self.columns,
                &read.request,
                &read.requests,
                &read.pagination,
            )?;
        } else {
            validate_columns(&self.columns, schema, &self.name)?;
        }

        let insert = validate_write_operation(
            schema,
            &self.name,
            &self.columns,
            HttpRelationWriteOperation::Insert,
            self.insert,
        )?;
        let update = validate_write_operation(
            schema,
            &self.name,
            &self.columns,
            HttpRelationWriteOperation::Update,
            self.update,
        )?;
        let delete = validate_write_operation(
            schema,
            &self.name,
            &self.columns,
            HttpRelationWriteOperation::Delete,
            self.delete,
        )?;
        let truncate = validate_write_operation(
            schema,
            &self.name,
            &self.columns,
            HttpRelationWriteOperation::Truncate,
            self.truncate,
        )?;

        if !has_read
            && insert.is_none()
            && update.is_none()
            && delete.is_none()
            && truncate.is_none()
        {
            return Err(ManifestError::validation(format!(
                "{schema}.{} must define read or at least one write operation",
                self.name
            )));
        }

        Ok(HttpRelationSpec {
            common: RelationCommon::new(
                self.name,
                self.description,
                self.guide,
                read.filters.clone(),
                read.fetch_limit_default,
                self.columns,
            ),
            read: has_read.then_some(HttpRelationReadSpec {
                request: read.request,
                requests: read.requests,
                response: read.response,
                pagination: read.pagination,
            }),
            insert,
            update,
            delete,
            truncate,
        })
    }
}

impl HttpSourceManifest {
    pub(crate) fn parse_manifest_value(value: Value) -> Result<Self> {
        let declared_inputs = collect_source_inputs_value(&value)?;
        let raw: RawHttpSourceManifest =
            serde_json::from_value(value).map_err(ManifestError::deserialize)?;
        let RawHttpSourceManifest {
            dsl_version,
            name,
            version,
            description,
            test_queries,
            backend: _backend,
            base_url,
            auth,
            request_headers,
            rate_limit,
            inputs: _inputs,
            relations,
            functions,
        } = raw;
        if relations.is_empty() && functions.is_empty() {
            return Err(ManifestError::validation(format!(
                "source '{name}' must define at least one relation or function"
            )));
        }
        validate_test_queries(&name, &test_queries)?;
        validate_relation_names(
            &name,
            relations.iter().map(|relation| relation.name.as_str()),
        )?;
        let common =
            SourceManifestCommon::new(dsl_version, name, version, description, test_queries);
        let relations = relations
            .into_iter()
            .map(|relation| relation.into_validated(&common.name))
            .collect::<Result<Vec<_>>>()?;
        validate_http_function_names(
            &common.name,
            relations.iter().map(HttpRelationSpec::name),
            &functions,
        )?;
        let functions = functions
            .into_iter()
            .map(|function| {
                validate_http_function(&common.name, &function)?;
                Ok(function)
            })
            .collect::<Result<Vec<_>>>()?;
        if base_url.raw().trim().is_empty() {
            return Err(ManifestError::validation(format!(
                "source '{}' must define a non-empty base_url",
                common.name
            )));
        }
        validate_template(
            &base_url,
            &HashSet::new(),
            &format!("source '{}'", common.name),
        )?;

        Ok(Self {
            common,
            base_url,
            auth,
            request_headers,
            rate_limit,
            relations,
            functions,
            declared_inputs,
        })
    }
}

#[cfg(test)]
pub(crate) fn test_http_table_spec(
    name: &str,
    columns: Vec<ColumnSpec>,
    filters: Vec<FilterSpec>,
    request: RequestSpec,
) -> HttpRelationSpec {
    HttpRelationSpec {
        common: RelationCommon::new(
            name.to_string(),
            "test".to_string(),
            String::new(),
            filters,
            None,
            columns,
        ),
        read: Some(HttpRelationReadSpec {
            request,
            requests: vec![],
            response: ResponseSpec::default(),
            pagination: PaginationSpec::default(),
        }),
        insert: None,
        update: None,
        delete: None,
        truncate: None,
    }
}
