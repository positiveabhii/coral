//! `MCP` stdio server for Coral.
//!
//! This crate adapts the local Coral client from `coral-client` to the
//! official Rust `MCP` SDK on stdio.
//!
//! # Primary Entry Points
//!
//! - [`run_stdio_with_client`] serves `MCP` messages on stdio using an
//!   existing [`coral_client::AppClient`], typically bootstrapped by
//!   `coral-cli`.
//!
//! The exposed MCP surface is intentionally small:
//!
//! - tools: `sql`, paginated `list_tables`, `search_tables`, `describe_table`, `list_columns`, and optionally `feedback`
//! - resources: `coral://guide`, `coral://tables`, `coral://build`
//!
//! Protocol lifecycle, initialization, and stdio transport behavior should stay
//! inside the SDK integration rather than being reimplemented locally.

#![allow(
    unused_crate_dependencies,
    reason = "Library test targets inherit package dependencies that are consumed by sibling targets."
)]

mod error;
mod server;
mod surface;
mod telemetry;

#[cfg(test)]
mod tests;

use coral_client::AppClient;
use rmcp::ServiceExt;

pub use error::McpError;
pub(crate) use server::CoralMcpServer;

/// Optional MCP surface features.
#[derive(Debug, Clone, Default)]
pub struct McpOptions {
    /// Expose the feedback submission tool.
    pub feedback_enabled: bool,
    /// Optional W3C traceparent used to parent each MCP request span.
    pub trace_parent: Option<String>,
    /// Build identity for the running MCP binary.
    pub build_identity: BuildIdentity,
}

/// Build identity exposed through the MCP `coral://build` resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Full `coral --version` output for the running binary.
    pub long_version: &'static str,
    /// Package version for the running binary.
    pub version: &'static str,
    /// Short git commit SHA for the running binary.
    pub sha: &'static str,
    /// Debug-only working-tree hash captured at build time.
    pub wip_tree: Option<&'static str>,
    /// Debug-only source checkout path captured at build time.
    pub source_path: Option<&'static str>,
    /// Build profile, usually `debug` or `release`.
    pub profile: &'static str,
}

impl Default for BuildIdentity {
    fn default() -> Self {
        Self {
            long_version: "coral unknown",
            version: env!("CARGO_PKG_VERSION"),
            sha: "unknown",
            wip_tree: None,
            source_path: None,
            profile: "release",
        }
    }
}

/// Runs the `MCP` stdio server using an existing Coral client.
///
/// # Errors
///
/// Returns [`McpError`] if the stdio server cannot complete its `MCP`
/// lifecycle.
pub async fn run_stdio_with_client(app: AppClient, options: McpOptions) -> Result<(), McpError> {
    let server = Box::pin(
        CoralMcpServer::new(&app, options).serve((tokio::io::stdin(), tokio::io::stdout())),
    )
    .await?;
    let _ = server.waiting().await?;
    Ok(())
}
