//! Owns the source lifecycle workflow for the local app.

use std::collections::{BTreeMap, BTreeSet};

use crate::bootstrap::AppError;
use crate::sources::SourceName;
use crate::sources::catalog::{
    describe_manifest, list_bundled_sources, load_bundled_source, resolve_installed_manifest,
};
use crate::sources::materialization::{
    materialize_source_model_manifest, refresh_source_model_manifest,
};
use crate::sources::model::{CandidateSource, InstalledSource, SourceOrigin};
use crate::state::{AppStateLayout, ConfigStore, SecretStore};
use crate::storage::fs;
use crate::workspaces::WorkspaceName;
use coral_spec::{ManifestInputKind, ValidatedSourceManifest, parse_source_manifest_yaml};
use tracing::warn;

#[derive(Clone)]
pub(crate) struct SourceManager {
    config_store: ConfigStore,
    secret_store: SecretStore,
    layout: AppStateLayout,
}

pub(crate) struct CreateBundledSourceCommand {
    pub(crate) name: SourceName,
    pub(crate) bindings: SourceBindings,
}

pub(crate) struct ImportSourceCommand {
    pub(crate) manifest_yaml: String,
    pub(crate) bindings: SourceBindings,
}

#[derive(Default)]
pub(crate) struct SourceBindings {
    pub(crate) variables: Vec<SourceBinding>,
    pub(crate) secrets: Vec<SourceBinding>,
}

pub(crate) struct SourceBinding {
    pub(crate) key: String,
    pub(crate) value: String,
}

struct ValidatedBindings {
    variables: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
}

struct PersistSourceRequest<'a> {
    candidate: &'a CandidateSource,
    source_spec: &'a ValidatedSourceManifest,
    manifest_yaml: Option<&'a str>,
    bindings: ValidatedBindings,
    origin: SourceOrigin,
}

struct SourceRollbackState {
    source: InstalledSource,
    manifest_yaml: Option<String>,
    secrets: BTreeMap<String, String>,
}

impl SourceManager {
    pub(crate) fn new(
        config_store: ConfigStore,
        secret_store: SecretStore,
        layout: AppStateLayout,
    ) -> Self {
        Self {
            config_store,
            secret_store,
            layout,
        }
    }

    pub(crate) fn list_workspace_sources(
        &self,
        workspace_name: &WorkspaceName,
    ) -> Result<Vec<InstalledSource>, AppError> {
        Ok(self
            .config_store
            .list_workspace_sources(workspace_name)?
            .into_iter()
            .map(|source| self.populate_source_version_or_keep(workspace_name, source))
            .collect())
    }

    pub(crate) fn get_source(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<InstalledSource, AppError> {
        Ok(self.populate_source_version_or_keep(
            workspace_name,
            self.config_store.get_source(workspace_name, source_name)?,
        ))
    }

    pub(crate) fn get_source_info(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<CandidateSource, AppError> {
        match self.config_store.get_source(workspace_name, source_name) {
            Ok(source) => {
                return Ok(
                    resolve_installed_manifest(workspace_name, &source, &self.layout)?.candidate,
                );
            }
            Err(AppError::SourceNotFound(_)) => {}
            Err(error) => return Err(error),
        }

        match load_bundled_source(source_name) {
            Ok(bundled) => self.describe_bundled_source(workspace_name, &bundled.manifest_yaml),
            Err(AppError::InvalidInput(_)) => {
                Err(AppError::SourceNotFound(source_name.to_string()))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn discover_sources(
        &self,
        workspace_name: &WorkspaceName,
    ) -> Result<Vec<CandidateSource>, AppError> {
        let installed = self
            .config_store
            .list_workspace_sources(workspace_name)?
            .into_iter()
            .map(|source| source.name)
            .collect::<BTreeSet<_>>();
        list_bundled_sources(&installed)
    }

    pub(crate) async fn create_bundled_source(
        &self,
        workspace_name: &WorkspaceName,
        command: &CreateBundledSourceCommand,
    ) -> Result<InstalledSource, AppError> {
        let bundled = load_bundled_source(&command.name)?;
        let source_spec = parse_source_manifest_yaml(&bundled.manifest_yaml)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let candidate = self.describe_bundled_source(workspace_name, &bundled.manifest_yaml)?;
        let bindings = validate_bindings(&candidate, &command.bindings)?;
        self.persist_source(
            workspace_name,
            PersistSourceRequest {
                candidate: &candidate,
                source_spec: &source_spec,
                manifest_yaml: None,
                bindings,
                origin: SourceOrigin::Bundled,
            },
        )
        .await
    }

    pub(crate) async fn import_source(
        &self,
        workspace_name: &WorkspaceName,
        command: &ImportSourceCommand,
    ) -> Result<InstalledSource, AppError> {
        let source_spec = parse_source_manifest_yaml(&command.manifest_yaml)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let mut candidate =
            describe_manifest(&command.manifest_yaml, SourceOrigin::Imported, false)?;
        candidate.installed = self.source_exists(workspace_name, &candidate.name)?;
        let bindings = validate_bindings(&candidate, &command.bindings)?;
        self.persist_source(
            workspace_name,
            PersistSourceRequest {
                candidate: &candidate,
                source_spec: &source_spec,
                manifest_yaml: Some(&command.manifest_yaml),
                bindings,
                origin: SourceOrigin::Imported,
            },
        )
        .await
    }

    pub(crate) async fn refresh_source(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<InstalledSource, AppError> {
        let stored = self.config_store.get_source(workspace_name, source_name)?;
        let installed = resolve_installed_manifest(workspace_name, &stored, &self.layout)?;
        let Some(source_model) = installed.source_spec.as_source_model() else {
            return Err(AppError::InvalidInput(format!(
                "source '{source_name}' is not a source-model source"
            )));
        };
        refresh_source_model_manifest(&self.layout, workspace_name, source_name, source_model)
            .await?;
        self.populate_source_version(workspace_name, stored)
    }

    pub(crate) fn delete_source(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<InstalledSource, AppError> {
        let stored = self.config_store.get_source(workspace_name, source_name)?;
        let removed = self.populate_source_version_or_keep(workspace_name, stored.clone());
        let source_dir = self.layout.source_dir(workspace_name, source_name);
        let previous = SourceRollbackState {
            source: stored,
            manifest_yaml: match removed.origin {
                SourceOrigin::Bundled => None,
                SourceOrigin::Imported => Some(std::fs::read_to_string(
                    self.layout.manifest_file(workspace_name, source_name),
                )?),
            },
            secrets: self
                .secret_store
                .read_source_secrets_for(workspace_name, source_name)?,
        };
        if source_dir.exists()
            && let Err(error) = std::fs::remove_dir_all(&source_dir)
        {
            self.restore_source_rollback_state(workspace_name, source_name, Some(previous));
            return Err(error.into());
        }
        if let Err(error) = self.config_store.remove_source(workspace_name, source_name) {
            self.restore_source_rollback_state(workspace_name, source_name, Some(previous));
            return Err(error);
        }
        cleanup_empty_parent(&self.layout.workspaces_root(), source_dir.parent());
        cleanup_empty_parent(
            &self.layout.workspaces_root(),
            self.layout.workspace_dir(workspace_name).parent(),
        );
        Ok(removed)
    }

    fn describe_bundled_source(
        &self,
        workspace_name: &WorkspaceName,
        manifest_yaml: &str,
    ) -> Result<CandidateSource, AppError> {
        let mut candidate = describe_manifest(manifest_yaml, SourceOrigin::Bundled, false)?;
        candidate.installed = self.source_exists(workspace_name, &candidate.name)?;
        Ok(candidate)
    }

    async fn persist_source(
        &self,
        workspace_name: &WorkspaceName,
        request: PersistSourceRequest<'_>,
    ) -> Result<InstalledSource, AppError> {
        let source_name = request.candidate.name.clone();
        let previous = self.load_source_rollback_state(workspace_name, &source_name)?;
        if let Err(error) =
            self.persist_manifest_artifact(workspace_name, &source_name, request.manifest_yaml)
        {
            self.restore_source_rollback_state(workspace_name, &source_name, previous);
            return Err(error);
        }

        if let Some(source_model) = request.source_spec.as_source_model()
            && let Err(error) = materialize_source_model_manifest(
                &self.layout,
                workspace_name,
                &source_name,
                source_model,
            )
            .await
        {
            self.restore_source_rollback_state(workspace_name, &source_name, previous);
            return Err(error);
        }

        let persisted_secrets = match self.secret_store.replace_source_secrets_for(
            workspace_name,
            &source_name,
            &request.bindings.secrets,
        ) {
            Ok(secrets) => secrets,
            Err(error) => {
                self.restore_source_rollback_state(workspace_name, &source_name, previous);
                return Err(error);
            }
        };

        let persisted_version = match request.origin {
            SourceOrigin::Bundled => None,
            SourceOrigin::Imported => Some(request.candidate.version.clone()),
        };
        let stored = InstalledSource {
            name: source_name.clone(),
            version: persisted_version,
            variables: request.bindings.variables,
            secrets: persisted_secrets,
            origin: request.origin,
        };
        if let Err(error) = self
            .config_store
            .upsert_source(workspace_name, stored.clone())
        {
            self.restore_source_rollback_state(workspace_name, &source_name, previous);
            return Err(error);
        }
        let mut resolved = stored;
        resolved.version = Some(request.candidate.version.clone());
        Ok(resolved)
    }

    fn source_exists(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<bool, AppError> {
        Ok(self
            .config_store
            .load_catalog()?
            .contains(workspace_name, source_name))
    }

    fn load_source_rollback_state(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
    ) -> Result<Option<SourceRollbackState>, AppError> {
        let source = match self.config_store.get_source(workspace_name, source_name) {
            Ok(source) => source,
            Err(AppError::SourceNotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let secrets = self
            .secret_store
            .read_source_secrets_for(workspace_name, source_name)?;
        Ok(Some(SourceRollbackState {
            manifest_yaml: match source.origin {
                SourceOrigin::Bundled => None,
                SourceOrigin::Imported => Some(std::fs::read_to_string(
                    self.layout.manifest_file(workspace_name, source_name),
                )?),
            },
            source,
            secrets,
        }))
    }

    fn restore_source_rollback_state(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
        previous: Option<SourceRollbackState>,
    ) {
        if let Some(previous) = previous {
            let manifest_path = self.layout.manifest_file(workspace_name, source_name);
            match previous.manifest_yaml {
                Some(manifest_yaml) => {
                    if let Some(parent) = manifest_path.parent()
                        && let Err(e) = fs::ensure_dir(parent)
                    {
                        warn!("rollback: failed to create manifest parent dir: {e}");
                    }
                    if let Err(e) = fs::write_atomic(&manifest_path, manifest_yaml.as_bytes()) {
                        warn!("rollback: failed to restore manifest file: {e}");
                    }
                }
                None if manifest_path.exists() => {
                    if let Err(e) = std::fs::remove_file(&manifest_path) {
                        warn!("rollback: failed to remove manifest file: {e}");
                    }
                }
                None => {}
            }
            if let Err(e) = self.secret_store.replace_source_secrets_for(
                workspace_name,
                source_name,
                &previous.secrets,
            ) {
                warn!("rollback: failed to restore source secrets: {e}");
            }
            if let Err(e) = self
                .config_store
                .upsert_source(workspace_name, previous.source)
            {
                warn!("rollback: failed to restore source config: {e}");
            }
        } else {
            let source_dir = self.layout.source_dir(workspace_name, source_name);
            if source_dir.exists()
                && let Err(e) = std::fs::remove_dir_all(&source_dir)
            {
                warn!("rollback: failed to remove source directory: {e}");
            }
        }
    }

    fn persist_manifest_artifact(
        &self,
        workspace_name: &WorkspaceName,
        source_name: &SourceName,
        manifest_yaml: Option<&str>,
    ) -> Result<(), AppError> {
        let manifest_path = self.layout.manifest_file(workspace_name, source_name);
        match manifest_yaml {
            Some(manifest_yaml) => {
                if let Some(parent) = manifest_path.parent() {
                    fs::ensure_dir(parent)?;
                }
                fs::write_atomic(&manifest_path, manifest_yaml.as_bytes())?;
            }
            None if manifest_path.exists() => {
                std::fs::remove_file(&manifest_path)?;
            }
            None => {}
        }
        cleanup_empty_parent(&self.layout.workspaces_root(), manifest_path.parent());
        Ok(())
    }

    fn populate_source_version(
        &self,
        workspace_name: &WorkspaceName,
        mut source: InstalledSource,
    ) -> Result<InstalledSource, AppError> {
        source.version = Some(
            resolve_installed_manifest(workspace_name, &source, &self.layout)?
                .candidate
                .version,
        );
        Ok(source)
    }

    fn populate_source_version_or_keep(
        &self,
        workspace_name: &WorkspaceName,
        source: InstalledSource,
    ) -> InstalledSource {
        self.populate_source_version(workspace_name, source.clone())
            .unwrap_or(source)
    }
}

fn validate_bindings(
    candidate: &CandidateSource,
    bindings: &SourceBindings,
) -> Result<ValidatedBindings, AppError> {
    let mut variable_values = collect_unique_variables(&bindings.variables)?;
    let secret_values = collect_unique_secrets(&bindings.secrets)?;
    let expected_variables = candidate
        .inputs
        .iter()
        .filter(|input| input.kind == ManifestInputKind::Variable)
        .map(|input| input.key.clone())
        .collect::<BTreeSet<_>>();
    let expected_secrets = candidate
        .inputs
        .iter()
        .filter(|input| input.kind == ManifestInputKind::Secret)
        .map(|input| input.key.clone())
        .collect::<BTreeSet<_>>();

    for key in variable_values.keys() {
        if !expected_variables.contains(key) {
            return Err(AppError::InvalidInput(format!(
                "unknown source variable '{key}'"
            )));
        }
    }
    for key in secret_values.keys() {
        if !expected_secrets.contains(key) {
            return Err(AppError::InvalidInput(format!(
                "unknown source secret '{key}'"
            )));
        }
    }

    for input in &candidate.inputs {
        if input.kind == ManifestInputKind::Variable
            && !variable_values.contains_key(&input.key)
            && !input.default_value.is_empty()
        {
            variable_values.insert(input.key.clone(), input.default_value.clone());
        }
    }

    for input in &candidate.inputs {
        match input.kind {
            ManifestInputKind::Variable
                if input.required && !variable_values.contains_key(&input.key) =>
            {
                return Err(AppError::InvalidInput(format!(
                    "missing required source variable '{}'",
                    input.key
                )));
            }
            ManifestInputKind::Secret
                if input.required && !secret_values.contains_key(&input.key) =>
            {
                return Err(AppError::InvalidInput(format!(
                    "missing required source secret '{}'",
                    input.key
                )));
            }
            _ => {}
        }
    }

    Ok(ValidatedBindings {
        variables: variable_values,
        secrets: secret_values,
    })
}

fn collect_unique_variables(
    variables: &[SourceBinding],
) -> Result<BTreeMap<String, String>, AppError> {
    let mut values = BTreeMap::new();
    for variable in variables {
        let key = normalize_binding_key("source variable key", &variable.key)?;
        if values.insert(key.clone(), variable.value.clone()).is_some() {
            return Err(AppError::InvalidInput(format!(
                "source variable '{key}' is repeated"
            )));
        }
    }
    Ok(values)
}

fn collect_unique_secrets(secrets: &[SourceBinding]) -> Result<BTreeMap<String, String>, AppError> {
    let mut values = BTreeMap::new();
    for secret in secrets {
        let key = normalize_binding_key("source secret key", &secret.key)?;
        if values.insert(key.clone(), secret.value.clone()).is_some() {
            return Err(AppError::InvalidInput(format!(
                "source secret '{key}' is repeated"
            )));
        }
    }
    Ok(values)
}

fn normalize_binding_key(label: &str, value: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(format!("missing {label}")));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(AppError::InvalidInput(format!(
            "{label} must not contain '/' or '\\\\'"
        )));
    }
    if trimmed.contains('=') || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(AppError::InvalidInput(format!(
            "{label} must not contain '=', '\\n', or '\\r'"
        )));
    }
    if trimmed.starts_with('#') {
        return Err(AppError::InvalidInput(format!(
            "{label} must not start with '#'"
        )));
    }
    Ok(trimmed.to_string())
}

fn cleanup_empty_parent(root: &std::path::Path, path: Option<&std::path::Path>) {
    let Some(mut current) = path.map(std::path::Path::to_path_buf) else {
        return;
    };
    while current.starts_with(root) && current != root {
        let Ok(mut entries) = std::fs::read_dir(&current) else {
            break;
        };
        if entries.next().is_some() {
            break;
        }
        let next = current.parent().unwrap_or(root).to_path_buf();
        if std::fs::remove_dir(&current).is_err() {
            break;
        }
        current = next;
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use url::Url;

    use coral_spec::parse_source_manifest_yaml;

    use super::{
        ImportSourceCommand, SourceBinding, SourceBindings, SourceManager, normalize_binding_key,
    };
    use crate::sources::SourceName;
    use crate::sources::materialization::load_materialized_source_model_manifest;
    use crate::sources::materialization::test_support::document_sha256;
    use crate::state::{AppStateLayout, ConfigStore, SecretStore};
    use crate::workspaces::WorkspaceName;

    fn default_workspace() -> WorkspaceName {
        WorkspaceName::default()
    }

    fn manifest_with_secret() -> String {
        r#"
name: secured_messages
version: 0.1.0
dsl_version: 3
backend: http
inputs:
  API_BASE:
    kind: variable
    default: https://example.com
  API_TOKEN:
    kind: secret
base_url: "{{input.API_BASE}}"
auth:
  type: HeaderAuth
  headers:
    - name: Authorization
      from: template
      template: Bearer {{input.API_TOKEN}}
tables:
  - name: messages
    description: Secured messages
    request:
      method: GET
      path: /messages
    response: {}
    columns:
      - name: id
        type: Utf8
"#
        .to_string()
    }

    fn source_model_openapi_document() -> &'static str {
        r"
openapi: 3.0.3
info:
  title: Test API
  version: 1.0.0
paths:
  /repos/{owner}/{repo}/issues:
    get:
      operationId: issues/list-for-repo
      parameters:
        - name: owner
          in: path
          required: true
          schema: { type: string }
        - name: repo
          in: path
          required: true
          schema: { type: string }
      responses:
        '200':
          description: OK
          content:
            application/json:
              schema:
                type: array
                items:
                  $ref: '#/components/schemas/Issue'
components:
  schemas:
    Issue:
      type: object
      required: [id, title]
      properties:
        id: { type: integer }
        title: { type: string }
"
    }

    fn source_model_manifest(surface_url: &str, sha256: &str) -> String {
        format!(
            r"
name: github
version: 1.0.0
dsl_version: 4
backend: source_model
surfaces:
  - id: github-rest
    type: open-api
    url: {surface_url}
    sha256: {sha256}
    base_url: https://api.github.com
projections:
  - name: issues
    kind: table
    surface: github-rest
    operation: issues/list-for-repo
    columns:
      - name: title
        type: Utf8
"
        )
    }

    #[tokio::test]
    async fn import_restores_prior_state_when_secret_persistence_fails() {
        let temp = TempDir::new().expect("temp dir");
        let layout =
            AppStateLayout::discover(Some(temp.path().join("coral-config"))).expect("layout");
        layout.ensure().expect("ensure layout");
        let manager = SourceManager::new(
            ConfigStore::new(layout.clone()),
            SecretStore::new(layout.clone()),
            layout.clone(),
        );

        let source_name = SourceName::parse("secured_messages").expect("source");
        let source_dir = layout.source_dir(&default_workspace(), &source_name);
        std::fs::create_dir_all(&source_dir).expect("create source dir");
        std::fs::create_dir(source_dir.join("secrets.env"))
            .expect("create blocking secrets directory");

        let error = manager
            .import_source(
                &default_workspace(),
                &ImportSourceCommand {
                    manifest_yaml: manifest_with_secret(),
                    bindings: SourceBindings {
                        variables: vec![SourceBinding {
                            key: "API_BASE".to_string(),
                            value: "https://example.com".to_string(),
                        }],
                        secrets: vec![SourceBinding {
                            key: "API_TOKEN".to_string(),
                            value: "secret-token".to_string(),
                        }],
                    },
                },
            )
            .await
            .expect_err("secret persistence should fail");

        assert!(
            matches!(
                error,
                crate::bootstrap::AppError::Credentials(crate::state::CredentialsError::Io(_))
            ),
            "unexpected error: {error:#}"
        );
        assert!(
            !layout
                .source_dir(&default_workspace(), &source_name)
                .exists(),
            "source dir should be cleaned up after secret persistence failure"
        );
        assert!(
            manager
                .list_workspace_sources(&default_workspace())
                .expect("list sources")
                .is_empty(),
            "source config should not be persisted after rollback"
        );
    }

    #[test]
    fn logical_binding_keys_allow_dot_segments() {
        assert_eq!(
            normalize_binding_key("source variable key", "..").expect("key"),
            ".."
        );
    }

    #[test]
    fn rejects_env_file_breaking_binding_keys() {
        let error = normalize_binding_key("source secret key", "API=TOKEN")
            .expect_err("'=' should be rejected");
        assert!(
            error
                .to_string()
                .contains("must not contain '=', '\\n', or '\\r'")
        );

        let error = normalize_binding_key("source secret key", "API\nTOKEN")
            .expect_err("newlines should be rejected");
        assert!(
            error
                .to_string()
                .contains("must not contain '=', '\\n', or '\\r'")
        );

        let error = normalize_binding_key("source secret key", " #comment")
            .expect_err("leading comment markers should be rejected");
        assert!(error.to_string().contains("must not start with '#'"));
    }

    #[tokio::test]
    async fn import_materializes_variable_defaults_server_side() {
        let temp = TempDir::new().expect("temp dir");
        let layout =
            AppStateLayout::discover(Some(temp.path().join("coral-config"))).expect("layout");
        layout.ensure().expect("ensure layout");
        let manager = SourceManager::new(
            ConfigStore::new(layout.clone()),
            SecretStore::new(layout.clone()),
            layout,
        );

        let source = manager
            .import_source(
                &default_workspace(),
                &ImportSourceCommand {
                    manifest_yaml: manifest_with_secret(),
                    bindings: SourceBindings {
                        variables: vec![],
                        secrets: vec![SourceBinding {
                            key: "API_TOKEN".to_string(),
                            value: "secret-token".to_string(),
                        }],
                    },
                },
            )
            .await
            .expect("import source");

        assert_eq!(
            source.variables.get("API_BASE").map(String::as_str),
            Some("https://example.com")
        );
    }

    #[tokio::test]
    async fn import_materializes_source_model_ir_from_pinned_file_url() {
        let temp = TempDir::new().expect("temp dir");
        let layout =
            AppStateLayout::discover(Some(temp.path().join("coral-config"))).expect("layout");
        layout.ensure().expect("ensure layout");
        let document_path = temp.path().join("openapi.yaml");
        std::fs::write(&document_path, source_model_openapi_document()).expect("write openapi");
        let surface_url = Url::from_file_path(&document_path)
            .expect("file url")
            .to_string();
        let manifest = source_model_manifest(
            &surface_url,
            &document_sha256(source_model_openapi_document()),
        );
        let manager = SourceManager::new(
            ConfigStore::new(layout.clone()),
            SecretStore::new(layout.clone()),
            layout.clone(),
        );

        manager
            .import_source(
                &default_workspace(),
                &ImportSourceCommand {
                    manifest_yaml: manifest.clone(),
                    bindings: SourceBindings::default(),
                },
            )
            .await
            .expect("import source-model source");

        let parsed = parse_source_manifest_yaml(&manifest).expect("parse manifest");
        let source_model = parsed.as_source_model().expect("source-model manifest");
        std::fs::remove_file(&document_path).expect("remove source OpenAPI document");
        let materialized = load_materialized_source_model_manifest(
            &layout,
            &default_workspace(),
            &SourceName::parse("github").expect("source"),
            source_model,
        )
        .expect("load materialized IR");
        assert!(
            materialized
                .operations
                .iter()
                .any(|operation| operation.id == "issues/list-for-repo")
        );
    }

    #[tokio::test]
    async fn refresh_rejects_changed_document_hash_and_reports_actual_hash() {
        let temp = TempDir::new().expect("temp dir");
        let layout =
            AppStateLayout::discover(Some(temp.path().join("coral-config"))).expect("layout");
        layout.ensure().expect("ensure layout");
        let document_path = temp.path().join("openapi.yaml");
        std::fs::write(&document_path, source_model_openapi_document()).expect("write openapi");
        let surface_url = Url::from_file_path(&document_path)
            .expect("file url")
            .to_string();
        let pinned_hash = document_sha256(source_model_openapi_document());
        let manifest = source_model_manifest(&surface_url, &pinned_hash);
        let manager = SourceManager::new(
            ConfigStore::new(layout.clone()),
            SecretStore::new(layout.clone()),
            layout,
        );
        let source_name = SourceName::parse("github").expect("source");
        manager
            .import_source(
                &default_workspace(),
                &ImportSourceCommand {
                    manifest_yaml: manifest,
                    bindings: SourceBindings::default(),
                },
            )
            .await
            .expect("import source-model source");

        let changed_document = source_model_openapi_document().replace("Test API", "Changed API");
        std::fs::write(&document_path, &changed_document).expect("rewrite openapi");
        let changed_hash = document_sha256(&changed_document);

        let error = manager
            .refresh_source(&default_workspace(), &source_name)
            .await
            .expect_err("refresh should reject changed hash");

        let message = error.to_string();
        assert!(
            message.contains(&pinned_hash),
            "missing pinned hash: {message}"
        );
        assert!(
            message.contains(&changed_hash),
            "missing actual hash: {message}"
        );
    }
}
