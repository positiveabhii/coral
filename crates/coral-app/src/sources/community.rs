//! Community source catalog and on-demand manifest resolution.
//!
//! Community sources live in `sources/community/<name>/manifest.yaml` and are
//! NOT bundled into the binary. The build script (`build.rs`) emits a static
//! `COMMUNITY_CATALOG` table containing only metadata (name, version,
//! description, repo-relative manifest path). Manifest YAML is resolved
//! lazily when a user asks for source info or starts an install:
//!
//! * Release builds (`CORAL_BUILD_COMMIT_SHA` set) fetch the manifest from
//!   `raw.githubusercontent.com` pinned to the build commit, so a given
//!   binary always sees the same manifest text it was tested against.
//! * Dev builds (empty SHA) read from the local workspace tree at the path
//!   baked in by `build.rs` (`CORAL_DEV_WORKSPACE_ROOT`).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use crate::bootstrap::AppError;
use crate::sources::SourceName;
use crate::sources::catalog::describe_manifest;
use crate::sources::model::{CandidateSource, SourceOrigin};

pub(crate) struct CommunityCatalogEntry {
    pub(crate) name: &'static str,
    pub(crate) version: &'static str,
    pub(crate) description: &'static str,
    pub(crate) manifest_path: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/community_catalog.rs"));

const BUILD_COMMIT_SHA: &str = env!("CORAL_BUILD_COMMIT_SHA");
const DEV_WORKSPACE_ROOT: &str = env!("CORAL_DEV_WORKSPACE_ROOT");
const GITHUB_RAW_BASE: &str = "https://raw.githubusercontent.com/withcoral/coral";
const MANIFEST_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Return community-catalog summaries with empty inputs (resolved on demand).
pub(crate) fn list_community_summaries(
    installed_source_names: &BTreeSet<SourceName>,
) -> Result<Vec<CandidateSource>, AppError> {
    let mut summaries = Vec::with_capacity(COMMUNITY_CATALOG.len());
    for entry in COMMUNITY_CATALOG {
        let name = SourceName::parse(entry.name)?;
        let installed = installed_source_names.contains(&name);
        summaries.push(CandidateSource {
            name,
            description: entry.description.to_string(),
            version: entry.version.to_string(),
            inputs: Vec::new(),
            installed,
            origin: SourceOrigin::Community,
        });
    }
    summaries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(summaries)
}

pub(crate) fn find_community_entry(name: &SourceName) -> Option<&'static CommunityCatalogEntry> {
    COMMUNITY_CATALOG
        .iter()
        .find(|entry| entry.name == name.as_str())
}

/// Resolve a community source manifest YAML and parse it into a candidate
/// alongside the raw YAML (needed by the existing import path).
pub(crate) async fn resolve_community_source(
    name: &SourceName,
) -> Result<ResolvedCommunitySource, AppError> {
    let entry = find_community_entry(name)
        .ok_or_else(|| AppError::SourceNotFound(name.to_string()))?;
    let manifest_yaml = fetch_manifest(entry).await?;
    let candidate = describe_manifest(&manifest_yaml, SourceOrigin::Community, false)?;
    if candidate.name.as_str() != entry.name {
        return Err(AppError::FailedPrecondition(format!(
            "community source '{}' resolves to manifest name '{}'",
            entry.name, candidate.name
        )));
    }
    Ok(ResolvedCommunitySource {
        manifest_yaml,
        candidate,
    })
}

pub(crate) struct ResolvedCommunitySource {
    pub(crate) manifest_yaml: String,
    pub(crate) candidate: CandidateSource,
}

async fn fetch_manifest(entry: &CommunityCatalogEntry) -> Result<String, AppError> {
    if BUILD_COMMIT_SHA.is_empty() {
        let path = PathBuf::from(DEV_WORKSPACE_ROOT).join(entry.manifest_path);
        return std::fs::read_to_string(&path).map_err(|error| {
            AppError::FailedPrecondition(format!(
                "could not read community manifest at {}: {error}",
                path.display()
            ))
        });
    }
    let url = format!(
        "{GITHUB_RAW_BASE}/{BUILD_COMMIT_SHA}/{path}",
        path = entry.manifest_path
    );
    let client = reqwest::Client::builder()
        .timeout(MANIFEST_FETCH_TIMEOUT)
        .build()
        .map_err(|error| {
            AppError::FailedPrecondition(format!("could not build manifest fetch client: {error}"))
        })?;
    let response = client.get(&url).send().await.map_err(|error| {
        AppError::FailedPrecondition(format!(
            "could not fetch community manifest from {url}: {error}"
        ))
    })?;
    let response = response.error_for_status().map_err(|error| {
        AppError::FailedPrecondition(format!(
            "community manifest fetch returned error from {url}: {error}"
        ))
    })?;
    response.text().await.map_err(|error| {
        AppError::FailedPrecondition(format!(
            "could not read community manifest body from {url}: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "summary ordering assertions intentionally fail loudly in tests"
    )]

    use super::*;

    #[test]
    fn community_catalog_is_not_empty() {
        assert!(
            !COMMUNITY_CATALOG.is_empty(),
            "community catalog must contain at least one entry"
        );
    }

    #[test]
    fn list_summaries_sorts_by_name_and_marks_installed() {
        let mut installed = BTreeSet::new();
        if let Some(first) = COMMUNITY_CATALOG.first() {
            installed.insert(SourceName::parse(first.name).expect("source name"));
        }
        let summaries = list_community_summaries(&installed).expect("summaries");
        assert_eq!(summaries.len(), COMMUNITY_CATALOG.len());
        for window in summaries.windows(2) {
            assert!(
                window[0].name <= window[1].name,
                "summaries must be sorted"
            );
        }
        if let Some(first) = COMMUNITY_CATALOG.first() {
            let summary = summaries
                .iter()
                .find(|candidate| candidate.name.as_str() == first.name)
                .expect("expected summary");
            assert!(summary.installed, "installed flag should propagate");
        }
    }

    #[test]
    fn find_community_entry_returns_expected_source() {
        // `hn` ships as a community source today; if that changes, pick the
        // first catalog entry instead so the test still proves lookup works.
        let probe = SourceName::parse("hn").expect("source name");
        let entry = find_community_entry(&probe).expect("hn lives in community catalog");
        assert_eq!(entry.name, "hn");
        assert_eq!(entry.manifest_path, "sources/community/hn/manifest.yaml");
    }
}
