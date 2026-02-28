//! waferctl - CLI for managing WAFER pipeline instances.

mod client;
mod config;
mod error;
mod output;

use clap::{Parser, Subcommand};

use client::WaferClient;
use config::CtlConfig;
use error::{exit_code, CliError, ResultExt};

/// waferctl - Manage WAFER pipeline instances
#[derive(Parser)]
#[command(name = "waferctl")]
#[command(version, about, long_about = None)]
struct Cli {
    /// Output format (human-readable or JSON)
    #[arg(long, global = true)]
    json: bool,

    /// Endpoint name or URL to connect to
    #[arg(short, long, global = true)]
    endpoint: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check if the runtime is healthy
    Health,

    /// Show pipeline status
    Status,

    /// List all nodes in the pipeline
    Nodes {
        /// Show additional columns
        #[arg(short, long)]
        wide: bool,
    },

    /// Show details for a specific node
    Node {
        /// Node ID
        id: String,
    },

    /// Trigger hot-swap on a node
    HotSwap {
        /// Node ID to hot-swap
        node_id: String,
    },

    /// Reload configuration and hot-swap changed nodes
    Reload,

    /// Drain the pipeline
    Drain,

    /// Shutdown the pipeline gracefully
    Shutdown,

    /// Show pipeline metrics
    Metrics {
        /// Output raw Prometheus format
        #[arg(long)]
        raw: bool,
    },

    /// Manage endpoint configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set an endpoint URL
    SetEndpoint {
        /// Endpoint name
        name: String,
        /// Endpoint URL
        url: String,
    },

    /// Set the default endpoint
    Use {
        /// Endpoint name to use as default
        name: String,
    },

    /// List configured endpoints
    List,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = run(cli).await;

    match result {
        Ok(()) => std::process::exit(exit_code::SUCCESS),
        Err(err) => err.exit(Cli::parse().json),
    }
}

async fn run(cli: Cli) -> error::Result<()> {
    // Handle config commands first (don't need a client)
    if let Commands::Config { action } = &cli.command {
        return handle_config_command(action, cli.json);
    }

    // Resolve endpoint URL
    let config = CtlConfig::load().user_err()?;
    let endpoint_url = resolve_endpoint(&cli.endpoint, &config)?;

    // Create client
    let client = WaferClient::new(&endpoint_url).user_err()?;

    // Execute command
    match cli.command {
        Commands::Health => cmd_health(&client, cli.json).await,
        Commands::Status => cmd_status(&client, cli.json).await,
        Commands::Nodes { wide } => cmd_nodes(&client, cli.json, wide).await,
        Commands::Node { id } => cmd_node(&client, &id, cli.json).await,
        Commands::HotSwap { node_id } => cmd_hot_swap(&client, &node_id, cli.json).await,
        Commands::Reload => cmd_reload(&client, cli.json).await,
        Commands::Drain => cmd_drain(&client, cli.json).await,
        Commands::Shutdown => cmd_shutdown(&client, cli.json).await,
        Commands::Metrics { raw } => cmd_metrics(&client, cli.json, raw).await,
        Commands::Config { .. } => unreachable!(),
    }
}

fn resolve_endpoint(endpoint_arg: &Option<String>, config: &CtlConfig) -> error::Result<String> {
    match endpoint_arg {
        Some(ep) => {
            // Check if it's a URL or an endpoint name
            if ep.starts_with("http://") || ep.starts_with("https://") {
                Ok(ep.clone())
            } else {
                config.get_endpoint(ep).ok_or_else(|| {
                    CliError::user(anyhow::anyhow!("Endpoint '{}' not found in config", ep))
                        .with_hint("Run 'waferctl config list' to see available endpoints.")
                })
            }
        }
        None => config.get_default_endpoint().ok_or_else(|| {
            CliError::user(anyhow::anyhow!("No default endpoint configured"))
                .with_hint("Use --endpoint <url> or run 'waferctl config set-endpoint <name> <url>'")
        }),
    }
}

fn handle_config_command(action: &ConfigAction, json: bool) -> error::Result<()> {
    let mut config = CtlConfig::load().user_err()?;

    match action {
        ConfigAction::SetEndpoint { name, url } => {
            config.set_endpoint(name, url);
            config.save().user_err()?;
            if json {
                println!(
                    r#"{{"ok": true, "message": "Endpoint '{}' set to '{}'"}}"#,
                    name, url
                );
            } else {
                println!("✓ Endpoint '{}' set to '{}'", name, url);
            }
        }
        ConfigAction::Use { name } => {
            if !config.has_endpoint(name) {
                return Err(CliError::user(anyhow::anyhow!(
                    "Endpoint '{}' not found",
                    name
                ))
                .with_hint("Run 'waferctl config list' to see available endpoints."));
            }
            config.set_default(name);
            config.save().user_err()?;
            if json {
                println!(
                    r#"{{"ok": true, "message": "Default endpoint set to '{}'"}}"#,
                    name
                );
            } else {
                println!("✓ Default endpoint set to '{}'", name);
            }
        }
        ConfigAction::List => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&config).user_err()?
                );
            } else {
                output::print_config(&config);
            }
        }
    }

    Ok(())
}

async fn cmd_health(client: &WaferClient, json: bool) -> error::Result<()> {
    let response = client.health().await.classify()?;
    if json {
        println!("{}", serde_json::to_string(&response).user_err()?);
    } else {
        println!("✓ Healthy");
    }
    Ok(())
}

async fn cmd_status(client: &WaferClient, json: bool) -> error::Result<()> {
    let status = client.status().await.classify()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status).user_err()?);
    } else {
        output::print_status(&status);
    }
    Ok(())
}

async fn cmd_nodes(client: &WaferClient, json: bool, wide: bool) -> error::Result<()> {
    let nodes = client.nodes().await.classify()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&nodes).user_err()?);
    } else {
        output::print_nodes(&nodes, wide);
    }
    Ok(())
}

async fn cmd_node(client: &WaferClient, id: &str, json: bool) -> error::Result<()> {
    let node = client.node(id).await.classify()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&node).user_err()?);
    } else {
        output::print_node_detail(&node);
    }
    Ok(())
}

async fn cmd_hot_swap(client: &WaferClient, node_id: &str, json: bool) -> error::Result<()> {
    if !json {
        println!("Triggering hot-swap for node '{node_id}'...");
    }

    let result = client.hot_swap(node_id).await.classify()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result).user_err()?);
    } else {
        output::print_hot_swap_result(&result);
    }
    Ok(())
}

async fn cmd_reload(client: &WaferClient, json: bool) -> error::Result<()> {
    if !json {
        println!("Reloading configuration...");
    }

    let result = client.reload().await.classify()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result).user_err()?);
    } else {
        output::print_reload_result(&result);
    }
    Ok(())
}

async fn cmd_drain(client: &WaferClient, json: bool) -> error::Result<()> {
    if !json {
        println!("Draining pipeline...");
    }

    client.drain().await.classify()?;

    if json {
        println!(r#"{{"ok": true, "message": "Pipeline drained"}}"#);
    } else {
        println!("✓ Pipeline drained");
    }
    Ok(())
}

async fn cmd_shutdown(client: &WaferClient, json: bool) -> error::Result<()> {
    if !json {
        println!("Shutting down pipeline...");
    }

    client.shutdown().await.classify()?;

    if json {
        println!(r#"{{"ok": true, "message": "Pipeline shut down"}}"#);
    } else {
        println!("✓ Pipeline shut down");
    }
    Ok(())
}

async fn cmd_metrics(client: &WaferClient, json: bool, raw: bool) -> error::Result<()> {
    if raw {
        let metrics = client.metrics_raw().await.classify()?;
        println!("{metrics}");
    } else {
        let metrics = client.metrics().await.classify()?;
        if json {
            println!("{}", serde_json::to_string_pretty(&metrics).user_err()?);
        } else {
            output::print_metrics(&metrics);
        }
    }
    Ok(())
}
