# WAFER Evaluation Makefile
# Run experiments with: make e-perf-1, make e-perf-4, etc.

WAFER_BIN := ../target/release/wafer-runtime
LOADGEN_BIN := ../target/release/wafer-loadgen
RESULTS := results
TIMESTAMP := $(shell date +%Y%m%d-%H%M%S)

.PHONY: build plugins e-perf-1 e-perf-4 e-perf-7 e-swap-1 e-swap-2 e-iso-7 figures all clean

# === BUILD ===

build:
	cd .. && cargo build --release --workspace

plugins:
	cd ../plugins/pass-through && cargo build --release
	cd ../plugins/uppercase && cargo build --release
	cd ../plugins/threshold-filter && cargo build --release

# === EXPERIMENTS ===

e-perf-1: build plugins
	@echo "=== E-Perf-1: E2E Latency (MQTT, steady 1000 msg/s) ==="
	@mkdir -p $(RESULTS)/e-perf-1/$(TIMESTAMP)
	@cp configs/pipeline-c-passthrough.toml $(RESULTS)/e-perf-1/$(TIMESTAMP)/config.toml
	./scripts/run-experiment.sh configs/pipeline-c-passthrough.toml $(RESULTS)/e-perf-1/$(TIMESTAMP)

e-perf-4: build plugins
	@echo "=== E-Perf-4: Per-Hop Overhead (in-process) ==="
	@mkdir -p $(RESULTS)/e-perf-4/$(TIMESTAMP)
	cd .. && cargo bench --package wafer-core --bench throughput -- wasm_boundary 2>&1 | tee eval/$(RESULTS)/e-perf-4/$(TIMESTAMP)/output.log

e-perf-7: build plugins
	@echo "=== E-Perf-7: Metering Decomposition (4 configs) ==="
	@mkdir -p $(RESULTS)/e-perf-7/$(TIMESTAMP)
	@for cfg in pipeline-c-passthrough pipeline-c-fuel-only pipeline-c-epoch-only pipeline-c-neither; do \
		echo "  Running $$cfg..."; \
		cp configs/$$cfg.toml $(RESULTS)/e-perf-7/$(TIMESTAMP)/; \
	done

e-swap-1: build plugins
	@echo "=== E-Swap-1: Hot-Swap Pause Duration ==="
	@mkdir -p $(RESULTS)/e-swap-1/$(TIMESTAMP)
	cd .. && cargo bench --package wafer-core --bench hot_swap 2>&1 | tee eval/$(RESULTS)/e-swap-1/$(TIMESTAMP)/output.log

e-swap-2: build plugins
	@echo "=== E-Swap-2: Zero-Loss Verification ==="
	@mkdir -p $(RESULTS)/e-swap-2/$(TIMESTAMP)
	@echo "TODO: Run pipeline with hot-swap and verify sequence tracker"

e-iso-7: build plugins
	@echo "=== E-Iso-7: Fault Isolation Throughput Impact ==="
	@mkdir -p $(RESULTS)/e-iso-7/$(TIMESTAMP)
	@echo "TODO: Run parallel-branch topology with attack node"

figures: 
	@echo "=== Generating thesis figures ==="
	cd analysis && uv run jupyter execute notebooks/*.ipynb

all: e-perf-1 e-perf-4 e-perf-7 e-swap-1 e-swap-2 e-iso-7

clean:
	rm -rf $(RESULTS)/*/
