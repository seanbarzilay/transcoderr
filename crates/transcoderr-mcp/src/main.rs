use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    transport::io::stdio,
    tool_handler, tool_router, ServerHandler, ServiceExt,
};

mod client;
mod tools;
use client::ApiClient;

#[derive(Parser, Debug, Clone)]
#[command(name = "transcoderr-mcp", version)]
struct Cli {
    /// transcoderr server base URL.
    #[arg(long, env = "TRANSCODERR_URL")]
    url: String,
    /// API token from Settings → API tokens.
    #[arg(long, env = "TRANSCODERR_TOKEN")]
    token: String,
    /// Per-call HTTP timeout, seconds.
    #[arg(long, env = "TRANSCODERR_TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,
}

#[derive(Clone)]
pub(crate) struct Server {
    pub(crate) api: ApiClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    pub fn new(api: ApiClient) -> Self {
        let tool_router = Self::tool_router() + Self::runs_router();
        Self { api, tool_router }
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_03_26,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("transcoderr MCP proxy -- drives runs, flows, sources, notifiers.".into()),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    // Build the API client, then probe an auth-gated endpoint to validate
    // URL reachability AND token authority before announcing capabilities.
    // /healthz alone wouldn't fail-fast on a bad token (it's not behind
    // require_auth).
    let api = ApiClient::new(cli.url.clone(), cli.token.clone(), cli.timeout_secs)?;
    api.probe().await.with_context(|| {
        format!("auth probe failed against {} — check TRANSCODERR_URL and TRANSCODERR_TOKEN", cli.url)
    })?;
    tracing::info!(url = %cli.url, "transcoderr-mcp starting");
    let server = Server::new(api);
    let (stdin, stdout) = stdio();
    server.serve((stdin, stdout)).await?.waiting().await?;
    Ok(())
}
