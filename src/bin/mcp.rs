use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use app::apiclient::{APP_SLUG, ApiClient, ClientConfig, mint_token_via_login};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};

#[derive(Clone)]
struct AppMcp {
    client: ApiClient,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl AppMcp {
    fn new(client: ApiClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Verify connectivity to the Rust Home Server homelab portal and return the signed-in user."
    )]
    async fn whoami(&self) -> Result<CallToolResult, McpError> {
        match self.client.me().await {
            Ok(user) => {
                let text = serde_json::to_string_pretty(&user)
                    .map_err(|err| McpError::internal_error(err.to_string(), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
            }
            Err(err) => Ok(CallToolResult::error(vec![ContentBlock::text(err)])),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AppMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                format!("{APP_SLUG}-mcp"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Rust Home Server homelab MCP server. Connects to the Rust Home Server API using personal access tokens.",
            )
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") | Some("--config") => {
            let rest = if args.first().map(String::as_str) == Some("serve") {
                &args[1..]
            } else {
                &args[..]
            };
            if let Err(err) = run_serve(rest).await {
                eprintln!("rust-home-server-mcp error: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some("token") => {
            if let Err(err) = run_token(&args[1..]).await {
                eprintln!("rust-home-server-mcp token error: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some("-h" | "--help" | "help") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown subcommand '{other}'\n");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  rust-home-server-mcp [serve] [--config <path>]
  rust-home-server-mcp token [--url <app-url>] [--email <email>] [--name <token-name>] [--config <path>]"
    );
}

async fn run_serve(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--config requires a path".to_string())?;
                config_path = Some(PathBuf::from(val));
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }

    let cfg = ClientConfig::load(config_path.as_deref())?;
    let client = ApiClient::new(&cfg)?;
    let service = AppMcp::new(client)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|err| format!("failed to start MCP stdio server: {err}"))?;

    service
        .waiting()
        .await
        .map_err(|err| format!("MCP stdio session terminated with error: {err}"))?;
    Ok(())
}

async fn run_token(args: &[String]) -> Result<(), String> {
    let mut url: Option<String> = env::var("MCP_APP_URL")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let mut email: Option<String> = env::var("MCP_EMAIL").ok().filter(|v| !v.trim().is_empty());
    let mut token_name = format!("{APP_SLUG}-mcp");
    let mut config_path: Option<PathBuf> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--url" => {
                url = Some(
                    iter.next()
                        .ok_or_else(|| "--url requires a value".to_string())?
                        .clone(),
                );
            }
            "--email" => {
                email = Some(
                    iter.next()
                        .ok_or_else(|| "--email requires a value".to_string())?
                        .clone(),
                );
            }
            "--name" => {
                token_name = iter
                    .next()
                    .ok_or_else(|| "--name requires a value".to_string())?
                    .clone();
            }
            "--config" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--config requires a path".to_string())?;
                config_path = Some(PathBuf::from(val));
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }

    let app_url = match url {
        Some(v) => v,
        None => prompt_line("App URL [http://localhost:8080]: ").map(|v| {
            if v.trim().is_empty() {
                "http://localhost:8080".to_string()
            } else {
                v.trim().to_string()
            }
        })?,
    };

    let user_email = match email {
        Some(v) => v,
        None => {
            let entered = prompt_line("Email: ")?;
            if entered.trim().is_empty() {
                return Err("email is required".to_string());
            }
            entered.trim().to_string()
        }
    };

    let password = match env::var("MCP_PASSWORD") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            if !io::stdin().is_terminal() {
                return Err("MCP_PASSWORD must be set when running non-interactively".to_string());
            }
            rpassword::prompt_password("Password: ")
                .map_err(|err| format!("failed to read password: {err}"))?
        }
    };

    let dest = match config_path {
        Some(p) => p,
        None => ClientConfig::default_path()?,
    };

    let created = mint_token_via_login(&app_url, &user_email, &password, &token_name).await?;
    let cfg = ClientConfig {
        app_url: app_url.trim().trim_end_matches('/').to_string(),
        token: created.token,
    };
    cfg.write_atomic(&dest)?;

    println!(
        "minted token '{}' (...{}) and wrote {}",
        created.name,
        created.last4,
        dest.display()
    );
    Ok(())
}

fn prompt_line(prompt: &str) -> Result<String, String> {
    if !io::stdin().is_terminal() {
        return Err(format!(
            "missing required value for '{prompt}' in non-interactive mode"
        ));
    }
    eprint!("{prompt}");
    io::stderr()
        .flush()
        .map_err(|err| format!("failed to flush prompt: {err}"))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|err| format!("failed to read input: {err}"))?;
    Ok(line)
}
