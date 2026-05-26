//! Per-instance MCP transport implementations.
//!
//! Today both stdio (`StdioMcpToolCaller`) and Streamable HTTP
//! (`StreamableHttpMcpToolCaller`) are supported. Both implementations create a
//! fresh MCP client session for each tool call; pooling is a future optimization.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use coral_spec::backends::mcp::McpServerSpec;
use datafusion::error::{DataFusionError, Result};
use rmcp::model::{CallToolRequestParams, ClientInfo, Implementation, JsonObject};
use rmcp::transport::ConfigureCommandExt;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientHandler, ServiceExt};
use serde_json::Value;
use tokio::process::Command;

use super::client::McpToolCaller;
use super::error::McpProviderQueryError;
use super::response::normalize_tool_result;
use crate::backends::shared::template::{RenderContext, resolve_value_source};

#[derive(Debug)]
pub(super) struct StdioMcpToolCaller {
    pub(super) source_name: String,
    pub(super) server: McpServerSpec,
    pub(super) resolved_inputs: Arc<BTreeMap<String, String>>,
}

#[async_trait]
impl McpToolCaller for StdioMcpToolCaller {
    async fn call_tool(
        &self,
        relation: &str,
        tool_name: &str,
        arguments: JsonObject,
    ) -> Result<Value> {
        let McpServerSpec::Stdio { command, args, env } = &self.server else {
            unreachable!("StdioMcpToolCaller requires a stdio MCP server spec");
        };
        let mut command = Command::new(command);
        command.args(args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let render_context = RenderContext::source_scoped(&self.resolved_inputs);
        for env in env {
            let Some(value) = resolve_value_source(&env.value, &render_context)? else {
                continue;
            };
            command.env(&env.name, value_to_env_string(value));
        }

        let transport = rmcp::transport::TokioChildProcess::new(command.configure(|cmd| {
            cmd.kill_on_drop(true);
        }))
        .map_err(|error| {
            DataFusionError::External(Box::new(McpProviderQueryError::ServerStart {
                source_schema: self.source_name.clone(),
                detail: error.to_string(),
            }))
        })?;
        let client = McpClientHandler::new(&self.source_name)
            .serve(transport)
            .await
            .map_err(|error| {
                DataFusionError::External(Box::new(McpProviderQueryError::Initialize {
                    source_schema: self.source_name.clone(),
                    detail: error.to_string(),
                }))
            })?;
        let result = client
            .call_tool(CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments))
            .await
            .map_err(|error| {
                DataFusionError::External(Box::new(McpProviderQueryError::ToolCall {
                    source_schema: self.source_name.clone(),
                    relation: relation.to_string(),
                    tool: tool_name.to_string(),
                    detail: error.to_string(),
                }))
            })?;
        normalize_tool_result(&self.source_name, relation, tool_name, result)
    }
}

#[derive(Debug)]
pub(super) struct StreamableHttpMcpToolCaller {
    pub(super) source_name: String,
    pub(super) server: McpServerSpec,
    pub(super) resolved_inputs: Arc<BTreeMap<String, String>>,
}

#[async_trait]
impl McpToolCaller for StreamableHttpMcpToolCaller {
    async fn call_tool(
        &self,
        relation: &str,
        tool_name: &str,
        arguments: JsonObject,
    ) -> Result<Value> {
        let McpServerSpec::StreamableHttp { url, auth } = &self.server else {
            unreachable!("StreamableHttpMcpToolCaller requires a Streamable HTTP MCP server spec");
        };
        let mut config = StreamableHttpClientTransportConfig::with_uri(url.clone())
            .reinit_on_expired_session(true);
        if let Some(auth) = auth
            && let Some(token) = resolve_value_source(
                auth.bearer_token(),
                &RenderContext::source_scoped(&self.resolved_inputs),
            )?
        {
            config = config.auth_header(value_to_env_string(token));
        }

        let transport = StreamableHttpClientTransport::from_config(config);
        let client = McpClientHandler::new(&self.source_name)
            .serve(transport)
            .await
            .map_err(|error| {
                DataFusionError::External(Box::new(mcp_http_initialize_error(
                    &self.source_name,
                    error.to_string(),
                )))
            })?;
        let result = client
            .call_tool(CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments))
            .await
            .map_err(|error| {
                DataFusionError::External(Box::new(mcp_http_tool_call_error(
                    &self.source_name,
                    relation,
                    tool_name,
                    error.to_string(),
                )))
            })?;
        normalize_tool_result(&self.source_name, relation, tool_name, result)
    }
}

#[derive(Debug, Clone)]
struct McpClientHandler {
    client_info: ClientInfo,
}

impl McpClientHandler {
    fn new(source_name: &str) -> Self {
        let mut client_info = ClientInfo::default();
        client_info.client_info = Implementation::new(
            format!("coral-engine/{source_name}"),
            env!("CARGO_PKG_VERSION"),
        );
        Self { client_info }
    }
}

impl ClientHandler for McpClientHandler {
    fn get_info(&self) -> ClientInfo {
        self.client_info.clone()
    }
}

fn value_to_env_string(value: Value) -> String {
    match value {
        Value::String(value) => value,
        other => other.to_string(),
    }
}

fn mcp_http_initialize_error(source_schema: &str, detail: String) -> McpProviderQueryError {
    if detail.contains("Auth required") {
        return McpProviderQueryError::AuthRequired {
            source_schema: source_schema.to_string(),
            detail,
        };
    }
    if detail.contains("Insufficient scope") {
        return McpProviderQueryError::AuthFailed {
            source_schema: source_schema.to_string(),
            detail,
        };
    }
    McpProviderQueryError::Initialize {
        source_schema: source_schema.to_string(),
        detail,
    }
}

fn mcp_http_tool_call_error(
    source_schema: &str,
    relation: &str,
    tool: &str,
    detail: String,
) -> McpProviderQueryError {
    if detail.contains("Auth required") {
        return McpProviderQueryError::AuthRequired {
            source_schema: source_schema.to_string(),
            detail,
        };
    }
    if detail.contains("Insufficient scope") {
        return McpProviderQueryError::AuthFailed {
            source_schema: source_schema.to_string(),
            detail,
        };
    }
    McpProviderQueryError::ToolCall {
        source_schema: source_schema.to_string(),
        relation: relation.to_string(),
        tool: tool.to_string(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use rmcp::model::JsonObject;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_partial_json, header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn streamable_http_manifest(url: &str) -> coral_spec::McpSourceManifest {
        let manifest = coral_spec::parse_source_manifest_value(json!({
            "dsl_version": 3,
            "name": "remote_mcp",
            "version": "0.1.0",
            "backend": "mcp",
            "inputs": {
                "MCP_ACCESS_TOKEN": { "kind": "secret" }
            },
            "server": {
                "transport": "streamable_http",
                "url": url,
                "auth": {
                    "type": "bearer",
                    "from": "input",
                    "key": "MCP_ACCESS_TOKEN"
                }
            },
            "tables": [{
                "name": "issues",
                "tool": "list_issues",
                "columns": [{ "name": "title", "type": "Utf8" }]
            }]
        }))
        .expect("manifest should parse");
        manifest.as_mcp().expect("expected mcp manifest").clone()
    }

    fn initialize_response() -> ResponseTemplate {
        ResponseTemplate::new(200)
            .append_header("Content-Type", "application/json")
            .set_body_json(json!({
                "jsonrpc": "2.0",
                "id": 0,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "serverInfo": {
                        "name": "fixture",
                        "version": "0.1.0"
                    }
                }
            }))
    }

    #[tokio::test]
    async fn streamable_http_caller_sends_bearer_token_and_decodes_tool_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({ "method": "initialize" })))
            .respond_with(initialize_response())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({ "method": "notifications/initialized" }),
            ))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer secret-token"))
            .and(body_partial_json(json!({
                "method": "tools/call",
                "params": {
                    "name": "list_issues",
                    "arguments": { "state": "open" }
                }
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Content-Type", "application/json")
                    .set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "structuredContent": {
                                "issues": [{ "title": "Bug A" }]
                            }
                        }
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let manifest = streamable_http_manifest(&server.uri());
        let mut secrets = BTreeMap::new();
        secrets.insert("MCP_ACCESS_TOKEN".to_string(), "secret-token".to_string());
        let resolved_inputs = Arc::new(coral_spec::resolve_inputs(
            &manifest.declared_inputs,
            &secrets,
            &BTreeMap::new(),
        ));
        let caller = StreamableHttpMcpToolCaller {
            source_name: manifest.common.name,
            server: manifest.server,
            resolved_inputs,
        };
        let mut arguments = JsonObject::new();
        arguments.insert("state".to_string(), Value::String("open".to_string()));

        let payload = caller
            .call_tool("issues", "list_issues", arguments)
            .await
            .expect("tool call should succeed");

        let title = payload
            .get("issues")
            .and_then(Value::as_array)
            .and_then(|issues| issues.first())
            .and_then(|issue| issue.get("title"))
            .and_then(Value::as_str);
        assert_eq!(title, Some("Bug A"));
    }
}
