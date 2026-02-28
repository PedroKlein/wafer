# waferctl Specification

CLI tool for managing WAFER pipeline instances via HTTP API.

## ADDED Requirements

### Requirement: Multi-endpoint configuration

The system SHALL support a configuration file (`~/.config/wafer/config.toml`) with named endpoints and a default endpoint.

#### Scenario: Use default endpoint

- **WHEN** user runs `waferctl status` with no `--endpoint` flag
- **THEN** waferctl uses the default endpoint from config file

#### Scenario: Override with flag

- **WHEN** user runs `waferctl --endpoint pi-gateway status`
- **THEN** waferctl uses the `pi-gateway` endpoint from config file

#### Scenario: Direct URL

- **WHEN** user runs `waferctl --endpoint http://192.168.1.100:9090 status`
- **THEN** waferctl connects directly to that URL (not looked up in config)

#### Scenario: No config file

- **WHEN** config file doesn't exist and no `--endpoint` flag
- **THEN** waferctl defaults to `http://127.0.0.1:9090`

### Requirement: Config management commands

The system SHALL provide commands to manage the endpoint configuration.

#### Scenario: Set endpoint

- **WHEN** user runs `waferctl config set-endpoint pi-gateway http://192.168.1.100:9090`
- **THEN** waferctl adds or updates the endpoint in config file

#### Scenario: Use endpoint as default

- **WHEN** user runs `waferctl config use pi-gateway`
- **THEN** waferctl sets `pi-gateway` as the default endpoint

#### Scenario: List endpoints

- **WHEN** user runs `waferctl config list`
- **THEN** waferctl displays all configured endpoints with default marked

### Requirement: Status command

The system SHALL provide `waferctl status` to show pipeline status.

#### Scenario: Show status

- **WHEN** user runs `waferctl status`
- **THEN** waferctl displays pipeline name, state, uptime, and summary metrics in human-readable format

#### Scenario: JSON output

- **WHEN** user runs `waferctl status --json`
- **THEN** waferctl outputs raw JSON from API

### Requirement: Nodes command

The system SHALL provide `waferctl nodes` to list all nodes.

#### Scenario: List nodes

- **WHEN** user runs `waferctl nodes`
- **THEN** waferctl displays table with columns: ID, TYPE, STATE, PROCESSED, LAST_MS

#### Scenario: Wide output

- **WHEN** user runs `waferctl nodes --wide`
- **THEN** waferctl displays additional columns (queue depth, errors, etc.)

### Requirement: Node detail command

The system SHALL provide `waferctl node <id>` to show details for a specific node.

#### Scenario: Show node

- **WHEN** user runs `waferctl node filter`
- **THEN** waferctl displays detailed information about the filter node

#### Scenario: Node not found

- **WHEN** user runs `waferctl node nonexistent`
- **THEN** waferctl displays error message and exits with code 1

### Requirement: Hot-swap command

The system SHALL provide `waferctl hot-swap <node-id>` to trigger hot-swap.

#### Scenario: Successful hot-swap

- **WHEN** user runs `waferctl hot-swap filter`
- **THEN** waferctl triggers hot-swap, displays progress, and shows timing results on completion

#### Scenario: Hot-swap in progress

- **WHEN** user runs `waferctl hot-swap filter` while another swap is running
- **THEN** waferctl displays error "Another hot-swap is in progress" and exits with code 1

### Requirement: Reload command

The system SHALL provide `waferctl reload` to reload config and hot-swap changed nodes.

#### Scenario: Reload with changes

- **WHEN** user runs `waferctl reload` after modifying config
- **THEN** waferctl displays which nodes were hot-swapped

#### Scenario: Reload no changes

- **WHEN** user runs `waferctl reload` with unchanged config
- **THEN** waferctl displays "No changes detected"

### Requirement: Drain command

The system SHALL provide `waferctl drain` to drain the pipeline.

#### Scenario: Drain pipeline

- **WHEN** user runs `waferctl drain`
- **THEN** waferctl initiates drain and displays "Pipeline drained" on completion

### Requirement: Shutdown command

The system SHALL provide `waferctl shutdown` to gracefully shut down the pipeline.

#### Scenario: Shutdown pipeline

- **WHEN** user runs `waferctl shutdown`
- **THEN** waferctl initiates shutdown and displays "Pipeline shut down" (or connection closed message)

### Requirement: Metrics command

The system SHALL provide `waferctl metrics` to display current metrics.

#### Scenario: Show metrics

- **WHEN** user runs `waferctl metrics`
- **THEN** waferctl displays key metrics in human-readable format

#### Scenario: Raw metrics

- **WHEN** user runs `waferctl metrics --raw`
- **THEN** waferctl outputs raw Prometheus format

### Requirement: Health command

The system SHALL provide `waferctl health` to check runtime health.

#### Scenario: Healthy

- **WHEN** user runs `waferctl health` and runtime is healthy
- **THEN** waferctl displays "Healthy" and exits with code 0

#### Scenario: Unhealthy

- **WHEN** user runs `waferctl health` and runtime is not reachable
- **THEN** waferctl displays error and exits with code 1

### Requirement: Exit codes

The system SHALL use consistent exit codes across all commands.

#### Scenario: Success

- **WHEN** command completes successfully
- **THEN** waferctl exits with code 0

#### Scenario: User error

- **WHEN** command has invalid arguments
- **THEN** waferctl exits with code 1

#### Scenario: API error

- **WHEN** API returns an error response
- **THEN** waferctl exits with code 2

#### Scenario: Connection error

- **WHEN** cannot connect to runtime
- **THEN** waferctl exits with code 3

### Requirement: Output formatting

The system SHALL support `--json` flag on all commands for machine-readable output.

#### Scenario: Default human output

- **WHEN** user runs any command without `--json`
- **THEN** waferctl displays human-readable formatted output (tables, colors if TTY)

#### Scenario: JSON output

- **WHEN** user runs any command with `--json`
- **THEN** waferctl outputs JSON suitable for parsing by scripts
