use std::sync::Arc;

use coral_app::AwsEngineExtensionsProvider;
use coral_client::{
    AppClient, ClientError,
    local::{LocalServerError, RunningServer, ServerBuilder},
};

pub(crate) struct Bootstrap {
    pub(crate) app: AppClient,
    pub(crate) _server: Option<RunningServer>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BootstrapError {
    #[error(transparent)]
    Startup(#[from] LocalServerError),
    #[error(transparent)]
    Connect(#[from] ClientError),
    #[cfg(feature = "embedded-ui")]
    #[error(
        "embedded UI assets are missing; run `npm run build --prefix ui` and rebuild `coral` with the `embedded-ui` feature"
    )]
    EmbeddedUiMissing,
}

pub(crate) async fn bootstrap() -> Result<Bootstrap, BootstrapError> {
    if let Some(endpoint) = bootstrap_endpoint() {
        return Ok(Bootstrap {
            app: AppClient::connect(&endpoint).await?,
            _server: None,
        });
    }

    let server = configure_server_builder(ServerBuilder::new())
        .start()
        .await?;
    let app = AppClient::connect(server.endpoint_uri()).await?;
    Ok(Bootstrap {
        app,
        _server: Some(server),
    })
}

#[cfg(feature = "server")]
pub(crate) async fn start_dev_server(
    bind_addr: std::net::SocketAddr,
) -> Result<RunningServer, BootstrapError> {
    Ok(configure_server_builder(ServerBuilder::new())
        .with_bind_addr(bind_addr)
        .with_grpc_web()
        .start()
        .await?)
}

#[cfg(feature = "embedded-ui")]
pub(crate) async fn start_ui_server(
    bind_addr: std::net::SocketAddr,
) -> Result<RunningServer, BootstrapError> {
    if !crate::embedded_ui_assets_available() {
        return Err(BootstrapError::EmbeddedUiMissing);
    }

    Ok(configure_server_builder(ServerBuilder::new())
        .with_bind_addr(bind_addr)
        .with_grpc_web()
        .with_static_assets(crate::embedded_ui_assets())
        .start()
        .await?)
}

fn configure_server_builder(builder: ServerBuilder) -> ServerBuilder {
    builder.add_engine_extensions_provider(Arc::new(AwsEngineExtensionsProvider))
}

#[cfg(feature = "cli-test-server")]
fn bootstrap_endpoint() -> Option<String> {
    crate::env::bootstrap_endpoint()
}

#[cfg(not(feature = "cli-test-server"))]
fn bootstrap_endpoint() -> Option<String> {
    None
}
