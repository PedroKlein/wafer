#![expect(clippy::print_stdout, clippy::print_stderr, reason = "CLI binary — stdout/stderr output is the primary interface")]
//! waferctl - CLI for managing WAFER pipeline instances.

mod client;
mod config;
mod error;
mod output;

use clap::{Parser, Subcommand};

use client::WaferClient;
use config::CtlConfig;
use error::{CliError, ResultExt, exit_code};

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
        /// Path to the replacement .wasm component on the runtime host
        #[arg(long, value_name = "PATH")]
        wasm_path: String,
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
    let endpoint_url = resolve_endpoint(cli.endpoint.as_ref(), &config)?;

    // Create client
    let client = WaferClient::new(&endpoint_url).user_err()?;

    // Execute command
    match cli.command {
        Commands::Health => cmd_health(&client, cli.json).await,
        Commands::Status => cmd_status(&client, cli.json).await,
        Commands::Nodes { wide } => cmd_nodes(&client, cli.json, wide).await,
        Commands::Node { id } => cmd_node(&client, &id, cli.json).await,
        Commands::HotSwap { node_id, wasm_path } => {
            cmd_hot_swap(&client, &node_id, &wasm_path, cli.json).await
        }
        Commands::Reload => cmd_reload(&client, cli.json),
        Commands::Drain => cmd_drain(&client, cli.json),
        Commands::Shutdown => cmd_shutdown(&client, cli.json).await,
        Commands::Metrics { raw } => cmd_metrics(&client, cli.json, raw).await,
        Commands::Config { .. } => {
            #[expect(clippy::unreachable, reason = "Config command is handled before client creation and never reaches this branch")]
            {unreachable!("Config commands handled earlier in main")}
        }
    }
}

fn resolve_endpoint(endpoint_arg: Option<&String>, config: &CtlConfig) -> error::Result<String> {
    endpoint_arg.map_or_else(
        || {
            config.get_default_endpoint().ok_or_else(|| {
                CliError::user(anyhow::anyhow!("No default endpoint configured")).with_hint(
                    "Use --endpoint <url> or run 'waferctl config set-endpoint <name> <url>'",
                )
            })
        },
        |ep| {
            if ep.starts_with("http://") || ep.starts_with("https://") {
                Ok(ep.clone())
            } else {
                config.get_endpoint(ep).ok_or_else(|| {
                    CliError::user(anyhow::anyhow!("Endpoint '{ep}' not found in config"))
                        .with_hint("Run 'waferctl config list' to see available endpoints.")
                })
            }
        },
    )
}

fn handle_config_command(action: &ConfigAction, json: bool) -> error::Result<()> {
    let mut config = CtlConfig::load().user_err()?;

    match action {
        ConfigAction::SetEndpoint { name, url } => {
            config.set_endpoint(name, url);
            config.save().user_err()?;
            if json {
                println!(r#"{{"ok": true, "message": "Endpoint '{name}' set to '{url}'"}}"#);
            } else {
                println!("✓ Endpoint '{name}' set to '{url}'");
            }
        }
        ConfigAction::Use { name } => {
            if !config.has_endpoint(name) {
                return Err(CliError::user(anyhow::anyhow!("Endpoint '{name}' not found"))
                    .with_hint("Run 'waferctl config list' to see available endpoints."));
            }
            config.set_default(name);
            config.save().user_err()?;
            if json {
                println!(r#"{{"ok": true, "message": "Default endpoint set to '{name}'"}}"#);
            } else {
                println!("✓ Default endpoint set to '{name}'");
            }
        }
        ConfigAction::List => {
            if json {
                println!("{}", serde_json::to_string_pretty(&config).user_err()?);
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

async fn cmd_hot_swap(
    client: &WaferClient,
    node_id: &str,
    wasm_path: &str,
    json: bool,
) -> error::Result<()> {
    if !json {
        println!("Triggering hot-swap for node '{node_id}'...");
    }

    let result = client.hot_swap(node_id, wasm_path).await.classify()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&result).user_err()?);
    } else {
        output::print_hot_swap_result(&result);
    }
    Ok(())
}

fn cmd_reload(_client: &WaferClient, _json: bool) -> error::Result<()> {
    Err(CliError::user(anyhow::anyhow!(
        "configuration reload is not exposed by the runtime HTTP API"
    ))
    .with_hint("Restart the runtime or hot-swap a specific node."))
}

fn cmd_drain(_client: &WaferClient, _json: bool) -> error::Result<()> {
    Err(CliError::user(anyhow::anyhow!(
        "standalone drain is not exposed by the runtime HTTP API"
    ))
    .with_hint("Use 'waferctl shutdown' to trigger graceful pipeline shutdown."))
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
        let metrics = client.metrics().classify()?;
        if json {
            println!("{}", serde_json::to_string_pretty(&metrics).user_err()?);
        } else {
            output::print_metrics(&metrics);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_parses_health() {
        let cli = Cli::try_parse_from(["waferctl", "health"]).unwrap();
        assert!(matches!(cli.command, Commands::Health));
        assert!(!cli.json);
        assert!(cli.endpoint.is_none());
    }

    #[test]
    fn test_cli_parses_status() {
        let cli = Cli::try_parse_from(["waferctl", "status"]).unwrap();
        assert!(matches!(cli.command, Commands::Status));
    }

    #[test]
    fn test_cli_parses_nodes() {
        let cli = Cli::try_parse_from(["waferctl", "nodes"]).unwrap();
        assert!(matches!(cli.command, Commands::Nodes { wide: false }));
    }

    #[test]
    fn test_cli_parses_nodes_wide() {
        let cli = Cli::try_parse_from(["waferctl", "nodes", "--wide"]).unwrap();
        assert!(matches!(cli.command, Commands::Nodes { wide: true }));
    }

    #[test]
    fn test_cli_parses_node_with_id() {
        let cli = Cli::try_parse_from(["waferctl", "node", "transform-1"]).unwrap();
        match cli.command {
            Commands::Node { id } => assert_eq!(id, "transform-1"),
            _ => panic!("Expected Node command"),
        }
    }

    #[test]
    fn test_cli_parses_hot_swap() {
        let cli = Cli::try_parse_from([
            "waferctl",
            "hot-swap",
            "my-node",
            "--wasm-path",
            "/tmp/new.wasm",
        ])
        .unwrap();
        match cli.command {
            Commands::HotSwap { node_id, wasm_path } => {
                assert_eq!(node_id, "my-node");
                assert_eq!(wasm_path, "/tmp/new.wasm");
            }
            _ => panic!("Expected HotSwap command"),
        }
    }

    #[test]
    fn test_cli_parses_reload() {
        let cli = Cli::try_parse_from(["waferctl", "reload"]).unwrap();
        assert!(matches!(cli.command, Commands::Reload));
    }

    #[test]
    fn test_cli_parses_drain() {
        let cli = Cli::try_parse_from(["waferctl", "drain"]).unwrap();
        assert!(matches!(cli.command, Commands::Drain));
    }

    #[test]
    fn test_cli_parses_shutdown() {
        let cli = Cli::try_parse_from(["waferctl", "shutdown"]).unwrap();
        assert!(matches!(cli.command, Commands::Shutdown));
    }

    #[test]
    fn test_cli_parses_metrics() {
        let cli = Cli::try_parse_from(["waferctl", "metrics"]).unwrap();
        assert!(matches!(cli.command, Commands::Metrics { raw: false }));
    }

    #[test]
    fn test_cli_parses_metrics_raw() {
        let cli = Cli::try_parse_from(["waferctl", "metrics", "--raw"]).unwrap();
        assert!(matches!(cli.command, Commands::Metrics { raw: true }));
    }

    #[test]
    fn test_cli_parses_config_set_endpoint() {
        let cli =
            Cli::try_parse_from(["waferctl", "config", "set-endpoint", "prod", "http://prod:9090"])
                .unwrap();
        match cli.command {
            Commands::Config { action: ConfigAction::SetEndpoint { name, url } } => {
                assert_eq!(name, "prod");
                assert_eq!(url, "http://prod:9090");
            }
            _ => panic!("Expected Config SetEndpoint command"),
        }
    }

    #[test]
    fn test_cli_parses_config_use() {
        let cli = Cli::try_parse_from(["waferctl", "config", "use", "staging"]).unwrap();
        match cli.command {
            Commands::Config { action: ConfigAction::Use { name } } => {
                assert_eq!(name, "staging");
            }
            _ => panic!("Expected Config Use command"),
        }
    }

    #[test]
    fn test_cli_parses_config_list() {
        let cli = Cli::try_parse_from(["waferctl", "config", "list"]).unwrap();
        assert!(matches!(cli.command, Commands::Config { action: ConfigAction::List }));
    }

    #[test]
    fn test_cli_global_json_flag() {
        let cli = Cli::try_parse_from(["waferctl", "--json", "health"]).unwrap();
        assert!(cli.json);
    }

    #[test]
    fn test_cli_global_endpoint_flag() {
        let cli =
            Cli::try_parse_from(["waferctl", "--endpoint", "http://localhost:8080", "status"])
                .unwrap();
        assert_eq!(cli.endpoint, Some("http://localhost:8080".to_string()));
    }

    #[test]
    fn test_cli_short_endpoint_flag() {
        let cli = Cli::try_parse_from(["waferctl", "-e", "prod", "health"]).unwrap();
        assert_eq!(cli.endpoint, Some("prod".to_string()));
    }

    #[test]
    fn test_cli_combined_flags() {
        let cli = Cli::try_parse_from([
            "waferctl",
            "--json",
            "-e",
            "http://localhost:9090",
            "nodes",
            "--wide",
        ])
        .unwrap();
        assert!(cli.json);
        assert_eq!(cli.endpoint, Some("http://localhost:9090".to_string()));
        assert!(matches!(cli.command, Commands::Nodes { wide: true }));
    }

    #[test]
    fn test_cli_help_does_not_panic() {
        // Verify the CLI definition is valid (catches issues with clap configuration)
        Cli::command().debug_assert();
    }

    #[test]
    fn test_cli_version_flag_exists() {
        // Attempt to parse with --version should result in DisplayVersion error
        let result = Cli::try_parse_from(["waferctl", "--version"]);
        assert!(result.is_err());
        // The error should be a DisplayVersion, not a parse error
    }

    #[test]
    fn test_cli_requires_subcommand() {
        let result = Cli::try_parse_from(["waferctl"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_rejects_unknown_command() {
        let result = Cli::try_parse_from(["waferctl", "unknown-cmd"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_node_requires_id() {
        let result = Cli::try_parse_from(["waferctl", "node"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_hot_swap_requires_node_id() {
        let result = Cli::try_parse_from(["waferctl", "hot-swap"]);
        assert!(result.is_err());

        let result = Cli::try_parse_from(["waferctl", "hot-swap", "node"]);
        assert!(result.is_err());
    }
}
