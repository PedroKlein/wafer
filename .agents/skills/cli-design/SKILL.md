---
name: cli-design
description: >
  CLI design patterns for waferctl using clap derive API. Covers dual-consumer error
  messages (human-readable + machine-parseable for automation), structured JSON output
  with RFC 9457 Problem Details, endpoint configuration, output formatting with tabled,
  and interactive vs scripted UX. Use when adding CLI commands, formatting output,
  handling CLI errors, designing subcommand interfaces, or making waferctl agent-friendly.
  Triggers on: waferctl, CLI, clap, subcommand, Parser, Subcommand, tabled, --json,
  output format, command, flag, arg, endpoint, exit code, error message, command-line.
  Do NOT use for HTTP API design (that's the axum server side) or general Rust patterns
  (use rust-best-practices).
---

# CLI Design (waferctl)

## Architecture

waferctl is a thin HTTP client that talks to the WAFER runtime's REST API:
```
waferctl [--json] [--endpoint name|url] <command> [args]
         ↓
    HTTP → wafer runtime (axum API)
```

---

## The Dual-Consumer Problem (2026)

CLI error messages have two audiences in 2026:
1. **Humans** reading terminal output
2. **Agents/scripts** parsing output for automated retry/recovery

A CLI built in 2026 that does not emit structured errors is leaving reliability on
the table. (zircote, "CLI Error Messages Are a Dual-Consumer Problem")

### Human Output (default)

```
Error: Cannot connect to runtime at http://localhost:8080
  Is the pipeline running? Try: wafer --config pipeline.toml
```

### Machine Output (--json)

Follow RFC 9457 Problem Details + extension fields:
```json
{
  "type": "urn:wafer:error:connection-refused",
  "title": "Cannot connect to runtime",
  "status": 1,
  "detail": "TCP connection refused at http://localhost:8080",
  "instance": "/health",
  "suggested_fix": "Start the pipeline: wafer --config pipeline.toml",
  "retry_after": null,
  "docs_url": "https://github.com/PedroKlein/wafer#quickstart"
}
```

Key extension fields:
- `suggested_fix`: free-text recovery action (agent can attempt automatically)
- `retry_after`: seconds to wait before retrying (null = don't retry)
- `docs_url`: link to relevant documentation

---

## Clap Derive Structure

```rust
#[derive(Parser)]
#[command(name = "waferctl", version, about)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[arg(short, long, global = true)]
    endpoint: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Health,
    Status,
    Nodes { #[arg(short, long)] wide: bool },
    Node { id: String },
    HotSwap { node_id: String },
    Reload,
    Drain,
    Shutdown,
    Metrics { #[arg(long)] raw: bool },
    Config { #[command(subcommand)] action: ConfigAction },
}
```

**Design decisions**:
- `global = true` on `--json` and `--endpoint` — available on ALL subcommands
- Doc comments become help text — write for the user, not the developer
- Verb-first commands (`health`, `status`, `hot-swap`) — not `pipeline status node list`

---

## Output Formatting

### Dual-Mode: Human Tables + Machine JSON

```rust
fn output<T: Serialize + Tabled>(data: &[T], json: bool) {
    if json {
        // stdout = structured data ONLY (pipeable to jq)
        println!("{}", serde_json::to_string_pretty(data).unwrap());
    } else {
        // Human-readable with nice formatting
        let table = Table::new(data).with(Style::rounded()).to_string();
        println!("{table}");
    }
}
```

**Stdout vs stderr separation**:
- `stdout`: data output (tables, JSON) — this is what `| jq` and `| grep` consume
- `stderr`: progress, status, confirmations, errors — never pollutes data stream

### Table Design with tabled

```rust
#[derive(Tabled, Serialize)]
struct NodeRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "TYPE")]
    node_type: String,
    #[tabled(rename = "STATE")]
    state: String,
    #[tabled(rename = "MSG/s")]
    throughput: String,
}
```

---

## Error Handling

### Exit Codes (Semantic, Not Random)

```rust
pub fn exit_code(err: &CliError) -> i32 {
    match err {
        CliError::ConnectionFailed(_) => 1,   // Runtime unreachable
        CliError::NotFound(_) => 2,           // Resource doesn't exist
        CliError::Conflict(_) => 3,           // Swap already in progress
        CliError::InvalidInput(_) => 4,       // Bad arguments
        CliError::Internal(_) => 5,           // Server error
        CliError::Timeout(_) => 6,            // Operation timed out
    }
}
```

### Error Presentation

```rust
// BAD — internal error details leak; useless to humans; unparseable by scripts
eprintln!("Error: reqwest::Error {{ kind: Connect, url: ... }}");

// GOOD — actionable message + context
eprintln!("Error: Cannot connect to runtime at {endpoint}");
eprintln!("  Hint: Is the pipeline running? Try: wafer --config pipeline.toml");
eprintln!("  Endpoint resolved from: {source}");  // "~/.config/wafer/endpoints.toml"
```

### Threading Global Flags Through Commands

```rust
// The common pattern for passing Cli-level config to handlers:
async fn run() -> Result<(), CliError> {
    let cli = Cli::parse();
    let client = WaferClient::from_endpoint(cli.endpoint.as_deref())?;
    
    match cli.command {
        Commands::Status => cmd_status(&client, cli.json).await,
        Commands::HotSwap { node_id } => cmd_hotswap(&client, &node_id, cli.json).await,
        // ...
    }
}
```

---

## Endpoint Configuration

Stored at `~/.config/wafer/endpoints.toml`:
```toml
default = "local"

[endpoints.local]
url = "http://localhost:8080"

[endpoints.raspi]
url = "https://192.168.1.100:8080"
```

Resolution order (first match wins):
1. `--endpoint http://...` — URL passed directly
2. `--endpoint raspi` — name lookup in config
3. Neither — use `default` from config
4. No config file — fallback to `http://localhost:8080`

---

## Interactive vs Scripted UX

### Destructive Operations: Confirm Unless Scripted

```rust
Commands::Shutdown => {
    if !cli.json && std::io::stdout().is_terminal() {
        eprint!("Shutdown pipeline at {endpoint}? [y/N] ");
        // ...confirmation logic...
    }
    // In --json mode or non-TTY: proceed without confirmation
    client.shutdown().await?;
}
```

### Progress for Long Operations

```rust
Commands::HotSwap { node_id } => {
    if !cli.json {
        eprintln!("Hot-swapping node '{node_id}'...");  // stderr = progress
    }
    let metrics = client.hot_swap(&node_id).await?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&metrics)?);  // stdout = data
    } else {
        eprintln!("✓ Complete in {}ms (drain: {}ms, flip: {}ms)",
            metrics.total_ms, metrics.drain_ms, metrics.flip_ms);
    }
}
```

---

## NEVER

- **NEVER print machine-parseable output to stderr** — `stdout` is for data (tables, JSON);
  `stderr` is for humans (progress, errors); `cmd | jq` must work with only stdout
- **NEVER require interactive confirmation in --json mode** — scripts assume non-interactive;
  if `--json` is set or stdout is not a TTY, skip all prompts
- **NEVER change JSON field names without versioning** — scripts depend on field names;
  additions are backwards-compatible; removals and renames break consumers
- **NEVER expose internal error types to users** — "reqwest::Error { kind: Connect }" is
  useless; always translate to actionable messages with suggested recovery
- **NEVER omit the endpoint URL from connection errors** — "connection refused" without
  context is undebuggable; always show which endpoint was tried and how it was resolved
- **NEVER emit ANSI color codes to non-TTY output** — piped output becomes garbled;
  check `std::io::stdout().is_terminal()` before coloring
- **NEVER treat errors as just strings** — structure them with type URIs, exit codes,
  and `suggested_fix`; in 2026 your CLI's consumers include AI agents that can parse
  structured diagnostics and retry autonomously
