//! Extracts interactive source inputs from source-spec documents.
//!
//! Sources that need interactive configuration declare their inputs under a
//! top-level `inputs` map. Each entry fixes the input's kind (`variable` or
//! `secret`), an optional default, and an optional hint. References elsewhere
//! in the manifest use `{{input.KEY}}` templates or `from: input` value
//! sources; the declared kind determines whether the value is resolved from
//! the variable or secret store. Manifests that take no interactive inputs
//! may omit the block entirely.

use std::collections::BTreeSet;

use crate::proto::v1 as specv1;
use crate::{ManifestError, ParsedTemplate, Result, TemplateNamespace};

/// The kind of interactive input required by one validated source spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestInputKind {
    /// A non-secret input persisted in source variables.
    Variable,
    /// A secret input persisted separately from source variables.
    Secret,
}

/// One interactive input extracted from a validated source spec.
///
/// The app and CLI can map this into prompts, persisted variables, or secret
/// collection flows without depending on protobuf-specific types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInputSpec {
    /// The source-spec-declared input key.
    pub key: String,
    /// Whether this input is a variable or a secret.
    pub kind: ManifestInputKind,
    /// Whether the user must provide an explicit value.
    pub required: bool,
    /// The source-spec-declared default value, if any.
    pub default_value: String,
    /// Optional authored hint shown to the user when collecting the input.
    pub hint: Option<String>,
}

/// Collect interactive source inputs from a generated source manifest proto.
///
/// # Errors
///
/// Returns a [`ManifestError`] when an input is declared incorrectly or the
/// manifest references an input that is not declared under the top-level
/// `inputs` block.
pub(crate) fn collect_source_inputs_proto(
    manifest: &specv1::SourceManifest,
) -> Result<Vec<ManifestInputSpec>> {
    let inputs = collect_declared_inputs_proto(&manifest.inputs)?;
    validate_input_references_proto(manifest, &inputs)?;
    Ok(inputs)
}

fn collect_declared_inputs_proto(
    inputs: &[specv1::SourceInputBinding],
) -> Result<Vec<ManifestInputSpec>> {
    let mut ordered = Vec::with_capacity(inputs.len());
    for input in inputs {
        let spec = input.input.as_ref().ok_or_else(|| {
            ManifestError::validation(format!(
                "manifest input '{}' must include an input spec",
                input.key
            ))
        })?;
        let kind = match specv1::SourceInputKind::try_from(spec.kind).map_err(|_| {
            ManifestError::validation(format!(
                "manifest input '{}' has unsupported kind enum value {}",
                input.key, spec.kind
            ))
        })? {
            specv1::SourceInputKind::Variable => ManifestInputKind::Variable,
            specv1::SourceInputKind::Secret => ManifestInputKind::Secret,
            specv1::SourceInputKind::Unspecified => {
                return Err(ManifestError::validation(format!(
                    "manifest input '{}' is missing kind",
                    input.key
                )));
            }
        };
        if kind == ManifestInputKind::Secret && spec.default_value.is_some() {
            return Err(ManifestError::validation(format!(
                "manifest secret input '{}' must not declare a default",
                input.key
            )));
        }
        let default_value = spec.default_value.clone();
        ordered.push(ManifestInputSpec {
            key: input.key.clone(),
            kind,
            required: default_value.is_none(),
            default_value: default_value.unwrap_or_default(),
            hint: if spec.hint.is_empty() {
                None
            } else {
                Some(spec.hint.clone())
            },
        });
    }
    Ok(ordered)
}

fn validate_input_references_proto(
    manifest: &specv1::SourceManifest,
    inputs: &[ManifestInputSpec],
) -> Result<()> {
    let declared: BTreeSet<String> = inputs.iter().map(|input| input.key.clone()).collect();
    validate_template(&manifest.base_url, &declared)?;
    if let Some(auth) = &manifest.auth {
        validate_headers_proto(&auth.headers, &declared)?;
    }
    for table in &manifest.tables {
        validate_request_proto(table.request.as_ref(), &declared)?;
        for route in &table.requests {
            validate_request_proto(route.request.as_ref(), &declared)?;
        }
    }
    Ok(())
}

fn validate_headers_proto(
    headers: &[specv1::HeaderSpec],
    declared: &BTreeSet<String>,
) -> Result<()> {
    for header in headers {
        validate_value_source_proto(header.value.as_ref(), declared)?;
    }
    Ok(())
}

fn validate_request_proto(
    request: Option<&specv1::RequestSpec>,
    declared: &BTreeSet<String>,
) -> Result<()> {
    let Some(request) = request else {
        return Ok(());
    };
    validate_template(&request.path, declared)?;
    validate_headers_proto(&request.headers, declared)?;
    for param in &request.query {
        validate_value_source_proto(param.value.as_ref(), declared)?;
    }
    for field in &request.body {
        validate_value_source_proto(field.value.as_ref(), declared)?;
    }
    Ok(())
}

fn validate_value_source_proto(
    source: Option<&specv1::ValueSource>,
    declared: &BTreeSet<String>,
) -> Result<()> {
    let Some(kind) = source.and_then(|source| source.kind.as_ref()) else {
        return Ok(());
    };
    match kind {
        specv1::value_source::Kind::Template(value) => validate_template(&value.template, declared),
        specv1::value_source::Kind::Input(value) => validate_input_key(&value.key, declared),
        specv1::value_source::Kind::Literal(_)
        | specv1::value_source::Kind::Filter(_)
        | specv1::value_source::Kind::FilterInt(_)
        | specv1::value_source::Kind::State(_)
        | specv1::value_source::Kind::NowEpochMinusSeconds(_) => Ok(()),
    }
}

fn validate_input_key(key: &str, declared: &BTreeSet<String>) -> Result<()> {
    if !declared.contains(key) {
        return Err(ManifestError::validation(format!(
            "manifest input '{key}' is referenced but not declared under top-level inputs"
        )));
    }
    Ok(())
}

fn validate_template(template: &str, declared: &BTreeSet<String>) -> Result<()> {
    let template = ParsedTemplate::parse(template)?;
    for token in template.tokens() {
        if !matches!(token.namespace(), TemplateNamespace::Input) {
            continue;
        }
        validate_input_key(token.key(), declared)?;
        if token.default_value().is_some() {
            return Err(ManifestError::validation(format!(
                "manifest input '{}' must declare defaults under top-level inputs",
                token.key()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ManifestInputKind, ManifestInputSpec, collect_source_inputs_proto};
    use crate::Result;
    use crate::proto_source::source_manifest_proto_from_yaml;

    fn collect(raw: &str) -> Result<Vec<ManifestInputSpec>> {
        let manifest = source_manifest_proto_from_yaml(raw)?;
        collect_source_inputs_proto(&manifest)
    }

    #[test]
    fn declared_inputs_are_parsed_in_manifest_order() {
        let manifest = r#"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
inputs:
  GITHUB_API_BASE:
    kind: variable
    default: https://api.github.com
    hint: For GitHub Enterprise, use https://<host>/api/v3
  GITHUB_TOKEN:
    kind: secret
    hint: Run `gh auth token` or create a PAT
base_url: "{{input.GITHUB_API_BASE}}"
auth:
  headers:
    - name: Authorization
      from: template
      template: Bearer {{input.GITHUB_TOKEN}}
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
"#;

        let inputs = collect(manifest).expect("inputs");
        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].key, "GITHUB_API_BASE");
        assert_eq!(inputs[0].kind, ManifestInputKind::Variable);
        assert!(!inputs[0].required);
        assert_eq!(inputs[0].default_value, "https://api.github.com");
        assert_eq!(
            inputs[0].hint.as_deref(),
            Some("For GitHub Enterprise, use https://<host>/api/v3")
        );
        assert_eq!(inputs[1].key, "GITHUB_TOKEN");
        assert_eq!(inputs[1].kind, ManifestInputKind::Secret);
        assert!(inputs[1].required);
        assert_eq!(inputs[1].default_value, "");
        assert_eq!(
            inputs[1].hint.as_deref(),
            Some("Run `gh auth token` or create a PAT")
        );
    }

    #[test]
    fn from_input_value_source_resolves_against_declarations() {
        let manifest = r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
inputs:
  GITHUB_TOKEN:
    kind: secret
auth:
  headers:
    - name: Authorization
      from: input
      key: GITHUB_TOKEN
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
";
        let inputs = collect(manifest).expect("inputs");
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].kind, ManifestInputKind::Secret);
    }

    #[test]
    fn manifests_without_inputs_block_are_allowed() {
        let manifest = r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: https://api.github.com
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
";
        let inputs = collect(manifest).expect("no inputs is fine");
        assert!(inputs.is_empty());
    }

    #[test]
    fn references_without_inputs_block_are_rejected() {
        let manifest = r#"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
base_url: "{{input.GITHUB_API_BASE}}"
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
"#;
        let error = collect(manifest).expect_err("undeclared reference");
        assert!(
            error
                .to_string()
                .contains("referenced but not declared under top-level inputs"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn undeclared_reference_is_rejected() {
        let manifest = r#"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
inputs:
  GITHUB_TOKEN:
    kind: secret
base_url: "{{input.GITHUB_API_BASE}}"
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
"#;
        let error = collect(manifest).expect_err("undeclared input");
        assert!(
            error
                .to_string()
                .contains("referenced but not declared under top-level inputs")
        );
    }

    #[test]
    fn inline_template_defaults_are_rejected() {
        let manifest = r#"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
inputs:
  GITHUB_API_BASE:
    kind: variable
    default: https://api.github.com
base_url: "{{input.GITHUB_API_BASE|https://other.example.com}}"
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
"#;
        let error = collect(manifest).expect_err("inline default");
        assert!(
            error
                .to_string()
                .contains("must declare defaults under top-level inputs")
        );
    }

    #[test]
    fn secret_defaults_are_rejected() {
        let manifest = r"
name: demo
version: 1.0.0
dsl_version: 3
backend: http
inputs:
  GITHUB_TOKEN:
    kind: secret
    default: abc123
tables:
  - name: messages
    description: Demo messages
    request:
      path: /messages
";
        let error = collect(manifest).expect_err("secret default");
        assert!(error.to_string().contains("must not define default"));
    }
}
