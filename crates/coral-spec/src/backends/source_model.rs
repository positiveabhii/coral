#![allow(
    missing_docs,
    reason = "This module defines field-heavy declarative source-model manifest types."
)]

//! DSL v4 source-model manifest model and validation.
//!
//! Source-model manifests describe which API descriptions should be imported
//! and which SQL projections should later resolve against materialized IR. They
//! do not contain importer-produced entities, operations, or protocol bindings.

use std::collections::{BTreeSet, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::backends::http::{AuthSpec, RateLimitSpec};
use crate::inputs::collect_source_inputs_value;
use crate::validate::validate_template;
use crate::{
    HeaderSpec, ManifestError, ManifestInputKind, ManifestInputSpec, ParsedTemplate, Result,
    SourceBackend, SourceManifestCommon, SourceModelProjection, SourceModelProjectionRef,
    validate_columns, validate_test_queries,
};

/// Validated top-level manifest for a DSL v4 source-model-backed source.
#[derive(Debug, Clone)]
pub struct SourceModelSourceManifest {
    pub common: SourceManifestCommon,
    pub surfaces: Vec<SourceModelManifestSurface>,
    pub projections: Vec<SourceModelProjection>,
    pub declared_inputs: Vec<ManifestInputSpec>,
}

/// One API description surface imported by a DSL v4 manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceModelManifestSurface {
    pub id: String,
    #[serde(rename = "type")]
    pub surface_type: SurfaceDescriptionType,
    pub url: String,
    pub sha256: String,
    pub base_url: ParsedTemplate,
    #[serde(default)]
    pub auth: AuthSpec,
    #[serde(default)]
    pub request_headers: Vec<HeaderSpec>,
    #[serde(default)]
    pub rate_limit: RateLimitSpec,
}

/// Supported author-facing API description formats for DSL v4 surfaces.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceDescriptionType {
    OpenApi,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSourceModelSourceManifest {
    dsl_version: u32,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    test_queries: Vec<String>,
    backend: SourceBackend,
    #[serde(default)]
    inputs: Option<Value>,
    surfaces: Vec<SourceModelManifestSurface>,
    projections: Vec<SourceModelProjection>,
}

impl SourceModelSourceManifest {
    pub(crate) fn parse_manifest_value(value: Value) -> Result<Self> {
        let declared_inputs = collect_source_inputs_value(&value)?;
        let raw: RawSourceModelSourceManifest =
            serde_json::from_value(value).map_err(ManifestError::deserialize)?;
        let RawSourceModelSourceManifest {
            dsl_version,
            name,
            version,
            description,
            test_queries,
            backend: _backend,
            inputs: _inputs,
            surfaces,
            projections,
        } = raw;

        validate_test_queries(&name, &test_queries)?;
        validate_surfaces(&name, &surfaces)?;
        validate_projections(&name, &surfaces, &projections)?;
        let common =
            SourceManifestCommon::new(dsl_version, name, version, description, test_queries);

        Ok(Self {
            common,
            surfaces,
            projections,
            declared_inputs,
        })
    }

    /// Returns the source secrets required by this manifest.
    ///
    /// In the input model, every declared input with `kind: secret` is required
    /// because secrets cannot carry defaults.
    pub fn required_secret_names(&self) -> BTreeSet<String> {
        self.declared_inputs
            .iter()
            .filter(|input| input.kind == ManifestInputKind::Secret)
            .map(|input| input.key.clone())
            .collect()
    }

    /// Returns projection references in the shape needed to validate
    /// materialized source-model IR.
    pub fn projection_refs(&self) -> Vec<SourceModelProjectionRef> {
        self.projections
            .iter()
            .map(SourceModelProjection::reference)
            .collect()
    }
}

fn validate_surfaces(source_name: &str, surfaces: &[SourceModelManifestSurface]) -> Result<()> {
    if surfaces.is_empty() {
        return Err(ManifestError::validation(format!(
            "source '{source_name}' must define at least one surface"
        )));
    }

    let mut seen = HashSet::new();
    for surface in surfaces {
        validate_non_empty(&surface.id, &format!("source '{source_name}' surface id"))?;
        if !seen.insert(surface.id.as_str()) {
            return Err(ManifestError::validation(format!(
                "source '{source_name}' declares surface '{}' more than once",
                surface.id
            )));
        }
        validate_non_empty(
            &surface.url,
            &format!("source '{source_name}' surface '{}' url", surface.id),
        )?;
        validate_sha256(source_name, surface)?;
        if surface.base_url.raw().trim().is_empty() {
            return Err(ManifestError::validation(format!(
                "source '{source_name}' surface '{}' must define a non-empty base_url",
                surface.id
            )));
        }
        validate_template(
            &surface.base_url,
            &HashSet::new(),
            &format!("source '{source_name}' surface '{}'", surface.id),
        )?;
    }

    Ok(())
}

fn validate_sha256(source_name: &str, surface: &SourceModelManifestSurface) -> Result<()> {
    let sha = surface.sha256.trim();
    if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::validation(format!(
            "source '{source_name}' surface '{}' sha256 must be a 64-character hex string",
            surface.id
        )));
    }
    Ok(())
}

fn validate_projections(
    source_name: &str,
    surfaces: &[SourceModelManifestSurface],
    projections: &[SourceModelProjection],
) -> Result<()> {
    if projections.is_empty() {
        return Err(ManifestError::validation(format!(
            "source '{source_name}' must define at least one projection"
        )));
    }

    let surface_ids = surfaces
        .iter()
        .map(|surface| surface.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    for projection in projections {
        validate_non_empty(
            &projection.name,
            &format!("source '{source_name}' projection name"),
        )?;
        if !seen.insert(projection.name.as_str()) {
            return Err(ManifestError::validation(format!(
                "source '{source_name}' declares projection '{}' more than once",
                projection.name
            )));
        }
        if !surface_ids.contains(projection.operation.surface.as_str()) {
            return Err(ManifestError::validation(format!(
                "source '{source_name}' projection '{}' references unknown surface '{}'",
                projection.name, projection.operation.surface
            )));
        }
        validate_non_empty(
            &projection.operation.operation,
            &format!(
                "source '{source_name}' projection '{}' operation",
                projection.name
            ),
        )?;
        validate_columns(&projection.columns, source_name, &projection.name)?;
    }

    Ok(())
}

fn validate_non_empty(value: &str, context: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(ManifestError::validation(format!(
            "{context} must not be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::ProjectionKind;
    use crate::backends::source_model::{SourceModelSourceManifest, SurfaceDescriptionType};

    fn source_model_manifest() -> Value {
        serde_yaml::from_str(
            r"
name: github
version: 1.0.0
dsl_version: 4
backend: source_model
description: GitHub via imported OpenAPI
inputs:
  GITHUB_TOKEN:
    kind: secret
surfaces:
  - id: github-rest
    type: open-api
    url: https://example.com/github-openapi.yaml
    sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    base_url: https://api.github.com
    auth:
      type: HeaderAuth
      headers:
        - name: Authorization
          from: template
          template: Bearer {{input.GITHUB_TOKEN}}
projections:
  - name: issues
    kind: table
    surface: github-rest
    operation: issues/list-for-repo
    columns:
      - name: title
        type: Utf8
",
        )
        .expect("test manifest should parse as yaml")
    }

    #[test]
    fn source_model_manifest_parses_surface_and_projection_metadata() {
        let manifest = SourceModelSourceManifest::parse_manifest_value(source_model_manifest())
            .expect("source model manifest should parse");

        assert_eq!(manifest.common.name, "github");
        assert_eq!(manifest.surfaces.len(), 1);
        let surface = manifest
            .surfaces
            .first()
            .expect("manifest should have a surface");
        assert_eq!(surface.id, "github-rest");
        assert_eq!(surface.surface_type, SurfaceDescriptionType::OpenApi);
        assert_eq!(manifest.projections.len(), 1);
        let projection = manifest
            .projections
            .first()
            .expect("manifest should have a parsed projection");
        assert_eq!(projection.name, "issues");
        assert_eq!(projection.kind, ProjectionKind::Table);
        assert_eq!(projection.operation.surface, "github-rest");
        assert_eq!(projection.operation.operation, "issues/list-for-repo");
        assert_eq!(projection.columns.len(), 1);
        let projection_refs = manifest.projection_refs();
        let projection_ref = projection_refs
            .first()
            .expect("manifest should have a projection ref");
        assert_eq!(projection_ref.operation.surface, "github-rest");
        assert_eq!(projection_ref.operation.operation, "issues/list-for-repo");
        assert_eq!(
            manifest
                .required_secret_names()
                .into_iter()
                .collect::<Vec<_>>(),
            ["GITHUB_TOKEN"]
        );
    }

    #[test]
    fn source_model_manifest_rejects_unknown_projection_surface() {
        let manifest = serde_yaml::from_str(
            r"
name: github
version: 1.0.0
dsl_version: 4
backend: source_model
surfaces:
  - id: github-rest
    type: open-api
    url: https://example.com/github-openapi.yaml
    sha256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    base_url: https://api.github.com
projections:
  - name: issues
    kind: table
    surface: missing
    operation: issues/list-for-repo
",
        )
        .expect("test manifest should parse as yaml");

        let error = SourceModelSourceManifest::parse_manifest_value(manifest)
            .expect_err("unknown surface should fail");

        assert_eq!(
            error.to_string(),
            "source 'github' projection 'issues' references unknown surface 'missing'"
        );
    }
}
