//! Embedded build identity for CLI and MCP debug workflows.

/// Build identity embedded into the `coral` binary at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Full `coral --version` output for the running binary.
    pub long_version: &'static str,
    /// Package version from `CARGO_PKG_VERSION`.
    pub version: &'static str,
    /// Short git commit SHA for the build.
    pub sha: &'static str,
    /// Debug-only working-tree hash captured at build time.
    pub wip_tree: Option<&'static str>,
    /// Debug-only source checkout path captured at build time.
    pub source_path: Option<&'static str>,
    /// Build profile, usually `debug` or `release`.
    pub profile: &'static str,
}

#[must_use]
/// Returns the build identity embedded into this binary.
pub const fn build_identity() -> BuildIdentity {
    BuildIdentity {
        long_version: env!("CORAL_LONG_VERSION"),
        version: env!("CARGO_PKG_VERSION"),
        sha: env!("CORAL_GIT_SHA"),
        wip_tree: option_env!("CORAL_WIP_TREE"),
        source_path: option_env!("CORAL_SOURCE_PATH"),
        profile: match option_env!("CORAL_BUILD_PROFILE") {
            Some(profile) => profile,
            None => "release",
        },
    }
}
