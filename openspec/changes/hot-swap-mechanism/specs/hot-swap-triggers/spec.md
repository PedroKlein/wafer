# Hot-Swap Triggers

Mechanisms that initiate hot-swap operations, starting with config file watching.

**SPEC Reference:** Section 7.3, 9.2

## ADDED Requirements

### Requirement: Config file watch trigger

The runtime SHALL watch the pipeline configuration file for changes and trigger hot-swap when a node's WASM path changes.

#### Scenario: WASM path change detected
- **WHEN** the pipeline config file is modified
- **WHEN** a node's `wasm` path changes from `v1.wasm` to `v2.wasm`
- **THEN** hot-swap is triggered for that node with the new WASM component

#### Scenario: Non-WASM config change
- **WHEN** the pipeline config file is modified
- **WHEN** only non-WASM fields change (e.g., queue capacity)
- **THEN** no hot-swap is triggered
- **THEN** a warning is logged that config reload requires restart

### Requirement: File watch debouncing

The file watcher SHALL debounce rapid changes to prevent spurious hot-swap triggers.

#### Scenario: Multiple rapid file saves
- **WHEN** the config file is saved multiple times within 500ms
- **THEN** only one hot-swap is triggered after the debounce window

#### Scenario: Changes after debounce window
- **WHEN** the config file is saved
- **WHEN** 500ms passes
- **WHEN** the config file is saved again
- **THEN** two separate hot-swaps are triggered

### Requirement: Config validation before swap

The runtime SHALL validate the new configuration before triggering any hot-swaps.

#### Scenario: Valid config change
- **WHEN** the config file changes to a valid configuration
- **THEN** config is parsed and validated
- **THEN** hot-swap proceeds for changed nodes

#### Scenario: Invalid config syntax
- **WHEN** the config file changes to invalid TOML syntax
- **THEN** no hot-swap is triggered
- **THEN** an error is logged with parse failure details
- **THEN** the pipeline continues with the previous configuration

#### Scenario: Invalid config semantics
- **WHEN** the config file has valid syntax but invalid semantics (e.g., missing required field)
- **THEN** no hot-swap is triggered
- **THEN** an error is logged with validation failure details
- **THEN** the pipeline continues with the previous configuration

### Requirement: Swap-in-progress lock

The runtime SHALL prevent concurrent hot-swap operations on the same node.

#### Scenario: Hot-swap already in progress
- **WHEN** hot-swap is triggered for a node
- **WHEN** another hot-swap is requested for the same node before the first completes
- **THEN** the second request is rejected
- **THEN** a warning is logged indicating swap already in progress
