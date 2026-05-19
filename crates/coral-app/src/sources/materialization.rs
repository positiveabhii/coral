//! Source-model IR materialization for DSL v4 sources.

use std::path::PathBuf;

use chrono::Utc;
use coral_spec::{
    OPENAPI_IMPORTER_VERSION, SourceModelIr, SourceModelSourceManifest, import_openapi_surface,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::bootstrap::AppError;
use crate::sources::SourceName;
use crate::state::AppStateLayout;
use crate::storage::fs;
use crate::workspaces::WorkspaceName;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MaterializedSourceModelMetadata {
    source_name: String,
    source_version: String,
    surface_id: String,
    surface_type: String,
    retrieval_url: String,
    pinned_sha256: String,
    content_sha256: String,
    importer_version: String,
    fetched_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}

struct MaterializedSurface {
    key: String,
    metadata: MaterializedSourceModelMetadata,
    ir: SourceModelIr,
}

struct FetchedSurfaceDocument {
    bytes: Vec<u8>,
    etag: Option<String>,
}

pub(crate) async fn materialize_source_model_manifest(
    layout: &AppStateLayout,
    workspace_name: &WorkspaceName,
    source_name: &SourceName,
    manifest: &SourceModelSourceManifest,
) -> Result<(), AppError> {
    materialize_source_model_manifest_inner(
        layout,
        workspace_name,
        source_name,
        manifest,
        MaterializationMode::Install,
    )
    .await
}

pub(crate) async fn refresh_source_model_manifest(
    layout: &AppStateLayout,
    workspace_name: &WorkspaceName,
    source_name: &SourceName,
    manifest: &SourceModelSourceManifest,
) -> Result<(), AppError> {
    materialize_source_model_manifest_inner(
        layout,
        workspace_name,
        source_name,
        manifest,
        MaterializationMode::Refresh,
    )
    .await
}

pub(crate) fn load_materialized_source_model_manifest(
    layout: &AppStateLayout,
    workspace_name: &WorkspaceName,
    source_name: &SourceName,
    manifest: &SourceModelSourceManifest,
) -> Result<SourceModelIr, AppError> {
    let mut entries = Vec::new();
    for surface in &manifest.surfaces {
        let key = materialization_key(manifest.common.version.as_str(), surface);
        let metadata_path =
            layout.source_model_metadata_file(workspace_name, source_name, key.as_str());
        let metadata: MaterializedSourceModelMetadata =
            serde_json::from_slice(&std::fs::read(&metadata_path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    AppError::FailedPrecondition(format!(
                        "source '{source_name}' surface '{}' has no materialized source-model IR; run `coral source refresh {source_name}` or reinstall the source",
                        surface.id
                    ))
                } else {
                    error.into()
                }
            })?)?;
        validate_metadata(
            source_name,
            manifest.common.version.as_str(),
            surface,
            &metadata,
        )?;
        let ir_path = layout.source_model_ir_file(workspace_name, source_name, key.as_str());
        let ir: SourceModelIr = serde_json::from_slice(&std::fs::read(ir_path)?)?;
        entries.push(ir);
    }

    let ir = combine_surface_irs(entries);
    ir.validate(&manifest.projection_refs())
        .map_err(|error| AppError::InvalidInput(error.to_string()))?;
    Ok(ir)
}

#[derive(Clone, Copy)]
enum MaterializationMode {
    Install,
    Refresh,
}

async fn materialize_source_model_manifest_inner(
    layout: &AppStateLayout,
    workspace_name: &WorkspaceName,
    source_name: &SourceName,
    manifest: &SourceModelSourceManifest,
    mode: MaterializationMode,
) -> Result<(), AppError> {
    let mut materialized = Vec::new();
    for surface in &manifest.surfaces {
        let fetched = fetch_surface_document(surface.url.as_str()).await?;
        let actual_sha256 = sha256_hex(&fetched.bytes);
        if !actual_sha256.eq_ignore_ascii_case(surface.sha256.trim()) {
            let message = match mode {
                MaterializationMode::Install => format!(
                    "source '{source_name}' surface '{}' OpenAPI document sha256 mismatch: expected {}, got {actual_sha256}",
                    surface.id, surface.sha256
                ),
                MaterializationMode::Refresh => format!(
                    "source '{source_name}' surface '{}' OpenAPI document sha256 mismatch during refresh: expected {}, got {actual_sha256}; update the manifest pin before refreshing",
                    surface.id, surface.sha256
                ),
            };
            return Err(AppError::InvalidInput(message));
        }
        let imported = import_openapi_surface(surface, &fetched.bytes)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let metadata = MaterializedSourceModelMetadata {
            source_name: source_name.as_str().to_string(),
            source_version: manifest.common.version.clone(),
            surface_id: surface.id.clone(),
            surface_type: "open-api".to_string(),
            retrieval_url: surface.url.clone(),
            pinned_sha256: surface.sha256.clone(),
            content_sha256: actual_sha256,
            importer_version: OPENAPI_IMPORTER_VERSION.to_string(),
            fetched_at: Utc::now().to_rfc3339(),
            etag: fetched.etag,
        };
        materialized.push(MaterializedSurface {
            key: materialization_key(manifest.common.version.as_str(), surface),
            metadata,
            ir: imported.ir,
        });
    }

    let combined = combine_surface_irs(materialized.iter().map(|entry| entry.ir.clone()));
    combined
        .validate(&manifest.projection_refs())
        .map_err(|error| AppError::InvalidInput(error.to_string()))?;

    for entry in materialized {
        write_materialized_surface(layout, workspace_name, source_name, &entry)?;
    }

    Ok(())
}

async fn fetch_surface_document(url: &str) -> Result<FetchedSurfaceDocument, AppError> {
    if let Ok(parsed) = Url::parse(url) {
        match parsed.scheme() {
            "file" => {
                let path = parsed.to_file_path().map_err(|()| {
                    AppError::InvalidInput(format!("invalid file URL for OpenAPI surface: {url}"))
                })?;
                return Ok(FetchedSurfaceDocument {
                    bytes: std::fs::read(path)?,
                    etag: None,
                });
            }
            "http" | "https" => {
                let response = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .map_err(|error| {
                        AppError::FailedPrecondition(format!(
                            "failed to create OpenAPI fetch client: {error}"
                        ))
                    })?
                    .get(url)
                    .send()
                    .await
                    .map_err(|error| {
                        AppError::FailedPrecondition(format!(
                            "failed to fetch OpenAPI surface '{url}': {error}"
                        ))
                    })?
                    .error_for_status()
                    .map_err(|error| {
                        AppError::FailedPrecondition(format!(
                            "failed to fetch OpenAPI surface '{url}': {error}"
                        ))
                    })?;
                let etag = response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let bytes = response.bytes().await.map_err(|error| {
                    AppError::FailedPrecondition(format!(
                        "failed to read OpenAPI surface '{url}': {error}"
                    ))
                })?;
                return Ok(FetchedSurfaceDocument {
                    bytes: bytes.to_vec(),
                    etag,
                });
            }
            _ => {}
        }
    }

    let path = PathBuf::from(url);
    if path.is_absolute() {
        return Ok(FetchedSurfaceDocument {
            bytes: std::fs::read(path)?,
            etag: None,
        });
    }

    Err(AppError::InvalidInput(format!(
        "OpenAPI surface url must be http, https, file, or an absolute local path: {url}"
    )))
}

fn write_materialized_surface(
    layout: &AppStateLayout,
    workspace_name: &WorkspaceName,
    source_name: &SourceName,
    entry: &MaterializedSurface,
) -> Result<(), AppError> {
    let dir = layout.source_model_materialization_dir(workspace_name, source_name, &entry.key);
    fs::ensure_dir(&dir)?;
    let ir_path = layout.source_model_ir_file(workspace_name, source_name, &entry.key);
    let metadata_path = layout.source_model_metadata_file(workspace_name, source_name, &entry.key);
    let ir_json = serde_json::to_vec_pretty(&entry.ir)?;
    let metadata_json = serde_json::to_vec_pretty(&entry.metadata)?;
    fs::write_atomic(&ir_path, &ir_json)?;
    fs::write_atomic(&metadata_path, &metadata_json)?;
    Ok(())
}

fn validate_metadata(
    source_name: &SourceName,
    source_version: &str,
    surface: &coral_spec::SourceModelManifestSurface,
    metadata: &MaterializedSourceModelMetadata,
) -> Result<(), AppError> {
    if metadata.source_name != source_name.as_str()
        || metadata.source_version != source_version
        || metadata.surface_id != surface.id
        || metadata.retrieval_url != surface.url
        || !metadata
            .pinned_sha256
            .eq_ignore_ascii_case(surface.sha256.trim())
        || metadata.importer_version != OPENAPI_IMPORTER_VERSION
    {
        return Err(AppError::FailedPrecondition(format!(
            "source '{source_name}' surface '{}' materialized source-model IR does not match the current manifest; run `coral source refresh {source_name}` or reinstall the source",
            surface.id
        )));
    }
    Ok(())
}

fn combine_surface_irs<I>(entries: I) -> SourceModelIr
where
    I: IntoIterator<Item = SourceModelIr>,
{
    let mut combined = SourceModelIr {
        ir_version: coral_spec::SOURCE_MODEL_IR_VERSION,
        surfaces: Vec::new(),
        types: Vec::new(),
        operations: Vec::new(),
        entities: Vec::new(),
    };
    for entry in entries {
        combined.surfaces.extend(entry.surfaces);
        combined.types.extend(entry.types);
        combined.operations.extend(entry.operations);
        combined.entities.extend(entry.entities);
    }
    combined
}

fn materialization_key(
    source_version: &str,
    surface: &coral_spec::SourceModelManifestSurface,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_version.as_bytes());
    hasher.update(b"\0");
    hasher.update(surface.id.as_bytes());
    hasher.update(b"\0open-api\0");
    hasher.update(surface.url.as_bytes());
    hasher.update(b"\0");
    hasher.update(surface.sha256.to_ascii_lowercase().as_bytes());
    hasher.update(b"\0");
    hasher.update(OPENAPI_IMPORTER_VERSION.as_bytes());
    hex_digest(hasher.finalize())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher.finalize())
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to a string cannot fail");
    }
    out
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::sha256_hex;

    pub(crate) fn document_sha256(document: &str) -> String {
        sha256_hex(document.as_bytes())
    }
}
