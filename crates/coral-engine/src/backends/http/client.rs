//! HTTP client used by HTTP-backed source tables.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use datafusion::error::{DataFusionError, Result};
use opentelemetry::propagation::Injector;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value, json};
use tracing::Instrument as _;
use tracing::field;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::RequestAuthenticator;
use crate::backends::http::ProviderQueryError;
use crate::backends::http::auth::{resolve_auth_headers, validate_auth_inputs};
use crate::backends::http::cache::{
    HttpCacheEntry, HttpResponseCache, build_cache_key, estimate_json_bytes,
};
use crate::backends::http::rate_limit::{RateLimitDecision, check_rate_limit};
use crate::backends::http::target::HttpFetchTarget;
use crate::backends::shared::json_path::get_path_value;
use crate::backends::shared::response_rows::extract_rows as shared_extract_rows;
use crate::backends::shared::template::{
    RenderContext, render_template, resolve_value_source, validate_input_dependencies,
    validate_value_source_inputs, value_to_string,
};
use coral_spec::backends::http::{HttpCacheMode, HttpSourceManifest, RateLimitSpec};
use coral_spec::{
    AuthSpec, BodySpec, HeaderSpec, HttpMethod, PageSizeSpec, ParsedTemplate, RequestRouteSpec,
    RequestSpec as ManifestRequestSpec, ResponseBodyFormat, ValidatedPagination,
    ValidatedPaginationMode,
};

const DEFAULT_MAX_PAGES: usize = 10_000;
const DEFAULT_HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_HTTP_USER_AGENT: &str = concat!("coral/", env!("CARGO_PKG_VERSION"));
static NEXT_HTTP_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Executes manifest-driven HTTP requests for one registered source.
#[derive(Clone)]
pub(crate) struct HttpSourceClient {
    http: reqwest::Client,
    request_timeout: Duration,
    source_schema: String,
    source_version: String,
    base_url: ParsedTemplate,
    auth: AuthSpec,
    request_headers: Vec<HeaderSpec>,
    request_authenticators: HashMap<String, Arc<dyn RequestAuthenticator>>,
    rate_limit: RateLimitSpec,
    resolved_inputs: Arc<BTreeMap<String, String>>,
    cache: HttpResponseCache,
}

impl std::fmt::Debug for HttpSourceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSourceClient")
            .field("source_schema", &self.source_schema)
            .field("source_version", &self.source_version)
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .field("request_headers", &self.request_headers)
            .field("rate_limit", &self.rate_limit)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default)]
struct PageState {
    cursor: Option<String>,
    page: i64,
    offset: i64,
    next_url: Option<String>,
}

/// Concrete request body shape passed to the HTTP layer.
#[derive(Debug, Clone)]
enum RequestBody {
    Json(Value),
    Text(String),
}

#[derive(Debug, Clone, Copy)]
struct FetchLimits {
    effective_limit: Option<usize>,
    page_size_limit: Option<usize>,
    max_search_calls: Option<usize>,
}

struct OutgoingHttpRequest<'a> {
    auth: &'a AuthSpec,
    request_headers: &'a [HeaderSpec],
    request_authenticators: &'a HashMap<String, Arc<dyn RequestAuthenticator>>,
    table_headers: &'a [HeaderSpec],
    table_name: &'a str,
    method: HttpMethod,
    base_url: &'a str,
    url: &'a str,
    query_pairs: &'a [(String, String)],
    body: Option<&'a RequestBody>,
    response_format: ResponseBodyFormat,
    source_schema: &'a str,
    rate_limit: &'a RateLimitSpec,
    render_context: RenderContext<'a>,
    allow_404_empty: bool,
    link_header_require_results: bool,
}

struct HttpRequestSite<'a> {
    label: String,
    request: &'a ManifestRequestSpec,
}

struct ResponseDecodeContext<'a> {
    source_schema: &'a str,
    table_name: &'a str,
    method_label: &'a str,
    logged_url: &'a str,
    response_span: &'a tracing::Span,
}

impl HttpSourceClient {
    /// Build a backend client from a validated source spec.
    ///
    /// # Errors
    ///
    /// Returns a `DataFusionError` if required credentials are missing or if an
    /// authentication header template cannot be resolved.
    pub(crate) fn from_manifest(
        manifest: &HttpSourceManifest,
        source_secrets: &BTreeMap<String, String>,
        source_variables: &BTreeMap<String, String>,
        request_authenticators: &HashMap<String, Arc<dyn RequestAuthenticator>>,
    ) -> Result<Self> {
        let resolved_inputs =
            coral_spec::resolve_inputs(&manifest.declared_inputs, source_secrets, source_variables);
        validate_source_scoped_http_config(manifest, request_authenticators, &resolved_inputs)?;

        let request_timeout = Duration::from_secs(DEFAULT_HTTP_REQUEST_TIMEOUT_SECS);
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .user_agent(DEFAULT_HTTP_USER_AGENT)
            .build()
            .map_err(|error| {
                DataFusionError::Execution(format!(
                    "failed to build HTTP client for source '{}': {error}",
                    manifest.common.name
                ))
            })?;

        Ok(Self {
            http,
            request_timeout,
            source_schema: manifest.common.name.clone(),
            source_version: manifest.common.version.clone(),
            base_url: manifest.base_url.clone(),
            auth: manifest.auth.clone(),
            request_headers: manifest.request_headers.clone(),
            request_authenticators: request_authenticators.clone(),
            rate_limit: manifest.rate_limit.clone(),
            resolved_inputs: Arc::new(resolved_inputs),
            cache: HttpResponseCache::new(),
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Paginated fetch logic is stateful and easier to audit in one sequential function"
    )]
    /// Fetch rows for a single table from the backend API.
    ///
    /// # Errors
    ///
    /// Returns a `DataFusionError` if request templates cannot be resolved, the
    /// `HTTP` request fails, the response payload cannot be interpreted, or the
    /// fetched rows cannot be extracted for the table strategy.
    pub(crate) async fn fetch(
        &self,
        target: &HttpFetchTarget,
        filter_values: &HashMap<String, String>,
        arg_values: &HashMap<String, String>,
        sql_limit: Option<usize>,
    ) -> Result<Vec<Value>> {
        let mut all_rows = Vec::new();
        let limits = resolve_fetch_limits(target, sql_limit);
        let pagination = target
            .pagination()
            .validated(&self.source_schema, target.name())
            .map_err(|error| {
                provider_error(ProviderQueryError::Pagination {
                    source_schema: self.source_schema.clone(),
                    table: target.name().to_string(),
                    method: None,
                    url: None,
                    detail: error.to_string(),
                })
            })?;
        let page_size = resolve_page_size(pagination.page_size.as_ref(), limits.page_size_limit);

        let active_request = target.resolved_request();

        let mut state = PageState {
            page: target.pagination().page_start,
            offset: match &pagination.mode {
                ValidatedPaginationMode::Offset(offset) => offset.start,
                _ => target.pagination().offset_start,
            },
            ..PageState::default()
        };

        let mut page_count = 0usize;
        let max_pages = target.pagination().max_pages.unwrap_or(DEFAULT_MAX_PAGES);

        loop {
            page_count += 1;
            if page_count > max_pages {
                return Err(provider_error(ProviderQueryError::Pagination {
                    source_schema: self.source_schema.clone(),
                    table: target.name().to_string(),
                    method: None,
                    url: None,
                    detail: format!("exceeded pagination max_pages={max_pages}"),
                }));
            }

            let state_values = pagination_state_values(&state);
            let render_context = RenderContext::new(
                filter_values,
                arg_values,
                &state_values,
                self.resolved_inputs.as_ref(),
            );
            let base_url = render_template(&self.base_url, &render_context)?;
            let base_url = normalize_base_url(&base_url);
            let following_link_header = matches!(
                pagination.mode,
                ValidatedPaginationMode::LinkHeader | ValidatedPaginationMode::Auto
            ) && state.next_url.is_some();

            let url = if matches!(
                pagination.mode,
                ValidatedPaginationMode::LinkHeader | ValidatedPaginationMode::Auto
            ) && let Some(next) = state.next_url.clone()
            {
                next
            } else {
                let rendered_path = render_template(&active_request.path, &render_context)?;
                join_url(&base_url, &rendered_path)?
            };

            let (query_pairs, body) = if following_link_header {
                (Vec::new(), None)
            } else {
                let mut query_pairs = build_query_pairs(active_request, &render_context)?;
                apply_pagination_query_pairs(
                    &mut query_pairs,
                    target,
                    &pagination,
                    &state,
                    page_size,
                )
                .map_err(|error| {
                    pagination_error(&self.source_schema, target.name(), None, Some(&url), &error)
                })?;

                let mut body = build_request_body(active_request, &render_context)?;
                apply_pagination_body_fields(
                    &mut body,
                    &active_request.body,
                    target,
                    &pagination,
                    &state,
                    page_size,
                )
                .map_err(|error| {
                    pagination_error(&self.source_schema, target.name(), None, Some(&url), &error)
                })?;
                (query_pairs, body)
            };

            // Determine whether this table has an active TTL cache policy.
            let cache_key: Option<(String, usize, Duration)> = target
                .cache()
                .filter(|p| p.mode == HttpCacheMode::Ttl)
                .filter(|policy| {
                    policy
                        .max_pages
                        .is_none_or(|max_cache_pages| page_count <= max_cache_pages)
                })
                .map(|policy| {
                    let body_hash = body.as_ref().map(hash_request_body);
                    let vary_headers = cache_vary_header_hashes(
                        &self.request_headers,
                        &active_request.headers,
                        body.as_ref(),
                        &render_context,
                        &policy.vary_headers,
                    )?;
                    let key = build_cache_key(
                        &self.source_schema,
                        &self.source_version,
                        target.name(),
                        http_method_label(active_request.method),
                        &url,
                        &query_pairs,
                        body_hash,
                        &vary_headers,
                        policy.ttl.as_secs(),
                    );
                    let max_entry = policy.max_entry_bytes.unwrap_or(usize::MAX);
                    Ok::<_, DataFusionError>((key, max_entry, policy.ttl))
                })
                .transpose()?;

            // Check the cache before issuing the outbound request.
            let page = if let Some((ref key, max_entry_bytes, ttl)) = cache_key {
                if let Some(entry) = self.cache.get(key).await {
                    tracing::trace!(
                        source = %self.source_schema,
                        table = %target.name(),
                        "http cache hit"
                    );
                    Some((entry.payload, entry.next_url))
                } else {
                    tracing::trace!(
                        source = %self.source_schema,
                        table = %target.name(),
                        "http cache miss"
                    );
                    let result = execute_request(
                        &self.http,
                        self.request_timeout,
                        OutgoingHttpRequest {
                            auth: &self.auth,
                            request_headers: &self.request_headers,
                            request_authenticators: &self.request_authenticators,
                            table_headers: &active_request.headers,
                            table_name: target.name(),
                            method: active_request.method,
                            base_url: &base_url,
                            url: &url,
                            query_pairs: &query_pairs,
                            body: body.as_ref(),
                            response_format: target.response().format,
                            source_schema: &self.source_schema,
                            rate_limit: &self.rate_limit,
                            render_context,
                            allow_404_empty: target.response().allow_404_empty,
                            link_header_require_results: pagination.link_header_require_results,
                        },
                    )
                    .await?;
                    if let Some((ref payload, ref next_url)) = result {
                        let estimated_bytes = estimate_json_bytes(payload);
                        let ok_for_cache = target.response().ok_path.is_empty()
                            || get_path_value(payload, &target.response().ok_path)
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                        if !ok_for_cache {
                            tracing::trace!(
                                source = %self.source_schema,
                                table = %target.name(),
                                "http cache entry skipped: ok_path=false"
                            );
                        } else if estimated_bytes <= max_entry_bytes {
                            self.cache
                                .put(
                                    key.clone(),
                                    HttpCacheEntry {
                                        payload: payload.clone(),
                                        next_url: next_url.clone(),
                                        ttl,
                                        estimated_bytes,
                                    },
                                )
                                .await;
                        } else {
                            tracing::trace!(
                                source = %self.source_schema,
                                table = %target.name(),
                                estimated_bytes,
                                "http cache entry skipped: exceeds max_entry_bytes"
                            );
                        }
                    }
                    result
                }
            } else {
                execute_request(
                    &self.http,
                    self.request_timeout,
                    OutgoingHttpRequest {
                        auth: &self.auth,
                        request_headers: &self.request_headers,
                        request_authenticators: &self.request_authenticators,
                        table_headers: &active_request.headers,
                        table_name: target.name(),
                        method: active_request.method,
                        base_url: &base_url,
                        url: &url,
                        query_pairs: &query_pairs,
                        body: body.as_ref(),
                        response_format: target.response().format,
                        source_schema: &self.source_schema,
                        rate_limit: &self.rate_limit,
                        render_context,
                        allow_404_empty: target.response().allow_404_empty,
                        link_header_require_results: pagination.link_header_require_results,
                    },
                )
                .await?
            };

            let Some((payload, next_url)) = page else {
                break;
            };

            if !target.response().ok_path.is_empty() {
                let ok = get_path_value(&payload, &target.response().ok_path)
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !ok {
                    let err = if target.response().error_path.is_empty() {
                        "unknown source API error".to_string()
                    } else {
                        get_path_value(&payload, &target.response().error_path)
                            .and_then(Value::as_str)
                            .unwrap_or("unknown source API error")
                            .to_string()
                    };
                    return Err(DataFusionError::External(Box::new(
                        ProviderQueryError::ApiRequest {
                            source_schema: self.source_schema.clone(),
                            table: target.name().to_string(),
                            status: None,
                            method: None,
                            url: None,
                            filters: filter_values.clone(),
                            detail: err,
                        },
                    )));
                }
            }

            let mut rows = extract_rows(target, &payload);
            let rows_on_page = rows.len();
            all_rows.append(&mut rows);

            if let Some(limit) = limits.effective_limit
                && all_rows.len() >= limit
            {
                all_rows.truncate(limit);
                break;
            }

            if limits
                .max_search_calls
                .is_some_and(|max_calls| page_count >= max_calls)
            {
                break;
            }

            match &pagination.mode {
                ValidatedPaginationMode::None => break,
                ValidatedPaginationMode::CursorQuery | ValidatedPaginationMode::CursorBody => {
                    let next_cursor =
                        get_path_value(&payload, &target.pagination().response_cursor_path)
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(ToOwned::to_owned);
                    match next_cursor {
                        Some(cursor) => state.cursor = Some(cursor),
                        None => break,
                    }
                }
                ValidatedPaginationMode::Page => {
                    if page_is_exhausted(rows_on_page, page_size) {
                        break;
                    }
                    state.page = state.page.saturating_add(target.pagination().page_step);
                }
                ValidatedPaginationMode::Offset(offset) => {
                    if page_is_exhausted(rows_on_page, page_size) {
                        break;
                    }
                    let step = offset
                        .resolve_step(page_size, &self.source_schema, target.name())
                        .map_err(|error| {
                            provider_error(ProviderQueryError::Pagination {
                                source_schema: self.source_schema.clone(),
                                table: target.name().to_string(),
                                method: None,
                                url: None,
                                detail: error.to_string(),
                            })
                        })?;
                    state.offset = state.offset.saturating_add(step);
                }
                ValidatedPaginationMode::LinkHeader | ValidatedPaginationMode::Auto => {
                    match next_url {
                        Some(next) => state.next_url = Some(next),
                        None => break,
                    }
                }
            }
        }

        Ok(all_rows)
    }
}

fn validate_source_scoped_http_config(
    manifest: &HttpSourceManifest,
    request_authenticators: &HashMap<String, Arc<dyn RequestAuthenticator>>,
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    check_base_url_inputs(manifest, resolved_inputs)?;
    check_request_header_inputs(manifest, resolved_inputs)?;
    check_request_site_inputs(manifest, resolved_inputs)?;
    check_auth_inputs(manifest, request_authenticators, resolved_inputs)?;
    Ok(())
}

/// `base_url` may reference `{{filter.*}}` / `{{state.*}}` that only resolve
/// per-request. Check input-token deps only; runtime renders the rest.
fn check_base_url_inputs(
    manifest: &HttpSourceManifest,
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    validate_input_dependencies(&manifest.base_url, resolved_inputs)
        .map_err(|error| registration_error(&manifest.common.name, "base_url", &error))
}

/// Same tolerance for filter/state tokens as `base_url`.
fn check_request_header_inputs(
    manifest: &HttpSourceManifest,
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    validate_header_inputs(
        &manifest.common.name,
        "request_headers",
        &manifest.request_headers,
        resolved_inputs,
    )?;
    Ok(())
}

fn check_request_site_inputs(
    manifest: &HttpSourceManifest,
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    for site in http_request_sites(manifest) {
        validate_request_template_inputs(
            &manifest.common.name,
            &site.label,
            site.request,
            resolved_inputs,
        )?;
    }
    Ok(())
}

fn http_request_sites(manifest: &HttpSourceManifest) -> Vec<HttpRequestSite<'_>> {
    let table_sites = manifest.tables.iter().flat_map(|table| {
        let default = std::iter::once(HttpRequestSite {
            label: format!("table '{}' request", table.name()),
            request: &table.request,
        });
        let routes = table.requests.iter().map(move |route| HttpRequestSite {
            label: table_request_route_label(table.name(), route),
            request: &route.request,
        });
        default.chain(routes)
    });

    let function_sites = manifest.functions.iter().map(|function| HttpRequestSite {
        label: format!("function '{}' request", function.name),
        request: &function.request,
    });

    table_sites.chain(function_sites).collect()
}

fn table_request_route_label(table_name: &str, route: &RequestRouteSpec) -> String {
    if route.when_filters.is_empty() {
        format!("table '{table_name}' request route")
    } else {
        format!(
            "table '{table_name}' request route for filters [{}]",
            route.when_filters.join(", ")
        )
    }
}

/// Auth is source-scoped: all template dependencies must resolve from inputs
/// before any request is issued.
fn check_auth_inputs(
    manifest: &HttpSourceManifest,
    request_authenticators: &HashMap<String, Arc<dyn RequestAuthenticator>>,
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    validate_auth_inputs(&manifest.auth, request_authenticators, resolved_inputs)
        .map_err(|error| registration_error(&manifest.common.name, "auth", &error))
}

fn registration_error(source: &str, field: &str, error: &DataFusionError) -> DataFusionError {
    DataFusionError::Execution(format!(
        "source '{source}' {field} could not be resolved: {error}"
    ))
}

fn validate_request_template_inputs(
    source_name: &str,
    request_label: &str,
    request: &ManifestRequestSpec,
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    validate_input_dependencies(&request.path, resolved_inputs).map_err(|error| {
        registration_error(source_name, &format!("{request_label} path"), &error)
    })?;
    validate_header_inputs(
        source_name,
        &format!("{request_label} header"),
        &request.headers,
        resolved_inputs,
    )?;
    for param in &request.query {
        validate_value_source_inputs(&param.value, resolved_inputs).map_err(|error| {
            registration_error(
                source_name,
                &format!("{request_label} query param '{}'", param.name),
                &error,
            )
        })?;
    }
    match &request.body {
        BodySpec::Json { fields } => {
            for field in fields {
                let field_path = if field.path.is_empty() {
                    "<root>".to_string()
                } else {
                    field.path.join(".")
                };
                validate_value_source_inputs(&field.value, resolved_inputs).map_err(|error| {
                    registration_error(
                        source_name,
                        &format!("{request_label} body field '{field_path}'"),
                        &error,
                    )
                })?;
            }
        }
        BodySpec::Text { content } => {
            validate_value_source_inputs(content, resolved_inputs).map_err(|error| {
                registration_error(source_name, &format!("{request_label} body text"), &error)
            })?;
        }
    }
    Ok(())
}

fn validate_header_inputs(
    source_name: &str,
    context: &str,
    headers: &[HeaderSpec],
    resolved_inputs: &BTreeMap<String, String>,
) -> Result<()> {
    for header in headers {
        validate_value_source_inputs(&header.value, resolved_inputs).map_err(|error| {
            registration_error(source_name, &format!("{context} '{}'", header.name), &error)
        })?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "HTTP request execution keeps retry, auth, logging, and response handling in one audited flow"
)]
async fn execute_request(
    http: &reqwest::Client,
    request_timeout: Duration,
    request: OutgoingHttpRequest<'_>,
) -> Result<Option<(Value, Option<String>)>> {
    enum ResponseOutcome {
        Done(Result<Option<(Value, Option<String>)>>),
        Retry(Duration),
    }

    let OutgoingHttpRequest {
        auth,
        request_headers,
        request_authenticators,
        table_headers,
        table_name,
        method,
        base_url,
        url,
        query_pairs,
        body,
        response_format,
        source_schema,
        rate_limit,
        render_context,
        allow_404_empty,
        link_header_require_results,
    } = request;
    let mut server_error_retries = 0usize;
    let mut throttle_retries = 0usize;
    loop {
        let method_label = http_method_label(method);
        let mut request = build_http_request(http, method, url);

        let mut header_map =
            build_declared_header_map(request_headers, table_headers, &render_context)?;
        apply_implicit_content_type(&mut header_map, body);
        let logged_url = build_logged_url(url, query_pairs);

        let request_id = NEXT_HTTP_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let attempt = server_error_retries + throttle_retries + 1;
        let traced_url = sanitize_trace_url(&logged_url);
        let trace_endpoint = trace_http_endpoint(&traced_url);
        let request_span = tracing::info_span!(
            target: "coral_engine::http",
            "http.request",
            coral.http.attempt = attempt,
            coral.http.error.connect = field::Empty,
            coral.http.error.request = field::Empty,
            coral.http.error.timeout = field::Empty,
            coral.http.request_id = request_id,
            coral.source = source_schema,
            coral.table = table_name,
            error = field::Empty,
            error.type = field::Empty,
            exception.message = field::Empty,
            http.host = field::Empty,
            http.request.body.present = body.is_some(),
            http.request.body.size = request_body_size(body).unwrap_or_default(),
            http.request.method = method_label,
            http.request.query_count = query_pairs.len(),
            http.request.resend_count = field::Empty,
            http.response.body.size = field::Empty,
            http.response.status_code = field::Empty,
            net.peer.name = field::Empty,
            otel.kind = "client",
            otel.name = method_label,
            otel.status_code = field::Empty,
            otel.status_description = field::Empty,
            peer.service = field::Empty,
            server.address = field::Empty,
            server.port = field::Empty,
            url.full = %traced_url,
        );
        record_trace_http_endpoint(&request_span, &trace_endpoint);
        if attempt > 1 {
            request_span.record(
                "http.request.resend_count",
                i64::try_from(attempt - 1).unwrap_or(i64::MAX),
            );
        }

        inject_trace_context(&request_span, &mut header_map);
        if !header_map.is_empty() {
            request = request.headers(header_map);
        }

        if !query_pairs.is_empty() {
            request = request.query(query_pairs);
        }

        match body {
            Some(RequestBody::Json(value)) => {
                request = request.json(value);
            }
            Some(RequestBody::Text(text)) => {
                request = request.body(text.clone());
            }
            None => {}
        }

        let built = match resolve_auth_headers(
            auth,
            request,
            request_authenticators,
            render_context.resolved_inputs,
        ) {
            Ok(request) => request,
            Err(error) => {
                record_http_processing_error(&request_span, "REQUEST_SETUP", &error);
                return Err(error);
            }
        };
        let response = match http.execute(built).instrument(request_span.clone()).await {
            Ok(response) => response,
            Err(error) => {
                record_http_processing_error(
                    &request_span,
                    trace_reqwest_error_type(&error),
                    trace_reqwest_error(&error),
                );
                request_span.record("coral.http.error.timeout", error.is_timeout());
                request_span.record("coral.http.error.connect", error.is_connect());
                request_span.record("coral.http.error.request", error.is_request());
                return Err(request_error(
                    source_schema,
                    table_name,
                    method_label,
                    &logged_url,
                    request_timeout,
                    &error,
                ));
            }
        };

        let status = response.status();
        request_span.record("http.response.status_code", status.as_u16());
        let outcome = 'response: {
            if let Some(length) = response.content_length() {
                request_span.record("http.response.body.size", length);
            }

            match check_rate_limit(status, response.headers(), rate_limit, throttle_retries) {
                RateLimitDecision::Continue => {}
                RateLimitDecision::Retry(wait) => {
                    record_http_status_error(&request_span, status, "rate limited; retrying");
                    throttle_retries += 1;
                    break 'response ResponseOutcome::Retry(wait);
                }
                RateLimitDecision::Fail(error) => {
                    let error_message = error.to_string();
                    record_http_status_error(&request_span, status, error_message.as_str());
                    break 'response ResponseOutcome::Done(Err(DataFusionError::External(
                        Box::new(ProviderQueryError::RateLimited {
                            source_schema: source_schema.to_string(),
                            table: table_name.to_string(),
                            method: Some(method_label.to_string()),
                            url: Some(logged_url.clone()),
                            detail: error_message,
                        }),
                    )));
                }
            }

            if status.is_server_error() && server_error_retries < 2 {
                record_http_status_error(&request_span, status, "server error; retrying");
                server_error_retries += 1;
                break 'response ResponseOutcome::Retry(Duration::from_secs(2));
            }

            if status == reqwest::StatusCode::NOT_FOUND && allow_404_empty {
                break 'response ResponseOutcome::Done(Ok(None));
            }

            if !status.is_success() {
                let body = response
                    .text()
                    .instrument(request_span.clone())
                    .await
                    .unwrap_or_default();
                record_http_status_error(
                    &request_span,
                    status,
                    response_error_summary(status, &body),
                );
                request_span.record("http.response.body.size", body.len());
                break 'response ResponseOutcome::Done(Err(DataFusionError::External(Box::new(
                    ProviderQueryError::ApiRequest {
                        source_schema: source_schema.to_string(),
                        table: table_name.to_string(),
                        status: Some(status.as_u16()),
                        method: Some(method_label.to_string()),
                        url: Some(logged_url.clone()),
                        filters: render_context.filters.clone(),
                        detail: body,
                    },
                ))));
            }

            let next_url =
                extract_next_link_url(response.headers(), base_url, link_header_require_results)
                    .map_err(|error| {
                        record_http_processing_error(&request_span, "PAGINATION", &error);
                        pagination_error(
                            source_schema,
                            table_name,
                            Some(method_label),
                            Some(&logged_url),
                            &error,
                        )
                    });
            let next_url = match next_url {
                Ok(next_url) => next_url,
                Err(error) => break 'response ResponseOutcome::Done(Err(error)),
            };

            let payload = decode_response_body(
                response,
                response_format,
                ResponseDecodeContext {
                    source_schema,
                    table_name,
                    method_label,
                    logged_url: &logged_url,
                    response_span: &request_span,
                },
            )
            .instrument(request_span.clone())
            .await
            .inspect_err(|error| {
                record_http_processing_error(&request_span, "DECODE", error);
            })
            .map(|payload| Some((payload, next_url)));
            ResponseOutcome::Done(payload)
        };

        drop(request_span);
        match outcome {
            ResponseOutcome::Done(result) => return result,
            ResponseOutcome::Retry(wait) => {
                tokio::time::sleep(wait).await;
            }
        }
    }
}

async fn decode_response_body(
    response: reqwest::Response,
    format: ResponseBodyFormat,
    context: ResponseDecodeContext<'_>,
) -> Result<Value> {
    let ResponseDecodeContext {
        source_schema,
        table_name,
        method_label,
        logged_url,
        response_span,
    } = context;
    match format {
        ResponseBodyFormat::Json => {
            let bytes = response.bytes().await.map_err(|error| {
                decode_error(source_schema, table_name, method_label, logged_url, &error)
            })?;
            response_span.record("http.response.body.size", bytes.len());
            serde_json::from_slice(&bytes).map_err(|error| {
                json_decode_error(source_schema, table_name, method_label, logged_url, &error)
            })
        }
        ResponseBodyFormat::JsonEachRow => {
            let text = response.text().await.map_err(|error| {
                decode_error(source_schema, table_name, method_label, logged_url, &error)
            })?;
            response_span.record("http.response.body.size", text.len());
            let mut rows = Vec::new();
            for (index, line) in text.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let row: Value = serde_json::from_str(trimmed).map_err(|error| {
                    provider_error(ProviderQueryError::Decode {
                        source_schema: source_schema.to_string(),
                        table: table_name.to_string(),
                        method: Some(method_label.to_string()),
                        url: Some(logged_url.to_string()),
                        detail: format!(
                            "source API response decoding failed: json_each_row line {} is not valid JSON: {error}",
                            index + 1
                        ),
                    })
                })?;
                rows.push(row);
            }
            Ok(Value::Array(rows))
        }
    }
}

fn request_error(
    source_schema: &str,
    table_name: &str,
    method_label: &str,
    logged_url: &str,
    request_timeout: Duration,
    error: &reqwest::Error,
) -> DataFusionError {
    let detail = if error.is_timeout() {
        format!(
            "source API request timed out after {}s",
            request_timeout.as_secs_f64()
        )
    } else {
        "source API request failed before a response was received".to_string()
    };

    provider_error(ProviderQueryError::Request {
        source_schema: source_schema.to_string(),
        table: table_name.to_string(),
        method: Some(method_label.to_string()),
        url: Some(logged_url.to_string()),
        detail,
        timed_out: error.is_timeout(),
    })
}

fn trace_reqwest_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "source API request timed out"
    } else if error.is_connect() {
        "source API connection failed"
    } else if error.is_request() {
        "source API request failed before a response was received"
    } else {
        "source API request failed"
    }
}

fn trace_reqwest_error_type(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "TIMEOUT"
    } else if error.is_connect() {
        "CONNECT"
    } else if error.is_request() {
        "REQUEST"
    } else {
        "OTHER"
    }
}

fn response_error_summary(status: reqwest::StatusCode, body: &str) -> String {
    format!(
        "upstream returned HTTP {}; body_bytes={}",
        status.as_u16(),
        body.len()
    )
}

fn record_http_status_error(
    span: &tracing::Span,
    status: reqwest::StatusCode,
    message: impl std::fmt::Display,
) {
    span.record("error", true);
    span.record("otel.status_code", "error");
    span.record("error.type", field::display(status.as_u16()));
    span.record("otel.status_description", field::display(&message));
    span.record("exception.message", field::display(&message));
}

fn record_http_processing_error(
    span: &tracing::Span,
    error_type: &'static str,
    message: impl std::fmt::Display,
) {
    span.record("error", true);
    span.record("otel.status_code", "error");
    span.record("error.type", error_type);
    span.record("otel.status_description", field::display(&message));
    span.record("exception.message", field::display(&message));
}

fn decode_error(
    source_schema: &str,
    table_name: &str,
    method_label: &str,
    logged_url: &str,
    error: &reqwest::Error,
) -> DataFusionError {
    provider_error(ProviderQueryError::Decode {
        source_schema: source_schema.to_string(),
        table: table_name.to_string(),
        method: Some(method_label.to_string()),
        url: Some(logged_url.to_string()),
        detail: format!("source API response decoding failed: {error}"),
    })
}

fn json_decode_error(
    source_schema: &str,
    table_name: &str,
    method_label: &str,
    logged_url: &str,
    error: &serde_json::Error,
) -> DataFusionError {
    provider_error(ProviderQueryError::Decode {
        source_schema: source_schema.to_string(),
        table: table_name.to_string(),
        method: Some(method_label.to_string()),
        url: Some(logged_url.to_string()),
        detail: format!("source API response decoding failed: {error}"),
    })
}

fn pagination_error(
    source_schema: &str,
    table_name: &str,
    method_label: Option<&str>,
    logged_url: Option<&str>,
    error: &DataFusionError,
) -> DataFusionError {
    provider_error(ProviderQueryError::Pagination {
        source_schema: source_schema.to_string(),
        table: table_name.to_string(),
        method: method_label.map(ToOwned::to_owned),
        url: logged_url.map(ToOwned::to_owned),
        detail: datafusion_detail(error),
    })
}

fn provider_error(error: ProviderQueryError) -> DataFusionError {
    DataFusionError::External(Box::new(error))
}

fn datafusion_detail(error: &DataFusionError) -> String {
    match error {
        DataFusionError::Execution(detail) => detail.clone(),
        other => other.to_string(),
    }
}

fn http_method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::GET => "GET",
        HttpMethod::POST => "POST",
    }
}

fn build_http_request(
    http: &reqwest::Client,
    method: HttpMethod,
    url: &str,
) -> reqwest::RequestBuilder {
    match method {
        HttpMethod::GET => http.get(url),
        HttpMethod::POST => http.post(url),
    }
}

fn build_declared_header_map(
    request_headers: &[HeaderSpec],
    table_headers: &[HeaderSpec],
    render_context: &RenderContext<'_>,
) -> Result<HeaderMap> {
    let mut header_map = HeaderMap::new();
    for header in request_headers.iter().chain(table_headers.iter()) {
        if let Some(value) = resolve_value_source(&header.value, render_context)? {
            let name = HeaderName::try_from(header.name.as_str()).map_err(|error| {
                DataFusionError::Execution(format!(
                    "invalid request header name '{}': {error}",
                    header.name
                ))
            })?;
            let value =
                HeaderValue::try_from(value_to_string(&value).as_str()).map_err(|error| {
                    DataFusionError::Execution(format!(
                        "invalid request header value for '{}': {error}",
                        header.name
                    ))
                })?;
            header_map.insert(name, value);
        }
    }
    Ok(header_map)
}

fn apply_implicit_content_type(header_map: &mut HeaderMap, body: Option<&RequestBody>) {
    if matches!(body, Some(RequestBody::Text(_)))
        && !header_map.contains_key(reqwest::header::CONTENT_TYPE)
    {
        header_map.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain"),
        );
    }
}

fn cache_vary_header_hashes(
    request_headers: &[HeaderSpec],
    table_headers: &[HeaderSpec],
    body: Option<&RequestBody>,
    render_context: &RenderContext<'_>,
    vary_headers: &[String],
) -> Result<Vec<(String, Option<u64>)>> {
    if vary_headers.is_empty() {
        return Ok(Vec::new());
    }

    let mut header_map = build_declared_header_map(request_headers, table_headers, render_context)?;
    apply_implicit_content_type(&mut header_map, body);

    vary_headers
        .iter()
        .map(|header| {
            let name = HeaderName::try_from(header.as_str()).map_err(|error| {
                DataFusionError::Execution(format!(
                    "invalid cache vary header name '{header}': {error}"
                ))
            })?;
            let value_hash = header_map
                .get(&name)
                .map(|value| hash_cache_bytes(value.as_bytes()));
            Ok((name.as_str().to_string(), value_hash))
        })
        .collect()
}

fn build_query_pairs(
    request: &coral_spec::RequestSpec,
    render_context: &RenderContext<'_>,
) -> Result<Vec<(String, String)>> {
    let mut params = Vec::new();

    for param in &request.query {
        let value = resolve_value_source(&param.value, render_context)?;
        if let Some(value) = value {
            params.push((param.name.clone(), value_to_string(&value)));
        }
    }

    Ok(params)
}

fn apply_pagination_query_pairs(
    params: &mut Vec<(String, String)>,
    target: &HttpFetchTarget,
    pagination: &ValidatedPagination,
    state: &PageState,
    page_size: Option<usize>,
) -> Result<()> {
    if let (Some(page_size), Some(spec)) = (page_size, pagination.page_size.as_ref())
        && let Some(name) = &spec.query_param
    {
        params.push((name.clone(), page_size.to_string()));
    }

    match &pagination.mode {
        ValidatedPaginationMode::None
        | ValidatedPaginationMode::Auto
        | ValidatedPaginationMode::CursorBody
        | ValidatedPaginationMode::LinkHeader => {}
        ValidatedPaginationMode::CursorQuery => {
            if let Some(cursor) = &state.cursor {
                let name = target.pagination().cursor_param.clone().ok_or_else(|| {
                    DataFusionError::Execution(
                        "cursor_query pagination requires cursor_param".to_string(),
                    )
                })?;
                params.push((name, cursor.clone()));
            }
        }
        ValidatedPaginationMode::Page => {
            let name = target.pagination().page_param.clone().ok_or_else(|| {
                DataFusionError::Execution("page pagination requires page_param".to_string())
            })?;
            params.push((name, state.page.to_string()));
        }
        ValidatedPaginationMode::Offset(offset) => {
            params.push((offset.param.clone(), state.offset.to_string()));
        }
    }

    Ok(())
}

fn build_request_body(
    request: &coral_spec::RequestSpec,
    render_context: &RenderContext<'_>,
) -> Result<Option<RequestBody>> {
    match &request.body {
        BodySpec::Json { fields } => {
            if fields.is_empty() {
                return Ok(None);
            }
            let mut root = Value::Object(Map::new());
            for field in fields {
                if field
                    .when_arg
                    .as_ref()
                    .is_some_and(|arg| !render_context.args.contains_key(arg))
                {
                    continue;
                }
                if let Some(value) = resolve_value_source(&field.value, render_context)? {
                    set_path_value(&mut root, &field.path, value)?;
                }
            }
            Ok(Some(RequestBody::Json(root)))
        }
        BodySpec::Text { content } => {
            let Some(value) = resolve_value_source(content, render_context)? else {
                return Ok(None);
            };
            Ok(Some(RequestBody::Text(value_to_string(&value))))
        }
    }
}

fn apply_pagination_body_fields(
    body: &mut Option<RequestBody>,
    body_spec: &BodySpec,
    target: &HttpFetchTarget,
    pagination: &ValidatedPagination,
    state: &PageState,
    page_size: Option<usize>,
) -> Result<()> {
    let needs_page_size_body = page_size
        .zip(pagination.page_size.as_ref())
        .is_some_and(|(_, spec)| !spec.body_path.is_empty());
    let needs_cursor_body = matches!(pagination.mode, ValidatedPaginationMode::CursorBody)
        && !target.pagination().cursor_body_path.is_empty()
        && state.cursor.is_some();

    if !needs_page_size_body && !needs_cursor_body {
        return Ok(());
    }

    if matches!(body_spec, BodySpec::Text { .. }) || matches!(body, Some(RequestBody::Text(_))) {
        return Err(DataFusionError::Execution(
            "pagination body fields are not supported with text request bodies".to_string(),
        ));
    }

    if body.is_none() {
        *body = Some(RequestBody::Json(Value::Object(Map::new())));
    }
    let root = match body.as_mut().expect("body is present") {
        RequestBody::Json(root) => root,
        RequestBody::Text(_) => unreachable!("text body rejected above"),
    };

    if let (Some(page_size), Some(spec)) = (page_size, pagination.page_size.as_ref())
        && !spec.body_path.is_empty()
    {
        set_path_value(root, &spec.body_path, json!(page_size))?;
    }

    if matches!(pagination.mode, ValidatedPaginationMode::CursorBody)
        && let Some(cursor) = &state.cursor
    {
        if target.pagination().cursor_body_path.is_empty() {
            return Err(DataFusionError::Execution(
                "cursor_body pagination requires cursor_body_path".to_string(),
            ));
        }
        set_path_value(root, &target.pagination().cursor_body_path, json!(cursor))?;
    }

    Ok(())
}

fn resolve_fetch_limits(target: &HttpFetchTarget, sql_limit: Option<usize>) -> FetchLimits {
    let Some(search_limits) = target.search_limits() else {
        return FetchLimits {
            effective_limit: sql_limit.or(target.fetch_limit_default()),
            page_size_limit: sql_limit,
            max_search_calls: None,
        };
    };

    let requested_top_k = sql_limit.unwrap_or(search_limits.default_top_k);
    let max_candidates = search_limits
        .max_top_k
        .saturating_mul(search_limits.max_calls_per_query);

    FetchLimits {
        effective_limit: Some(requested_top_k.min(max_candidates)),
        page_size_limit: Some(requested_top_k.min(search_limits.max_top_k)),
        max_search_calls: Some(search_limits.max_calls_per_query),
    }
}

fn resolve_page_size(spec: Option<&PageSizeSpec>, requested_limit: Option<usize>) -> Option<usize> {
    let spec = spec?;
    let base = requested_limit.unwrap_or(spec.default);
    Some(base.min(spec.max).max(1))
}

fn page_is_exhausted(rows_on_page: usize, page_size: Option<usize>) -> bool {
    rows_on_page == 0 || page_size.is_some_and(|requested| rows_on_page < requested)
}

fn pagination_state_values(state: &PageState) -> HashMap<String, String> {
    let mut values = HashMap::new();
    values.insert("page".to_string(), state.page.to_string());
    values.insert("offset".to_string(), state.offset.to_string());
    if let Some(cursor) = &state.cursor {
        values.insert("cursor".to_string(), cursor.clone());
    }
    values
}

fn build_logged_url(url: &str, query_pairs: &[(String, String)]) -> String {
    if query_pairs.is_empty() {
        return url.to_string();
    }
    let suffix = query_pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    if url.contains('?') {
        format!("{url}&{suffix}")
    } else {
        format!("{url}?{suffix}")
    }
}

fn sanitize_trace_url(raw: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(raw) else {
        let without_fragment = raw.split_once('#').map_or(raw, |(before, _)| before);
        return without_fragment
            .split_once('?')
            .map_or(without_fragment, |(before, _)| before)
            .to_string();
    };
    url.set_query(None);
    url.set_fragment(None);
    #[expect(
        clippy::let_underscore_must_use,
        reason = "set_username/set_password only fail for cannot-be-a-base URLs; HTTP URLs always have a host"
    )]
    let _ = url.set_username("");
    #[expect(
        clippy::let_underscore_must_use,
        reason = "set_username/set_password only fail for cannot-be-a-base URLs; HTTP URLs always have a host"
    )]
    let _ = url.set_password(None);
    url.to_string()
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TraceHttpEndpoint {
    server_address: Option<String>,
    server_port: Option<u16>,
}

fn trace_http_endpoint(raw: &str) -> TraceHttpEndpoint {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return TraceHttpEndpoint::default();
    };
    TraceHttpEndpoint {
        server_address: url.host_str().map(str::to_string),
        server_port: url.port_or_known_default(),
    }
}

fn record_trace_http_endpoint(span: &tracing::Span, endpoint: &TraceHttpEndpoint) {
    if let Some(address) = &endpoint.server_address {
        span.record("server.address", address.as_str());
        span.record("peer.service", address.as_str());
        span.record("http.host", address.as_str());
        span.record("net.peer.name", address.as_str());
    }
    if let Some(port) = endpoint.server_port {
        span.record("server.port", i64::from(port));
    }
}

struct HeaderMapInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderMapInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(name) = HeaderName::try_from(key)
            && let Ok(value) = HeaderValue::try_from(value)
        {
            self.0.insert(name, value);
        }
    }
}

fn inject_trace_context(span: &tracing::Span, headers: &mut HeaderMap) {
    let cx = span.context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderMapInjector(headers));
    });
}

fn request_body_size(body: Option<&RequestBody>) -> Option<usize> {
    match body {
        Some(RequestBody::Json(value)) => serde_json::to_vec(value).ok().map(|body| body.len()),
        Some(RequestBody::Text(text)) => Some(text.len()),
        None => None,
    }
}

fn join_url(base: &str, path: &str) -> Result<String> {
    let trimmed = path.trim();
    if reqwest::Url::parse(trimmed).is_ok() || trimmed.starts_with("//") {
        return Err(DataFusionError::Execution(
            "request path must be relative; absolute URLs are not allowed".to_string(),
        ));
    }
    let base = base.trim_end_matches('/');
    if trimmed.starts_with('/') {
        Ok(format!("{base}{trimmed}"))
    } else {
        Ok(format!("{base}/{trimmed}"))
    }
}

fn normalize_base_url(base: &str) -> String {
    let trimmed = base.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return trimmed.to_string();
    }
    if trimmed.starts_with("//") {
        return format!("https:{trimmed}");
    }
    format!("https://{trimmed}")
}

fn set_path_value(root: &mut Value, path: &[String], value: Value) -> Result<()> {
    if path.is_empty() {
        *root = value;
        return Ok(());
    }

    set_path_value_at(root, path, value)
}

fn set_path_value_at(cursor: &mut Value, path: &[String], value: Value) -> Result<()> {
    let Some((head, tail)) = path.split_first() else {
        *cursor = value;
        return Ok(());
    };

    if let Ok(index) = head.parse::<usize>() {
        if !cursor.is_array() {
            *cursor = Value::Array(Vec::new());
        }
        let array = cursor.as_array_mut().ok_or_else(|| {
            DataFusionError::Execution("failed to create JSON array path".to_string())
        })?;
        if array.len() <= index {
            const MAX_JSON_ARRAY_INDEX: usize = 10_000;
            if index > MAX_JSON_ARRAY_INDEX {
                return Err(DataFusionError::Execution(format!(
                    "JSON array index {index} exceeds supported maximum {MAX_JSON_ARRAY_INDEX}"
                )));
            }
            array.resize_with(index + 1, || Value::Null);
        }
        let next = array.get_mut(index).ok_or_else(|| {
            DataFusionError::Execution("failed to access JSON array path".to_string())
        })?;
        return set_path_value_at(next, tail, value);
    }

    if !cursor.is_object() {
        *cursor = Value::Object(Map::new());
    }
    let obj = cursor.as_object_mut().ok_or_else(|| {
        DataFusionError::Execution("failed to create JSON object path".to_string())
    })?;
    let next = obj.entry(head.clone()).or_insert(Value::Null);
    set_path_value_at(next, tail, value)
}

fn extract_rows(target: &HttpFetchTarget, payload: &Value) -> Vec<Value> {
    shared_extract_rows(target.response(), payload)
}

fn hash_request_body(body: &RequestBody) -> u64 {
    match body {
        RequestBody::Json(value) => {
            hash_cache_bytes(serde_json::to_string(value).unwrap_or_default().as_bytes())
        }
        RequestBody::Text(text) => hash_cache_bytes(text.as_bytes()),
    }
}

fn hash_cache_bytes(value: &[u8]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn extract_next_link_url(
    headers: &HeaderMap,
    base_url: &str,
    require_results_true: bool,
) -> Result<Option<String>> {
    let Some(header) = headers.get("link") else {
        return Ok(None);
    };
    let Ok(header) = header.to_str() else {
        return Ok(None);
    };
    let base = reqwest::Url::parse(base_url).map_err(|e| {
        DataFusionError::Execution(format!(
            "invalid base URL for pagination links '{base_url}': {e}"
        ))
    })?;
    for part in header.split(',') {
        let item = part.trim();
        if !item.contains("rel=\"next\"") {
            continue;
        }
        if require_results_true && !item.contains("results=\"true\"") {
            continue;
        }
        let start = item.find('<').ok_or_else(|| {
            DataFusionError::Execution(format!("invalid pagination Link header item '{item}'"))
        })?;
        let end = item.find('>').ok_or_else(|| {
            DataFusionError::Execution(format!("invalid pagination Link header item '{item}'"))
        })?;
        let next_raw = item.get(start + 1..end).ok_or_else(|| {
            DataFusionError::Execution(format!("invalid pagination Link header item '{item}'"))
        })?;
        let next_url = base.join(next_raw).map_err(|e| {
            DataFusionError::Execution(format!("invalid pagination next link '{next_raw}': {e}"))
        })?;
        if next_url.origin() != base.origin() {
            return Err(DataFusionError::Execution(format!(
                "pagination next link must stay on origin {}: {next_raw}",
                base.origin().ascii_serialization()
            )));
        }
        return Ok(Some(next_url.to_string()));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::time::Duration;

    use datafusion::error::DataFusionError;
    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::{
        HttpSourceClient, OutgoingHttpRequest as TestOutgoingHttpRequest, PageState,
        apply_pagination_body_fields, apply_pagination_query_pairs, execute_request,
        extract_next_link_url, extract_rows, join_url, normalize_base_url, page_is_exhausted,
        resolve_value_source, set_path_value, trace_http_endpoint,
    };
    use crate::backends::http::ProviderQueryError;
    use crate::backends::http::target::HttpFetchTarget;
    use crate::backends::shared::template::{EMPTY_MAP, RenderContext};
    use coral_spec::PaginationMode;
    use coral_spec::backends::http::{HttpSourceManifest, HttpTableSpec, RateLimitSpec};
    use coral_spec::{
        AuthSpec, BodySpec, HttpMethod, PaginationSpec, ParsedTemplate, RequestSpec,
        ResponseBodyFormat, RowStrategy, ValidatedPaginationMode, ValueSourceSpec,
        parse_source_manifest_value,
    };

    fn parse_http_manifest(value: serde_json::Value) -> HttpSourceManifest {
        parse_source_manifest_value(value)
            .expect("manifest should deserialize")
            .as_http()
            .expect("http manifest")
            .clone()
    }

    async fn spawn_hanging_http_server() -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hanging http server");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept hanging request");
            let _socket = socket;
            std::future::pending::<()>().await;
        });

        (format!("http://{addr}"), task)
    }

    fn test_http_table_spec(columns: &serde_json::Value, request: &RequestSpec) -> HttpTableSpec {
        parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "tables": [{
                "name": "items",
                "description": "items",
                "request": request_json(request),
                "columns": columns
            }]
        }))
        .tables
        .into_iter()
        .next()
        .expect("table should exist")
    }

    fn test_http_request_target(table: &HttpTableSpec) -> HttpFetchTarget {
        HttpFetchTarget::from_resolved_table_request(table, table.request.clone())
    }

    fn test_render_context<'a>(
        filters: &'a HashMap<String, String>,
        args: &'a HashMap<String, String>,
        resolved_inputs: &'a BTreeMap<String, String>,
    ) -> RenderContext<'a> {
        RenderContext::new(filters, args, &EMPTY_MAP, resolved_inputs)
    }

    #[test]
    fn set_path_value_builds_arrays_from_numeric_segments() {
        let mut root = json!({});

        set_path_value(
            &mut root,
            &[
                "Dimensions".to_string(),
                "0".to_string(),
                "Name".to_string(),
            ],
            json!("ClusterName"),
        )
        .expect("path assignment should succeed");
        set_path_value(
            &mut root,
            &[
                "Dimensions".to_string(),
                "0".to_string(),
                "Value".to_string(),
            ],
            json!("titaness"),
        )
        .expect("path assignment should succeed");
        set_path_value(
            &mut root,
            &["Statistics".to_string(), "0".to_string()],
            json!("Average"),
        )
        .expect("path assignment should succeed");

        assert_eq!(
            root,
            json!({
                "Dimensions": [{
                    "Name": "ClusterName",
                    "Value": "titaness"
                }],
                "Statistics": ["Average"]
            })
        );
    }

    fn request_json(request: &RequestSpec) -> serde_json::Value {
        let body = match &request.body {
            BodySpec::Json { fields } => fields
                .iter()
                .map(|field| {
                    json!({
                        "path": field.path,
                        "value": value_source_json(&field.value),
                    })
                })
                .collect::<Vec<_>>(),
            BodySpec::Text { .. } => Vec::new(),
        };
        json!({
            "method": format!("{:?}", request.method),
            "path": request.path,
            "query": request.query.iter().map(|query| json!({
                "name": query.name,
                "value": value_source_json(&query.value),
            })).collect::<Vec<_>>(),
            "body": body,
            "headers": request.headers.iter().map(|header| json!({
                "name": header.name,
                "value": value_source_json(&header.value),
            })).collect::<Vec<_>>(),
        })
    }

    fn value_source_json(value: &ValueSourceSpec) -> serde_json::Value {
        match value {
            ValueSourceSpec::Literal { value } => json!({
                "from": "literal",
                "value": value,
            }),
            ValueSourceSpec::Filter { key, default } => json!({
                "from": "filter",
                "key": key,
                "default": default,
            }),
            ValueSourceSpec::FilterInt { key, default } => json!({
                "from": "filter_int",
                "key": key,
                "default": default,
            }),
            ValueSourceSpec::FilterBool { key, default } => json!({
                "from": "filter_bool",
                "key": key,
                "default": default,
            }),
            ValueSourceSpec::FilterSplit {
                key,
                separator,
                part,
            } => json!({
                "from": "filter_split",
                "key": key,
                "separator": separator,
                "part": part,
            }),
            ValueSourceSpec::FilterSplitInt {
                key,
                separator,
                part,
            } => json!({
                "from": "filter_split_int",
                "key": key,
                "separator": separator,
                "part": part,
            }),
            ValueSourceSpec::Arg { key, default } => json!({
                "from": "arg",
                "key": key,
                "default": default,
            }),
            ValueSourceSpec::ArgInt { key, default } => json!({
                "from": "arg_int",
                "key": key,
                "default": default,
            }),
            ValueSourceSpec::ArgBool { key, default } => json!({
                "from": "arg_bool",
                "key": key,
                "default": default,
            }),
            ValueSourceSpec::ArgSplit {
                key,
                separator,
                part,
            } => json!({
                "from": "arg_split",
                "key": key,
                "separator": separator,
                "part": part,
            }),
            ValueSourceSpec::ArgSplitInt {
                key,
                separator,
                part,
            } => json!({
                "from": "arg_split_int",
                "key": key,
                "separator": separator,
                "part": part,
            }),
            ValueSourceSpec::Input { key } => json!({
                "from": "input",
                "key": key,
            }),
            ValueSourceSpec::Template { template } => json!({
                "from": "template",
                "template": template,
            }),
            ValueSourceSpec::State { key } => json!({
                "from": "state",
                "key": key,
            }),
            ValueSourceSpec::NowEpochMinusSeconds { seconds } => json!({
                "from": "now_epoch_minus_seconds",
                "seconds": seconds,
            }),
        }
    }

    #[test]
    fn normalize_base_url_adds_https_scheme_for_host_only_values() {
        assert_eq!(
            normalize_base_url("eu.posthog.com"),
            "https://eu.posthog.com"
        );
        assert_eq!(
            normalize_base_url("//api.example.com"),
            "https://api.example.com"
        );
    }

    #[test]
    fn normalize_base_url_preserves_existing_schemes() {
        assert_eq!(
            normalize_base_url("https://api.github.com"),
            "https://api.github.com"
        );
        assert_eq!(
            normalize_base_url("http://localhost:8080"),
            "http://localhost:8080"
        );
    }

    #[test]
    fn trace_http_endpoint_extracts_host_and_port() {
        let endpoint = trace_http_endpoint("https://api.example.com/v1/items");
        assert_eq!(endpoint.server_address.as_deref(), Some("api.example.com"));
        assert_eq!(endpoint.server_port, Some(443));

        let endpoint = trace_http_endpoint("http://localhost:8080/v1/items");
        assert_eq!(endpoint.server_address.as_deref(), Some("localhost"));
        assert_eq!(endpoint.server_port, Some(8080));
    }

    #[test]
    fn trace_http_endpoint_ignores_unparseable_urls() {
        let endpoint = trace_http_endpoint("/v1/items");
        assert!(endpoint.server_address.is_none());
        assert!(endpoint.server_port.is_none());
    }

    #[test]
    fn join_url_handles_relative_paths() {
        assert_eq!(
            join_url("https://api.example.com", "/v1/resources").unwrap(),
            "https://api.example.com/v1/resources"
        );
        assert_eq!(
            join_url("https://api.example.com/", "v1/resources").unwrap(),
            "https://api.example.com/v1/resources"
        );
    }

    #[test]
    fn join_url_rejects_absolute_paths() {
        let err = join_url("https://api.example.com", "https://next.example.com/page").unwrap_err();
        assert!(
            err.to_string()
                .contains("request path must be relative; absolute URLs are not allowed")
        );
    }

    #[test]
    fn extract_next_link_url_resolves_relative_links_on_same_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_static("</v1/resources?page=2>; rel=\"next\""),
        );

        let next = extract_next_link_url(&headers, "https://api.example.com", false).unwrap();

        assert_eq!(
            next,
            Some("https://api.example.com/v1/resources?page=2".to_string())
        );
    }

    #[test]
    fn extract_next_link_url_rejects_cross_origin_absolute_links() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_static("<https://attacker.example/steal>; rel=\"next\""),
        );

        let err = extract_next_link_url(&headers, "https://api.example.com", false).unwrap_err();

        assert!(
            err.to_string()
                .contains("pagination next link must stay on origin https://api.example.com")
        );
    }

    #[test]
    fn extract_next_link_url_rejects_misordered_link_delimiters() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "link",
            HeaderValue::from_static(">/v1/resources?page=2<; rel=\"next\""),
        );

        let err = extract_next_link_url(&headers, "https://api.example.com", false).unwrap_err();

        assert!(
            err.to_string()
                .contains("invalid pagination Link header item")
        );
    }

    #[test]
    fn resolve_value_source_uses_provider_scoped_credentials() {
        let resolved_inputs = BTreeMap::from([("API_KEY".to_string(), "alpha-secret".to_string())]);

        let value = resolve_value_source(
            &ValueSourceSpec::Input {
                key: "API_KEY".to_string(),
            },
            &test_render_context(&HashMap::new(), &HashMap::new(), &resolved_inputs),
        )
        .expect("input lookup should succeed");

        assert_eq!(value, Some(json!("alpha-secret")));
    }

    #[test]
    fn resolve_value_source_uses_declared_store_without_fallback() {
        let resolved_inputs = BTreeMap::new();

        let value = resolve_value_source(
            &ValueSourceSpec::Input {
                key: "API_KEY".to_string(),
            },
            &test_render_context(&HashMap::new(), &HashMap::new(), &resolved_inputs),
        )
        .expect("input lookup should succeed");

        assert_eq!(value, None);
    }

    #[test]
    fn resolve_value_source_parses_filter_ints_as_numbers() {
        let filters = HashMap::from([("start_time".to_string(), "1700000000000000".to_string())]);

        let value = resolve_value_source(
            &ValueSourceSpec::FilterInt {
                key: "start_time".to_string(),
                default: None,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect("integer filter should resolve");

        assert_eq!(value, Some(json!(1_700_000_000_000_000_i64)));
    }

    #[test]
    fn resolve_value_source_rejects_invalid_filter_ints() {
        let filters = HashMap::from([("start_time".to_string(), "not-a-number".to_string())]);

        let error = resolve_value_source(
            &ValueSourceSpec::FilterInt {
                key: "start_time".to_string(),
                default: None,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect_err("invalid integer filter should fail");

        assert!(
            error
                .to_string()
                .contains("filter 'start_time' value 'not-a-number' is not a valid i64")
        );
    }

    #[test]
    fn resolve_value_source_splits_filter_parts() {
        let filters = HashMap::from([("issue_identifier".to_string(), "SOURCE-496".to_string())]);

        let team = resolve_value_source(
            &ValueSourceSpec::FilterSplit {
                key: "issue_identifier".to_string(),
                separator: "-".to_string(),
                part: 0,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect("split filter should resolve");
        let number = resolve_value_source(
            &ValueSourceSpec::FilterSplitInt {
                key: "issue_identifier".to_string(),
                separator: "-".to_string(),
                part: 1,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect("split integer filter should resolve");

        assert_eq!(team, Some(json!("SOURCE")));
        assert_eq!(number, Some(json!(496)));
    }

    #[test]
    fn resolve_value_source_splits_function_argument_parts() {
        let args = HashMap::from([("issue".to_string(), "SOURCE-496".to_string())]);

        let team = resolve_value_source(
            &ValueSourceSpec::ArgSplit {
                key: "issue".to_string(),
                separator: "-".to_string(),
                part: 0,
            },
            &test_render_context(&HashMap::new(), &args, &BTreeMap::new()),
        )
        .expect("split function argument should resolve");
        let number = resolve_value_source(
            &ValueSourceSpec::ArgSplitInt {
                key: "issue".to_string(),
                separator: "-".to_string(),
                part: 1,
            },
            &test_render_context(&HashMap::new(), &args, &BTreeMap::new()),
        )
        .expect("split integer function argument should resolve");

        assert_eq!(team, Some(json!("SOURCE")));
        assert_eq!(number, Some(json!(496)));
    }

    #[test]
    fn resolve_value_source_rejects_missing_filter_split_part() {
        let filters = HashMap::from([("issue_identifier".to_string(), "SOURCE496".to_string())]);

        let error = resolve_value_source(
            &ValueSourceSpec::FilterSplit {
                key: "issue_identifier".to_string(),
                separator: "-".to_string(),
                part: 1,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect_err("missing split part should fail");

        assert!(
            error.to_string().contains(
                "filter 'issue_identifier' value 'SOURCE496' does not contain split part 1"
            )
        );
    }

    #[test]
    fn resolve_value_source_rejects_missing_function_argument_split_part() {
        let args = HashMap::from([("issue".to_string(), "SOURCE496".to_string())]);

        let error = resolve_value_source(
            &ValueSourceSpec::ArgSplit {
                key: "issue".to_string(),
                separator: "-".to_string(),
                part: 1,
            },
            &test_render_context(&HashMap::new(), &args, &BTreeMap::new()),
        )
        .expect_err("missing split function argument part should fail");

        assert!(
            error.to_string().contains(
                "function argument 'issue' value 'SOURCE496' does not contain split part 1"
            )
        );
    }

    #[test]
    fn resolve_value_source_rejects_missing_filter_split_int_part() {
        let filters = HashMap::from([("issue_identifier".to_string(), "SOURCE496".to_string())]);

        let error = resolve_value_source(
            &ValueSourceSpec::FilterSplitInt {
                key: "issue_identifier".to_string(),
                separator: "-".to_string(),
                part: 1,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect_err("missing split integer part should fail");

        assert!(
            error.to_string().contains(
                "filter 'issue_identifier' value 'SOURCE496' does not contain split part 1"
            )
        );
    }

    #[test]
    fn resolve_value_source_rejects_invalid_function_argument_split_int_part() {
        let args = HashMap::from([("issue".to_string(), "SOURCE-abc".to_string())]);

        let error = resolve_value_source(
            &ValueSourceSpec::ArgSplitInt {
                key: "issue".to_string(),
                separator: "-".to_string(),
                part: 1,
            },
            &test_render_context(&HashMap::new(), &args, &BTreeMap::new()),
        )
        .expect_err("invalid split function argument int should fail");

        assert!(
            error
                .to_string()
                .contains("function argument 'issue' split part 1 value 'abc' is not a valid i64")
        );
    }

    #[test]
    fn resolve_value_source_parses_filter_bools_as_bools() {
        let filters = HashMap::from([("descending".to_string(), "false".to_string())]);

        let value = resolve_value_source(
            &ValueSourceSpec::FilterBool {
                key: "descending".to_string(),
                default: None,
            },
            &test_render_context(&filters, &HashMap::new(), &BTreeMap::new()),
        )
        .expect("bool filter should resolve");

        assert_eq!(value, Some(json!(false)));
    }

    #[test]
    fn backend_client_requires_source_scoped_credentials() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "auth": {
                "type": "HeaderAuth",
                "headers": [{
                    "name": "Authorization",
                    "from": "template",
                    "template": "Bearer {{input.API_KEY}}"
                }]
            },
            "inputs": {
                "API_KEY": { "kind": "secret" }
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));
        let source_secrets = BTreeMap::new();

        let error = HttpSourceClient::from_manifest(
            &manifest,
            &source_secrets,
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect_err("missing source-scoped credentials must fail");

        assert!(
            error
                .to_string()
                .contains("missing source input 'API_KEY' for template token")
        );
    }

    #[test]
    fn backend_client_rejects_unresolved_table_request_path_inputs() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "inputs": {
                "API_KEY": { "kind": "secret" },
                "ACCOUNT_ID": { "kind": "variable" }
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": {
                    "path": "/{{input.ACCOUNT_ID}}/items"
                },
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));

        let error = HttpSourceClient::from_manifest(
            &manifest,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect_err("missing table request path inputs must fail");

        assert!(
            error
                .to_string()
                .contains("table 'items' request path could not be resolved")
        );
    }

    #[test]
    fn backend_client_rejects_unresolved_table_request_header_inputs() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "inputs": {
                "ACCOUNT_ID": { "kind": "variable" }
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": {
                    "path": "/items",
                    "headers": [{
                        "name": "X-Account",
                        "from": "input",
                        "key": "ACCOUNT_ID"
                    }]
                },
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));

        let error = HttpSourceClient::from_manifest(
            &manifest,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect_err("missing table request header inputs must fail");

        assert!(
            error
                .to_string()
                .contains("table 'items' request header 'X-Account' could not be resolved")
        );
    }

    #[test]
    fn backend_client_rejects_unresolved_table_request_query_inputs() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "inputs": {
                "ACCOUNT_ID": { "kind": "variable" }
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": {
                    "path": "/items",
                    "query": [{
                        "name": "account_id",
                        "from": "input",
                        "key": "ACCOUNT_ID"
                    }]
                },
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));

        let error = HttpSourceClient::from_manifest(
            &manifest,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect_err("missing table request query inputs must fail");

        assert!(
            error
                .to_string()
                .contains("table 'items' request query param 'account_id' could not be resolved")
        );
    }

    #[test]
    fn backend_client_rejects_unresolved_table_request_body_inputs() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "inputs": {
                "ACCOUNT_ID": { "kind": "variable" }
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": {
                    "method": "POST",
                    "path": "/items",
                    "body": [{
                        "path": ["account", "id"],
                        "from": "input",
                        "key": "ACCOUNT_ID"
                    }]
                },
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));

        let error = HttpSourceClient::from_manifest(
            &manifest,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect_err("missing table request body inputs must fail");

        assert!(
            error
                .to_string()
                .contains("table 'items' request body field 'account.id' could not be resolved")
        );
    }

    #[test]
    fn backend_client_rejects_unresolved_request_route_inputs() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "inputs": {
                "ACCOUNT_ID": { "kind": "variable" }
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "requests": [{
                    "when_filters": ["account_id"],
                    "method": "GET",
                    "path": "/{{input.ACCOUNT_ID}}/items"
                }],
                "filters": [{
                    "name": "account_id"
                }],
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));

        let error = HttpSourceClient::from_manifest(
            &manifest,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect_err("missing request route inputs must fail");

        assert!(error.to_string().contains(
            "table 'items' request route for filters [account_id] path could not be resolved"
        ));
    }

    #[test]
    fn backend_client_rejects_unresolved_function_request_inputs() {
        let cases = [
            (
                "path",
                json!({
                    "path": "/{{input.ACCOUNT_ID}}/items"
                }),
                "function 'search_items' request path could not be resolved",
            ),
            (
                "header",
                json!({
                    "path": "/items",
                    "headers": [{
                        "name": "X-Account",
                        "from": "input",
                        "key": "ACCOUNT_ID"
                    }]
                }),
                "function 'search_items' request header 'X-Account' could not be resolved",
            ),
            (
                "query",
                json!({
                    "path": "/items",
                    "query": [{
                        "name": "account_id",
                        "from": "input",
                        "key": "ACCOUNT_ID"
                    }]
                }),
                "function 'search_items' request query param 'account_id' could not be resolved",
            ),
            (
                "body",
                json!({
                    "method": "POST",
                    "path": "/items",
                    "body": [{
                        "path": ["account", "id"],
                        "from": "input",
                        "key": "ACCOUNT_ID"
                    }]
                }),
                "function 'search_items' request body field 'account.id' could not be resolved",
            ),
        ];

        for (name, request, expected) in cases {
            let manifest = parse_http_manifest(json!({
                "dsl_version": 3,
                "name": "alpha",
                "version": "0.1.0",
                "backend": "http",
                "base_url": "https://api.example.com",
                "inputs": {
                    "ACCOUNT_ID": { "kind": "variable" }
                },
                "tables": [{
                    "name": "items",
                    "description": "items",
                    "request": { "path": "/items" },
                    "columns": [{
                        "name": "id",
                        "type": "Utf8"
                    }]
                }],
                "functions": [{
                    "name": "search_items",
                    "description": "Search items",
                    "request": request,
                    "columns": [{
                        "name": "id",
                        "type": "Utf8"
                    }]
                }]
            }));

            let error = HttpSourceClient::from_manifest(
                &manifest,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &HashMap::new(),
            )
            .expect_err(&format!(
                "missing function request {name} input should fail"
            ));

            assert!(
                error.to_string().contains(expected),
                "unexpected error for {name}: {error}"
            );
        }
    }

    #[test]
    fn apply_pagination_query_pairs_uses_typed_offset_param() {
        let table = test_http_table_spec(
            &json!([]),
            &RequestSpec {
                method: HttpMethod::GET,
                path: ParsedTemplate::parse("/items").expect("template"),
                query: vec![],
                body: BodySpec::default(),
                headers: vec![],
            },
        );
        let pagination = PaginationSpec {
            mode: PaginationMode::Offset,
            page_size: Some(coral_spec::PageSizeSpec {
                default: 25,
                max: 100,
                query_param: Some("limit".to_string()),
                body_path: vec![],
            }),
            offset_param: Some("start".to_string()),
            offset_start: 10,
            offset_step: Some(25),
            ..PaginationSpec::default()
        }
        .validated("demo", "items")
        .unwrap();
        let mut params = Vec::new();
        let state = PageState {
            offset: 35,
            ..PageState::default()
        };

        let target = test_http_request_target(&table);
        apply_pagination_query_pairs(&mut params, &target, &pagination, &state, Some(25)).unwrap();

        assert_eq!(
            params,
            vec![
                ("limit".to_string(), "25".to_string()),
                ("start".to_string(), "35".to_string()),
            ]
        );
        assert!(matches!(
            pagination.mode,
            ValidatedPaginationMode::Offset(_)
        ));
    }

    #[test]
    fn apply_pagination_body_fields_rejects_declared_text_body_even_when_absent() {
        let table = test_http_table_spec(
            &json!([]),
            &RequestSpec {
                method: HttpMethod::GET,
                path: ParsedTemplate::parse("/items").expect("template"),
                query: vec![],
                body: BodySpec::default(),
                headers: vec![],
            },
        );
        let body_spec = BodySpec::Text {
            content: ValueSourceSpec::Filter {
                key: "sql".to_string(),
                default: None,
            },
        };
        let pagination = PaginationSpec {
            page_size: Some(coral_spec::PageSizeSpec {
                default: 25,
                max: 100,
                query_param: None,
                body_path: vec!["limit".to_string()],
            }),
            ..PaginationSpec::default()
        }
        .validated("demo", "items")
        .unwrap();
        let mut body = None;
        let target = test_http_request_target(&table);

        let error = apply_pagination_body_fields(
            &mut body,
            &body_spec,
            &target,
            &pagination,
            &PageState::default(),
            Some(25),
        )
        .expect_err("text request bodies must not receive pagination body fields");

        assert!(
            error
                .to_string()
                .contains("pagination body fields are not supported with text request bodies")
        );
        assert!(body.is_none());
    }

    #[test]
    fn page_is_exhausted_handles_empty_short_and_full_pages() {
        for (rows_on_page, page_size, expected) in
            [(0, Some(50), true), (24, Some(25), true), (24, None, false)]
        {
            assert_eq!(page_is_exhausted(rows_on_page, page_size), expected);
        }
    }

    fn make_table_with_row_strategy(
        strategy: RowStrategy,
        rows_path: Vec<String>,
    ) -> coral_spec::backends::http::HttpTableSpec {
        let mut table = test_http_table_spec(
            &json!([]),
            &RequestSpec {
                method: HttpMethod::GET,
                path: ParsedTemplate::parse("/items").expect("template"),
                query: vec![],
                body: BodySpec::default(),
                headers: vec![],
            },
        );
        table.response.rows_path = rows_path;
        table.response.row_strategy = strategy;
        table
    }

    #[test]
    fn dict_entries_flattens_object_values() {
        let table =
            make_table_with_row_strategy(RowStrategy::DictEntries, vec!["result".to_string()]);
        let payload = json!({
            "result": {
                "2024-02-27 EST": {"Open": 8.29, "Close": 8.15},
                "2024-02-28 EST": {"Open": 7.85, "Close": 7.90}
            }
        });

        let rows = extract_rows(&test_http_request_target(&table), &payload);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert!(row.get("_key").is_some());
            assert!(row.get("Open").is_some());
            assert!(row.get("Close").is_some());
        }

        let keys: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.get("_key").and_then(|value| value.as_str()))
            .collect();
        assert!(keys.contains(&"2024-02-27 EST"));
        assert!(keys.contains(&"2024-02-28 EST"));
    }

    #[test]
    fn dict_entries_uses_value_field_for_scalars() {
        let table =
            make_table_with_row_strategy(RowStrategy::DictEntries, vec!["result".to_string()]);
        let payload = json!({
            "result": {
                "2020-01-15 EST": 0.058,
                "2020-06-12 EST": 0.2
            }
        });

        let rows = extract_rows(&test_http_request_target(&table), &payload);
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert!(row.get("_key").is_some());
            assert!(row.get("_value").is_some());
        }
    }

    #[test]
    fn dict_entries_returns_empty_for_null() {
        let table =
            make_table_with_row_strategy(RowStrategy::DictEntries, vec!["result".to_string()]);
        let payload = json!({ "result": null });

        let rows = extract_rows(&test_http_request_target(&table), &payload);
        assert!(rows.is_empty());
    }

    #[test]
    fn dict_entries_returns_empty_for_missing_path() {
        let table =
            make_table_with_row_strategy(RowStrategy::DictEntries, vec!["missing".to_string()]);
        let payload = json!({ "result": { "a": 1 } });

        let rows = extract_rows(&test_http_request_target(&table), &payload);
        assert!(rows.is_empty());
    }

    #[test]
    fn dict_entries_returns_empty_for_non_object() {
        let table =
            make_table_with_row_strategy(RowStrategy::DictEntries, vec!["result".to_string()]);
        let payload = json!({ "result": [1, 2, 3] });

        let rows = extract_rows(&test_http_request_target(&table), &payload);
        assert!(rows.is_empty());
    }

    #[test]
    fn dict_entries_empty_dict_returns_empty() {
        let table =
            make_table_with_row_strategy(RowStrategy::DictEntries, vec!["result".to_string()]);
        let payload = json!({ "result": {} });

        let rows = extract_rows(&test_http_request_target(&table), &payload);
        assert!(rows.is_empty());
    }

    #[test]
    fn series_point_list_skips_malformed_points() {
        let table = make_table_with_row_strategy(RowStrategy::SeriesPointList, vec![]);
        let payload = json!({
            "series": [{
                "metric": "system.cpu.user",
                "scope": "host:demo",
                "pointlist": [
                    [1_710_000_000, 42.5],
                    [1_710_000_060],
                    [null, 1.0],
                    ["1710000120", 2.0],
                    [1_710_000_180, "3.0"],
                    {"timestamp": 1_710_000_240, "value": 4.0}
                ]
            }]
        });

        let rows = extract_rows(&test_http_request_target(&table), &payload);

        assert_eq!(
            rows,
            vec![json!({
                "metric": "system.cpu.user",
                "scope": "host:demo",
                "timestamp": 1_710_000_000_i64,
                "value": 42.5
            })]
        );
    }

    #[test]
    fn parse_manifest_accepts_dict_entries_row_strategy() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "response": {
                    "rows_path": ["result"],
                    "row_strategy": "dict_entries"
                },
                "columns": [{
                    "name": "_key",
                    "type": "Utf8"
                }]
            }]
        }));
        let table = manifest.tables.first().expect("HTTP table");
        assert!(matches!(
            table.response.row_strategy,
            RowStrategy::DictEntries
        ));
    }

    #[tokio::test]
    async fn execute_request_times_out_when_upstream_stalls() {
        let (base_url, task) = spawn_hanging_http_server().await;
        let request_timeout = Duration::from_millis(100);
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .expect("build test client");
        let url = format!("{base_url}/items");
        let query_pairs = vec![("api_key".to_string(), "secret-token".to_string())];
        let filters = HashMap::new();
        let args = HashMap::new();
        let state = HashMap::new();
        let resolved_inputs = BTreeMap::new();
        let render_context = RenderContext::new(&filters, &args, &state, &resolved_inputs);

        let error = execute_request(
            &http,
            request_timeout,
            TestOutgoingHttpRequest {
                auth: &AuthSpec::default(),
                request_headers: &[],
                request_authenticators: &HashMap::new(),
                table_headers: &[],
                table_name: "items",
                method: HttpMethod::GET,
                base_url: &base_url,
                url: &url,
                query_pairs: &query_pairs,
                body: None,
                response_format: ResponseBodyFormat::default(),
                source_schema: "demo",
                rate_limit: &RateLimitSpec::default(),
                render_context,
                allow_404_empty: false,
                link_header_require_results: false,
            },
        )
        .await
        .expect_err("hung upstream should time out");

        match error {
            DataFusionError::External(inner) => {
                let provider_error = inner
                    .downcast_ref::<ProviderQueryError>()
                    .expect("timeout should be a provider query error");
                match provider_error {
                    ProviderQueryError::Request {
                        source_schema,
                        table,
                        detail,
                        timed_out,
                        ..
                    } => {
                        assert_eq!(source_schema, "demo");
                        assert_eq!(table, "items");
                        assert!(*timed_out);
                        assert!(detail.contains("timed out"));
                        assert!(!detail.contains("secret-token"));
                    }
                    other => panic!("expected request provider error, got {other:?}"),
                }
                let structured = provider_error.to_structured();
                assert_eq!(
                    structured.metadata().get("url").map(String::as_str),
                    Some(format!("{base_url}/items").as_str())
                );
                assert!(!structured.detail().contains("secret-token"));
            }
            other => panic!("expected external provider error, got {other:?}"),
        }
        task.abort();
    }

    #[test]
    fn parse_manifest_accepts_source_rate_limit_policy() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "rate_limit": {
                "extra_statuses": [403],
                "remaining_header": "X-RateLimit-Remaining",
                "reset_header": "X-RateLimit-Reset"
            },
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "columns": [{
                    "name": "id",
                    "type": "Utf8"
                }]
            }]
        }));

        assert_eq!(manifest.rate_limit.extra_statuses, vec![403]);
        assert_eq!(
            manifest.rate_limit.remaining_header.as_deref(),
            Some("X-RateLimit-Remaining")
        );
    }

    // ── Cache tests ───────────────────────────────────────────────────────────

    fn cached_users_manifest(base_url: &str) -> HttpSourceManifest {
        parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": base_url,
            "tables": [{
                "name": "users",
                "description": "Users",
                "request": { "path": "/api/users" },
                "response": { "rows_path": ["data"] },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [
                    { "name": "id", "type": "Int64" },
                    { "name": "name", "type": "Utf8" }
                ]
            }]
        }))
    }

    fn build_test_client(manifest: &HttpSourceManifest) -> HttpSourceClient {
        HttpSourceClient::from_manifest(
            manifest,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &HashMap::new(),
        )
        .expect("test client should build")
    }

    fn first_table(manifest: &HttpSourceManifest) -> &HttpTableSpec {
        manifest
            .tables
            .first()
            .expect("manifest should have a table")
    }

    async fn fetch_table(
        client: &HttpSourceClient,
        table: &HttpTableSpec,
        filters: &HashMap<String, String>,
        sql_limit: Option<usize>,
    ) -> datafusion::error::Result<Vec<serde_json::Value>> {
        let target = test_http_request_target(table);
        client
            .fetch(&target, filters, &HashMap::new(), sql_limit)
            .await
    }

    #[tokio::test]
    async fn cache_hit_avoids_second_outbound_request() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "id": 1, "name": "Ada" },
                    { "id": 2, "name": "Grace" }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let manifest = cached_users_manifest(&server.uri());
        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        let rows1 = fetch_table(&client, table, &filters, None)
            .await
            .expect("first fetch");
        let rows2 = fetch_table(&client, table, &filters, None)
            .await
            .expect("second fetch from cache");

        assert_eq!(rows1, rows2);
        assert_eq!(rows1.len(), 2);
        // MockServer verifies .expect(1) on drop — panics if != 1 request made
    }

    #[tokio::test]
    async fn cache_miss_on_different_filter_values() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "filters": [{ "name": "status" }],
                "request": {
                    "path": "/api/items",
                    "query": [{ "name": "status", "from": "filter", "key": "status" }]
                },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("status", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("status", "closed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 2 }])))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);

        let rows_open = fetch_table(
            &client,
            table,
            &HashMap::from([("status".into(), "open".into())]),
            None,
        )
        .await
        .expect("open fetch");
        let rows_closed = fetch_table(
            &client,
            table,
            &HashMap::from([("status".into(), "closed".into())]),
            None,
        )
        .await
        .expect("closed fetch");

        assert_ne!(rows_open, rows_closed);
        // Both mocks have .expect(1) — verified on drop
    }

    #[tokio::test]
    async fn cache_miss_on_different_vary_header_values() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "filters": [{ "name": "mode" }],
                "request": {
                    "path": "/api/items",
                    "headers": [{ "name": "X-Mode", "from": "filter", "key": "mode" }]
                },
                "cache": {
                    "mode": "ttl",
                    "ttl": "1h",
                    "vary": { "headers": ["X-Mode"] }
                },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(header("X-Mode", "alpha"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(header("X-Mode", "beta"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 2 }])))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);

        let rows_alpha = fetch_table(
            &client,
            table,
            &HashMap::from([("mode".into(), "alpha".into())]),
            None,
        )
        .await
        .expect("alpha fetch");
        let rows_beta = fetch_table(
            &client,
            table,
            &HashMap::from([("mode".into(), "beta".into())]),
            None,
        )
        .await
        .expect("beta fetch");

        assert_ne!(rows_alpha, rows_beta);
    }

    #[tokio::test]
    async fn cache_second_identical_query_uses_first_result() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "filters": [{ "name": "status" }],
                "request": {
                    "path": "/api/items",
                    "query": [{ "name": "status", "from": "filter", "key": "status" }]
                },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("status", "open"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }])))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::from([("status".to_string(), "open".to_string())]);

        let r1 = fetch_table(&client, table, &filters, None)
            .await
            .expect("first");
        let r2 = fetch_table(&client, table, &filters, None)
            .await
            .expect("second, from cache");
        assert_eq!(r1, r2);
        // .expect(1) verified on drop
    }

    #[tokio::test]
    async fn cache_does_not_cache_failed_responses() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        // Use 400 (not retried, unlike 5xx) to get exactly one request per fetch() call.
        // The second call must still hit the server, proving failed responses are not cached.
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .expect(2)
            .mount(&server)
            .await;

        let manifest = cached_users_manifest(&server.uri());
        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "first call should fail"
        );
        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "second call should also fail"
        );
        // .expect(2) verifies 2 separate outbound requests (no caching of errors)
    }

    #[tokio::test]
    async fn cache_expires_after_ttl() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "id": 1 }] })),
            )
            .expect(2)
            .mount(&server)
            .await;

        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "users",
                "description": "Users",
                "request": { "path": "/api/users" },
                "response": { "rows_path": ["data"] },
                "cache": { "mode": "ttl", "ttl": "1s" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        fetch_table(&client, table, &filters, None)
            .await
            .expect("first fetch");
        // Wait for the 1s TTL to expire
        tokio::time::sleep(Duration::from_millis(1100)).await;
        fetch_table(&client, table, &filters, None)
            .await
            .expect("second fetch after expiry");
        // .expect(2) verifies 2 outbound requests were made
    }

    #[tokio::test]
    async fn cache_disabled_by_default() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "data": [{ "id": 1 }] })),
            )
            .expect(2)
            .mount(&server)
            .await;

        // Manifest with no cache field — caching must stay disabled.
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "users",
                "description": "Users",
                "request": { "path": "/api/users" },
                "response": { "rows_path": ["data"] },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        fetch_table(&client, table, &filters, None)
            .await
            .expect("first");
        fetch_table(&client, table, &filters, None)
            .await
            .expect("second");
        // .expect(2) verifies both calls made it to the server (no caching)
    }

    #[test]
    fn parse_manifest_accepts_cache_ttl_policy() {
        use coral_spec::backends::http::HttpCacheMode;

        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "cache": {
                    "mode": "ttl",
                    "ttl": "5m",
                    "vary": { "headers": ["Accept"] },
                    "max_pages": 50,
                    "max_entry_bytes": 1_048_576
                },
                "columns": [{ "name": "id", "type": "Utf8" }]
            }]
        }));

        let cache = first_table(&manifest)
            .cache
            .as_ref()
            .expect("cache policy should be set");
        assert_eq!(cache.mode, HttpCacheMode::Ttl);
        assert_eq!(cache.ttl.as_secs(), 300);
        assert_eq!(cache.vary_headers, vec!["Accept"]);
        assert_eq!(cache.max_pages, Some(50));
        assert_eq!(cache.max_entry_bytes, Some(1_048_576));
    }

    #[test]
    fn parse_manifest_rejects_overflowing_cache_ttl() {
        let error = parse_source_manifest_value(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "cache": { "mode": "ttl", "ttl": "18446744073709551615h" },
                "columns": [{ "name": "id", "type": "Utf8" }]
            }]
        }))
        .expect_err("overflowing ttl should fail");

        assert!(
            error
                .to_string()
                .contains("cache ttl '18446744073709551615h' overflows u64 seconds"),
            "unexpected ttl overflow error: {error}"
        );
    }

    #[test]
    fn parse_manifest_no_cache_field_gives_none() {
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "alpha",
            "version": "0.1.0",
            "backend": "http",
            "base_url": "https://api.example.com",
            "tables": [{
                "name": "items",
                "description": "items",
                "request": { "path": "/items" },
                "columns": [{ "name": "id", "type": "Utf8" }]
            }]
        }));

        assert!(first_table(&manifest).cache.is_none());
    }

    #[tokio::test]
    async fn cache_different_post_body_causes_miss() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "filters": [{ "name": "status" }],
                "request": {
                    "method": "POST",
                    "path": "/api/items",
                    "body": [{ "path": ["filter"], "from": "filter", "key": "status" }]
                },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        Mock::given(method("POST"))
            .and(path("/api/items"))
            .and(body_json(json!({ "filter": "open" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/items"))
            .and(body_json(json!({ "filter": "closed" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 2 }])))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);

        let rows_open = fetch_table(
            &client,
            table,
            &HashMap::from([("status".into(), "open".into())]),
            None,
        )
        .await
        .expect("open fetch");
        let rows_closed = fetch_table(
            &client,
            table,
            &HashMap::from([("status".into(), "closed".into())]),
            None,
        )
        .await
        .expect("closed fetch");

        assert_ne!(rows_open, rows_closed);
        // Both mocks have .expect(1) — verified on drop
    }

    #[tokio::test]
    async fn cache_different_pagination_state_causes_miss() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // No page_size query_param so the page number is the only URL-level pagination
        // state. This ensures the cache key for page 1 is the same regardless of the
        // SQL limit used by the caller.
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "pagination": {
                    "mode": "page",
                    "page_param": "page",
                    "page_start": 1
                },
                "request": { "path": "/api/items" },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        // Page 1 is fetched once from the server (first call), then served from cache
        // (second call) — same URL so same cache key.
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }, { "id": 2 }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        // Page 2 is only fetched by the second call (different cache key: page=2).
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 3 }])))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);

        // First call: limit=2 → fetches page 1 (2 rows = limit), stops.
        let rows1 = fetch_table(&client, table, &HashMap::new(), Some(2))
            .await
            .expect("first fetch");
        assert_eq!(rows1.len(), 2);

        // Second call: limit=3 → page 1 served from cache (2 rows), page 2 fresh from server.
        let rows2 = fetch_table(&client, table, &HashMap::new(), Some(3))
            .await
            .expect("second fetch");
        assert_eq!(rows2.len(), 3);
        // Page 1 expect(1) and page 2 expect(1) are verified on drop
    }

    #[tokio::test]
    async fn cache_pagination_stores_pages_independently() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "pagination": {
                    "mode": "page",
                    "page_param": "page",
                    "page_start": 1,
                    "page_size": { "default": 2, "max": 100, "query_param": "per_page" }
                },
                "request": { "path": "/api/items" },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        // Both pages are fetched once each on the first run, then served from
        // cache on the second run — total expect(1) per page.
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }, { "id": 2 }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 3 }])))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);

        // First run: fetches both pages from server
        let rows1 = fetch_table(&client, table, &HashMap::new(), None)
            .await
            .expect("first fetch");
        assert_eq!(rows1.len(), 3);

        // Second run: both pages served from cache — no additional server hits
        let rows2 = fetch_table(&client, table, &HashMap::new(), None)
            .await
            .expect("second fetch");
        assert_eq!(rows2, rows1);
        // expect(1) per page verified on mock drop
    }

    #[tokio::test]
    async fn cache_max_pages_limits_cached_pages_per_fetch() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "pagination": {
                    "mode": "page",
                    "page_param": "page",
                    "page_start": 1,
                    "page_size": { "default": 2, "max": 100, "query_param": "per_page" }
                },
                "request": { "path": "/api/items" },
                "cache": { "mode": "ttl", "ttl": "1h", "max_pages": 1 },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!([{ "id": 1 }, { "id": 2 }])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": 3 }])))
            .expect(2)
            .mount(&server)
            .await;

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);

        let rows1 = fetch_table(&client, table, &HashMap::new(), None)
            .await
            .expect("first fetch");
        let rows2 = fetch_table(&client, table, &HashMap::new(), None)
            .await
            .expect("second fetch");

        assert_eq!(rows2, rows1);
    }

    #[tokio::test]
    async fn cache_does_not_cache_5xx_responses() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // 500 triggers 2 retries → 3 requests for first fetch(); served up_to 3 times.
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .up_to_n_times(3)
            .expect(3)
            .mount(&server)
            .await;
        // After the 500 mock is exhausted, the second fetch hits the server and gets 200.
        // If 5xx responses were incorrectly cached, this mock would never be reached.
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": [{ "id": 1, "name": "Ada" }] })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let manifest = cached_users_manifest(&server.uri());
        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "first call should fail with server error"
        );
        let rows = fetch_table(&client, table, &filters, None)
            .await
            .expect("second call should succeed from server — 5xx response was not cached");
        assert_eq!(rows.len(), 1);
        // expect(3) on 500 mock and expect(1) on 200 mock verified on drop
    }

    #[tokio::test]
    async fn cache_does_not_cache_429_responses() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Retry-After: 60 exceeds MAX_SHORT_RETRY_AFTER (15s) → rate limit fails
        // immediately without retrying, so each fetch() makes exactly one request.
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(429).append_header("Retry-After", "60"))
            .expect(2)
            .mount(&server)
            .await;

        let manifest = cached_users_manifest(&server.uri());
        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "first call should fail with rate-limit error"
        );
        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "second call should also fail — 429 responses are not cached"
        );
        // expect(2) verifies 2 outbound requests (no caching of rate-limit errors)
    }

    #[tokio::test]
    async fn cache_does_not_cache_malformed_json() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(200).set_body_string("this is not json"))
            .expect(2)
            .mount(&server)
            .await;

        let manifest = cached_users_manifest(&server.uri());
        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "first call should fail to decode"
        );
        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "second call should also fail — malformed JSON is not cached"
        );
        // expect(2) verifies 2 outbound requests (decode error, no cache write)
    }

    #[tokio::test]
    async fn cache_does_not_cache_allow_404_empty() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/items"))
            .respond_with(ResponseTemplate::new(404))
            .expect(2)
            .mount(&server)
            .await;

        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "items",
                "description": "Items",
                "request": { "path": "/api/items" },
                "response": { "allow_404_empty": true },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        let rows1 = fetch_table(&client, table, &filters, None)
            .await
            .expect("first");
        assert!(rows1.is_empty(), "allow_404_empty should return empty rows");

        let rows2 = fetch_table(&client, table, &filters, None)
            .await
            .expect("second");
        assert!(rows2.is_empty());
        // expect(2) verifies both calls hit the server (empty result is not cached)
    }

    #[tokio::test]
    async fn cache_skips_oversized_entry_without_failing() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    { "id": 1, "name": "Ada" },
                    { "id": 2, "name": "Grace" }
                ]
            })))
            .expect(2) // entry is never cached, so both calls hit server
            .mount(&server)
            .await;

        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "users",
                "description": "Users",
                "request": { "path": "/api/users" },
                "response": { "rows_path": ["data"] },
                "cache": { "mode": "ttl", "ttl": "1h", "max_entry_bytes": 10 },
                "columns": [
                    { "name": "id", "type": "Int64" },
                    { "name": "name", "type": "Utf8" }
                ]
            }]
        }));

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        let rows1 = fetch_table(&client, table, &filters, None)
            .await
            .expect("first");
        assert_eq!(
            rows1.len(),
            2,
            "rows should be returned even when entry is skipped"
        );

        let rows2 = fetch_table(&client, table, &filters, None)
            .await
            .expect("second");
        assert_eq!(rows2, rows1);
        // expect(2) verifies both calls hit server (oversized entry was not stored)
    }

    #[tokio::test]
    async fn cache_does_not_cache_ok_path_false() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/users"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "ok": false, "error": "rate limited" })),
            )
            .expect(2)
            .mount(&server)
            .await;

        let manifest = parse_http_manifest(json!({
            "dsl_version": 3,
            "name": "demo",
            "version": "0.1.0",
            "backend": "http",
            "base_url": server.uri(),
            "tables": [{
                "name": "users",
                "description": "Users",
                "request": { "path": "/api/users" },
                "response": {
                    "ok_path": ["ok"],
                    "error_path": ["error"]
                },
                "cache": { "mode": "ttl", "ttl": "1h" },
                "columns": [{ "name": "id", "type": "Int64" }]
            }]
        }));

        let client = build_test_client(&manifest);
        let table = first_table(&manifest);
        let filters = HashMap::new();

        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "first call should fail: ok_path=false"
        );
        assert!(
            fetch_table(&client, table, &filters, None).await.is_err(),
            "second call should also fail — ok_path=false response was not cached"
        );
        // expect(2) verifies both calls hit the server (bad response not cached)
    }
}
