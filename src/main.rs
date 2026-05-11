use anyhow::{Context, Result, anyhow};
use aura_mcp::{
    aura::AuraApi,
    config::{CONFIG_ENV, Config, config_path},
    mcp,
    validation::parse_address,
};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "aura-mcp", version, about = "Local stdio MCP server for Aura")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Login {
        #[arg(long)]
        api_key: String,
        #[arg(long)]
        api_endpoint: Option<String>,
        #[arg(long)]
        read_only: Option<bool>,
    },
    Doctor,
    ListTools,
    PrintConfig {
        target: ConfigTarget,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ConfigTarget {
    Claude,
    Codex,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve => mcp::serve().await,
        Command::Login {
            api_key,
            api_endpoint,
            read_only,
        } => login(api_key, api_endpoint, read_only),
        Command::Doctor => doctor().await,
        Command::ListTools => list_tools(),
        Command::PrintConfig { target } => print_config(target),
    }
}

fn login(api_key: String, api_endpoint: Option<String>, read_only: Option<bool>) -> Result<()> {
    parse_address(&api_key, "api_key")
        .context("Aura API keys are expected to be Solana-address shaped")?;
    let path = config_path()?;
    let mut config = Config::load(&path).unwrap_or_default();
    if let Some(endpoint) = api_endpoint {
        if endpoint.trim().is_empty() {
            return Err(anyhow!("api_endpoint must not be empty"));
        }
        config.api_endpoint = endpoint;
    }
    config.api_key = Some(api_key);
    if let Some(read_only) = read_only {
        config.read_only = read_only;
    }
    config.save(&path)?;
    println!("Aura MCP config updated at {}", path.display());
    println!("API key stored locally and not printed.");
    Ok(())
}

async fn doctor() -> Result<()> {
    let path = config_path()?;
    println!("Aura MCP doctor");
    println!("config env: {CONFIG_ENV}");
    println!("config path: {}", path.display());

    if !path.exists() {
        println!("config exists: no");
        println!("next step: run `aura-mcp login --api-key <KEY>`");
        return Ok(());
    }
    println!("config exists: yes");

    let config = Config::load(&path)?;
    println!("api_endpoint: {}", present(&config.api_endpoint));
    println!(
        "api_key: {}",
        if config.api_key.as_deref().unwrap_or_default().is_empty() {
            "missing"
        } else {
            "present"
        }
    );
    println!("read_only: {}", config.read_only);

    if let Err(err) = config.validate_for_api() {
        println!("api config: invalid ({err})");
        return Ok(());
    }
    if let Some(key) = &config.api_key {
        match parse_address(key, "api_key") {
            Ok(_) => println!("api_key format: ok"),
            Err(err) => {
                println!("api_key format: invalid ({err})");
                return Ok(());
            }
        }
    }

    print!("api ping: ");
    let ping = tokio::time::timeout(Duration::from_secs(5), async {
        let api = AuraApi::connect(&config).await?;
        api.get_aura_status().await
    })
    .await;
    match ping {
        Ok(Ok(_)) => println!("ok"),
        Ok(Err(err)) => println!("failed ({err})"),
        Err(_) => println!("timeout after 5s"),
    }

    Ok(())
}

fn list_tools() -> Result<()> {
    for tool in mcp::tools() {
        let name = tool
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("<unknown>");
        let description = tool
            .get("description")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        println!("{name}\t{description}");
    }
    Ok(())
}

fn print_config(target: ConfigTarget) -> Result<()> {
    match target {
        ConfigTarget::Claude => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "mcpServers": {
                        "aura": {
                            "command": "aura-mcp",
                            "args": ["serve"]
                        }
                    }
                }))?
            );
        }
        ConfigTarget::Codex => {
            println!("[mcp_servers.aura]");
            println!("command = \"aura-mcp\"");
            println!("args = [\"serve\"]");
        }
    }
    Ok(())
}

fn present(value: &str) -> &str {
    if value.trim().is_empty() {
        "missing"
    } else {
        value
    }
}
