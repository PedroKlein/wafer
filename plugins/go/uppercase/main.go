// Package main implements the WAFER uppercase transform plugin in Go.
//
// Demonstrates polyglot interop: same WIT contract as the Rust plugin,
// different source language, same pipeline. Built with TinyGo to wasm32-wasip2.
package main

import (
	"strings"

	"go.bytecodealliance.org/cm"

	"github.com/PedroKlein/wafer/plugins/go/uppercase/gen/wafer/pipeline/lifecycle"
	"github.com/PedroKlein/wafer/plugins/go/uppercase/gen/wafer/pipeline/transform"
	"github.com/PedroKlein/wafer/plugins/go/uppercase/gen/wafer/pipeline/types"
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

	// Every process() call must release the input Payload borrow before
	// returning. See borrow_shim.go for the "why" and the exact conditions
	// under which this call must be removed.
	defer releaseInputBorrow(input.Payload)

	payload := input.Payload.ReadAll()
	bytes := payload.Slice()

	metadata := append([][2]string(nil), input.Metadata.Slice()...)
	metadata = append(metadata, [2]string{"plugin", "go-uppercase"})
	uppercased := strings.ToUpper(string(bytes))

	output := types.OutputMessage{
		ID:          "guest-" + input.ID,
		Timestamp:   input.Timestamp + 7,
		Source:      "guest-" + input.Source,
		ContentType: "text/uppercase",
		Metadata:    cm.ToList(metadata),
		Payload:     cm.ToList([]uint8(uppercased)),
	}
	result.SetOK(output)
	return result
}

func main() {}
