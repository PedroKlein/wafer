.PHONY: build clean generate

# Build the TinyGo uppercase plugin as a WASI P2 Wasm component.
# Requires: tinygo >= 0.34, go 1.24.x, wasm-opt (binaryen), wasm-tools
#
# Note: TinyGo's wasip2 target needs WASI CLI imports at runtime.
# The transform-node-go world extends transform-node with wasi:cli/imports.
# The host runtime already provides WASI — no functional difference.

GO_VERSION := 1.24.13
GO_ROOT := $(shell mise where go $(GO_VERSION) 2>/dev/null)

build:
	PATH="$(GO_ROOT)/bin:$(PATH)" GOROOT="$(GO_ROOT)" \
		tinygo build -target=wasip2 -no-debug \
		-wit-package ./wit -wit-world transform-node-go \
		-o wafer-uppercase-go.wasm .
	@echo "Built: wafer-uppercase-go.wasm ($$(wc -c < wafer-uppercase-go.wasm | tr -d ' ') bytes)"

generate:
	go run go.bytecodealliance.org/cmd/wit-bindgen-go generate ../../../wit/node --out gen --world transform-node

clean:
	rm -f wafer-uppercase-go.wasm
