// Package main implements the WAFER uppercase transform plugin in Go.
//
// Demonstrates polyglot interop: same WIT contract as the Rust plugin,
// different source language, same pipeline. Built with TinyGo to wasm32-wasip2.
package main

import (
	"strings"

	"go.bytecodealliance.org/cm"

	"github.com/PedroKlein/wafer-poc/plugins/go/uppercase/gen/pipeline/node/lifecycle"
	"github.com/PedroKlein/wafer-poc/plugins/go/uppercase/gen/pipeline/node/transform"
	"github.com/PedroKlein/wafer-poc/plugins/go/uppercase/gen/pipeline/types/types"
)

func init() {
	lifecycle.Exports.Validate = validate
	lifecycle.Exports.Init = initNode
	lifecycle.Exports.Close = closeNode
	transform.Exports.Process = process
}

func validate(_ lifecycle.NodeConfig) cm.Option[string] {
	return cm.None[string]()
}

func initNode(_ lifecycle.NodeConfig) cm.Result[lifecycle.ProcessError, struct{}, lifecycle.ProcessError] {
	var result cm.Result[lifecycle.ProcessError, struct{}, lifecycle.ProcessError]
	result.SetOK(struct{}{})
	return result
}

func closeNode() {}

func process(input transform.Message) cm.Result[transform.OutputMessageShape, transform.OutputMessage, transform.ProcessError] {
	var result cm.Result[transform.OutputMessageShape, transform.OutputMessage, transform.ProcessError]

	// Read payload from host buffer
	payload := input.Payload.ReadAll()
	bytes := payload.Slice()

	// Empty payload: pass through
	if len(bytes) == 0 {
		output := types.OutputMessage{
			ID:          input.ID,
			Timestamp:   input.Timestamp,
			Source:      input.Source,
			ContentType: input.ContentType,
			Metadata:    input.Metadata,
			Payload:     cm.ToList([]uint8{}),
		}
		result.SetOK(output)
		return result
	}

	// Convert to uppercase (ASCII)
	text := string(bytes)
	uppercased := strings.ToUpper(text)

	output := types.OutputMessage{
		ID:          input.ID,
		Timestamp:   input.Timestamp,
		Source:      input.Source,
		ContentType: input.ContentType,
		Metadata:    input.Metadata,
		Payload:     cm.ToList([]uint8(uppercased)),
	}
	result.SetOK(output)
	return result
}

func main() {}
