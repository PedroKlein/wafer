# waferctl Specification

## Purpose
TBD - created by archiving change control-plane. Update Purpose after archive.
## Requirements
### Requirement: Connection configuration

The CLI SHALL support configurable connection to the runtime API.

#### Scenario: Default connection
- **WHEN** waferctl is invoked without connection options
- **THEN** it connects to `http://127.0.0.1:9090`

#### Scenario: Environment variable connection
- **WHEN** `WAFER_API` environment variable is set
- **THEN** waferctl uses that URL as the API endpoint

#### Scenario: CLI flag connection
- **WHEN** `--api <url>` flag is provided
- **THEN** waferctl uses that URL as the API endpoint
- **THEN** CLI flag takes precedence over environment variable

#### Scenario: Connection failure
- **WHEN** waferctl cannot connect to the API
- **THEN** error message indicates connection failure with the attempted URL
- **THEN** exit code is non-zero

### Requirement: Health command

The CLI SHALL provide a health check command.

#### Scenario: Healthy runtime
- **WHEN** `waferctl health` is invoked
- **WHEN** runtime is healthy
- **THEN** output indicates healthy status
- **THEN** exit code is 0

#### Scenario: Unhealthy runtime
- **WHEN** `waferctl health` is invoked
- **WHEN** runtime is unhealthy or unreachable
- **THEN** output indicates unhealthy status with reason
- **THEN** exit code is non-zero

### Requirement: Status command

The CLI SHALL provide a pipeline status command.

#### Scenario: Pipeline running
- **WHEN** `waferctl status` is invoked
- **THEN** output shows pipeline name, status, node counts, and aggregate metrics
- **THEN** human-readable table format by default

#### Scenario: Status JSON output
- **WHEN** `waferctl status --json` is invoked
- **THEN** output is JSON matching the `/api/v1/pipeline` response schema

### Requirement: Resync command

The CLI SHALL provide a config reload and hot-swap trigger command.

#### Scenario: Resync with changes
- **WHEN** `waferctl resync` is invoked
- **WHEN** config file has changed nodes
- **THEN** output lists each changed node and hot-swap result
- **THEN** output includes timing information

#### Scenario: Resync with no changes
- **WHEN** `waferctl resync` is invoked
- **WHEN** config file has not changed
- **THEN** output indicates no changes detected

#### Scenario: Resync with error
- **WHEN** `waferctl resync` is invoked
- **WHEN** config file has errors
- **THEN** output shows error details
- **THEN** exit code is non-zero

### Requirement: Hot-swap command

The CLI SHALL provide a direct hot-swap command.

#### Scenario: Hot-swap with local file
- **WHEN** `waferctl hot-swap <node-id> --wasm <path>` is invoked
- **THEN** hot-swap is triggered for the specified node
- **THEN** output includes swap timing and message metrics

#### Scenario: Hot-swap success
- **WHEN** hot-swap completes successfully
- **THEN** output shows old version, new version, and timing breakdown
- **THEN** exit code is 0

#### Scenario: Hot-swap failure
- **WHEN** hot-swap fails (validation, init, or runtime error)
- **THEN** output shows error reason
- **THEN** exit code is non-zero

### Requirement: Nodes command

The CLI SHALL provide node listing and detail commands.

#### Scenario: List nodes
- **WHEN** `waferctl nodes` is invoked
- **THEN** output is a table with columns: ID, TYPE, STATE, PROCESSED, LAST_MS

#### Scenario: Node details
- **WHEN** `waferctl node <id>` is invoked
- **THEN** output shows full node details including configuration and metrics

#### Scenario: Node not found
- **WHEN** `waferctl node <id>` is invoked for non-existent node
- **THEN** output shows error message
- **THEN** exit code is non-zero

### Requirement: Pipeline control commands

The CLI SHALL provide drain, resume, and stop commands.

#### Scenario: Drain pipeline
- **WHEN** `waferctl drain` is invoked
- **THEN** pipeline begins draining
- **THEN** output confirms drain started

#### Scenario: Resume pipeline
- **WHEN** `waferctl resume` is invoked
- **THEN** pipeline resumes from drained state
- **THEN** output confirms resume completed

#### Scenario: Stop pipeline
- **WHEN** `waferctl stop` is invoked
- **THEN** pipeline begins graceful shutdown
- **THEN** output confirms stop initiated

### Requirement: Metrics command

The CLI SHALL provide a metrics dump command.

#### Scenario: Metrics in Prometheus format
- **WHEN** `waferctl metrics` is invoked
- **THEN** output is Prometheus text format

#### Scenario: Metrics in JSON format
- **WHEN** `waferctl metrics --json` is invoked
- **THEN** output is parsed metrics as JSON object

### Requirement: Output formatting

The CLI SHALL support human-readable and machine-readable output formats.

#### Scenario: Default human output
- **WHEN** commands are invoked without format flags
- **THEN** output is human-readable (tables, formatted text)

#### Scenario: JSON output
- **WHEN** `--json` flag is provided
- **THEN** output is JSON, suitable for scripting with jq

#### Scenario: Quiet output
- **WHEN** `--quiet` or `-q` flag is provided
- **THEN** only errors are printed
- **THEN** exit code indicates success/failure

### Requirement: Error handling

The CLI SHALL provide clear error messages and appropriate exit codes.

#### Scenario: Successful command
- **WHEN** a command completes successfully
- **THEN** exit code is 0

#### Scenario: Client-side error
- **WHEN** a command fails due to user error (bad arguments, file not found)
- **THEN** exit code is 1
- **THEN** error message explains the issue

#### Scenario: Server-side error
- **WHEN** a command fails due to API error
- **THEN** exit code is 2
- **THEN** error message includes API error details

#### Scenario: Connection error
- **WHEN** a command fails due to connection issues
- **THEN** exit code is 3
- **THEN** error message indicates the connection problem

