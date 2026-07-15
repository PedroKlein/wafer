.PHONY: build clean

# Build the Python threshold-filter plugin as a Wasm component.
# Requires: componentize-py >= 0.14, Python 3.12+
#
# Install: pip install componentize-py
#
# The WIT deps directory includes WASI interfaces needed by the Python runtime.

WIT_DIR := ../../../wit/node

build:
	componentize-py -d $(WIT_DIR) -w filter-node componentize app -o wafer-threshold-filter-py.wasm
	@echo "Built: wafer-threshold-filter-py.wasm ($$(wc -c < wafer-threshold-filter-py.wasm | tr -d ' ') bytes)"

clean:
	rm -f wafer-threshold-filter-py.wasm
