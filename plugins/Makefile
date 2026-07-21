# plugins/Makefile — build all Wasm plugins
# Temporary: will migrate to mise task runner.
.PHONY: all release clean test check

PLUGINS := pass-through uppercase json-parse threshold-filter content-router \
           tensor-prep result-format anomaly-detector cayenne-decoder \
           vibration-features quality-rules delay-injector
ATTACKS := attacks/buffer-overflow attacks/cross-read attacks/infinite-loop \
           attacks/memory-exhaust attacks/fs-access attacks/panic

all:
	@for p in $(PLUGINS) $(ATTACKS); do \
		echo "Building $$p..."; \
		(cd $$p && cargo build) || exit 1; \
	done

release:
	@for p in $(PLUGINS) $(ATTACKS); do \
		echo "Building $$p (release)..."; \
		(cd $$p && cargo build --release) || exit 1; \
	done
	@# Post-process: 10-30% size reduction (requires wasm-opt from binaryen)
	@if command -v wasm-opt >/dev/null 2>&1; then \
		find . -path "*/wasm32-wasip2/release/*.wasm" -exec sh -c \
			'echo "Optimizing $$(basename {})" && wasm-opt -Os {} -o {}' \;; \
	else \
		echo "wasm-opt not found, skipping post-processing"; \
	fi

test:
	@for p in $(PLUGINS); do \
		echo "Testing $$p..."; \
		(cd $$p && cargo test --target $$(rustc -vV | grep host | awk '{print $$2}')) || exit 1; \
	done

check:
	@for p in $(PLUGINS) $(ATTACKS); do \
		echo "Checking $$p..."; \
		(cd $$p && cargo check) || exit 1; \
	done

clean:
	@for p in $(PLUGINS) $(ATTACKS); do \
		(cd $$p && cargo clean); \
	done
