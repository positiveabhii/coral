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
//! - resources: `coral://guide`, `coral://tables`
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
#[derive(Debug, Clone)]
pub struct McpOptions {
    /// Expose the feedback submission tool.
    pub feedback_enabled: bool,
    /// Optional W3C traceparent used to parent each MCP request span.
    pub trace_parent: Option<String>,
    /// Full version string for the running binary, used in the MCP `serverInfo` response.
    pub long_version: &'static str,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            feedback_enabled: false,
            trace_parent: None,
            long_version: env!("CARGO_PKG_VERSION"),
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
