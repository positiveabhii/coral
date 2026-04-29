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

use crate::{
    AuthSpec, ColumnSpec, FilterSpec, ManifestError, ManifestInputKind, ManifestInputSpec,
    PaginationSpec, ParsedTemplate, RequestRouteSpec, RequestSpec, ResponseSpec, Result,
    SourceManifestCommon, TableCommon,
    inputs::collect_source_inputs_proto,
    proto::v1 as specv1,
    proto_normalize::{
        auth_from_proto, pagination_from_proto, request_from_proto, request_routes_from_proto,
        response_from_proto, source_common_from_proto, table_common_from_proto,
    },
    validate::validate_template,
    validate_http_table, validate_test_queries,
};

/// Provider-specific response hints for classifying and delaying rate-limit retries.
#[derive(Debug, Clone, Default)]
pub struct RateLimitSpec {
    pub extra_statuses: Vec<u16>,
    pub retry_after_header: Option<String>,
    pub remaining_header: Option<String>,
    pub reset_header: Option<String>,
}

/// Validated top-level manifest for an HTTP-backed source.
#[derive(Debug, Clone)]
pub struct HttpSourceManifest {
    pub common: SourceManifestCommon,
    pub base_url: ParsedTemplate,
    pub auth: AuthSpec,
    pub rate_limit: RateLimitSpec,
    pub tables: Vec<HttpTableSpec>,
    pub declared_inputs: Vec<ManifestInputSpec>,
}

/// One validated HTTP table declaration.
#[derive(Debug, Clone)]
pub struct HttpTableSpec {
    pub common: TableCommon,
    pub request: RequestSpec,
    pub requests: Vec<RequestRouteSpec>,
    pub response: ResponseSpec,
    pub pagination: PaginationSpec,
}

impl HttpTableSpec {
    #[must_use]
    /// Returns the stable table name.
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
    /// Selects the most specific request route that matches the provided
    /// filter set, or falls back to the default request.
    pub fn resolve_request(&self, provided_filters: &HashSet<String>) -> &RequestSpec {
        let mut best_match: Option<&RequestRouteSpec> = None;
        let mut best_specificity = 0usize;

        for route in &self.requests {
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

        best_match.map_or(&self.request, |route| &route.request)
    }
}

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

impl HttpTableSpec {
    fn from_proto(table: &specv1::TableSpec, schema: &str) -> Result<HttpTableSpec> {
        let common = table_common_from_proto(table)?;
        let request = request_from_proto(table.request.as_ref())?;
        let requests = request_routes_from_proto(&table.requests)?;
        let response = response_from_proto(table.response.as_ref())?;
        let pagination = pagination_from_proto(table.pagination.as_ref())?;
        validate_http_table(
            schema,
            &common.name,
            &common.filters,
            &common.columns,
            &request,
            &requests,
            &pagination,
        )?;

        Ok(HttpTableSpec {
            common,
            request,
            requests,
            response,
            pagination,
        })
    }
}

impl HttpSourceManifest {
    pub(crate) fn parse_manifest_proto(manifest: &specv1::SourceManifest) -> Result<Self> {
        let declared_inputs = collect_source_inputs_proto(manifest)?;
        validate_test_queries(&manifest.name, &manifest.test_queries)?;
        let common = source_common_from_proto(manifest);
        let tables = manifest
            .tables
            .iter()
            .map(|table| HttpTableSpec::from_proto(table, &common.name))
            .collect::<Result<Vec<_>>>()?;
        let base_url = ParsedTemplate::parse(&manifest.base_url)?;
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
            auth: auth_from_proto(manifest.auth.as_ref())?,
            rate_limit: rate_limit_from_proto(manifest.rate_limit.as_ref())?,
            tables,
            declared_inputs,
        })
    }
}

fn rate_limit_from_proto(rate_limit: Option<&specv1::RateLimitSpec>) -> Result<RateLimitSpec> {
    let Some(rate_limit) = rate_limit else {
        return Ok(RateLimitSpec::default());
    };
    Ok(RateLimitSpec {
        extra_statuses: rate_limit
            .extra_statuses
            .iter()
            .map(|status| {
                u16::try_from(*status).map_err(|_| {
                    ManifestError::validation(format!(
                        "source manifest rate_limit extra status {status} exceeds supported u16 range"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()?,
        retry_after_header: rate_limit.retry_after_header.clone(),
        remaining_header: rate_limit.remaining_header.clone(),
        reset_header: rate_limit.reset_header.clone(),
    })
}

#[cfg(test)]
pub(crate) fn test_http_table_spec(
    name: &str,
    columns: Vec<ColumnSpec>,
    filters: Vec<FilterSpec>,
    request: RequestSpec,
) -> HttpTableSpec {
    HttpTableSpec {
        common: TableCommon::new(
            name.to_string(),
            "test".to_string(),
            String::new(),
            filters,
            None,
            columns,
        ),
        request,
        requests: vec![],
        response: ResponseSpec::default(),
        pagination: PaginationSpec::default(),
    }
}
