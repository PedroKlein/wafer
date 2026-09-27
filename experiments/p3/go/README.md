# Experimental Go P3 probe

P1-T4 tested the maintained TinyGo workflow before the current exact componentize-go release. The outcome is `go-p3-blocked`; automatic P3 adoption must not proceed without an explicit owner decision.

## Stage verdicts

| Path | Parse/codegen | Compile | Validate | P3 host execution |
| --- | --- | --- | --- | --- |
| TinyGo 0.41.1 / Go 1.24.13 | Pass: TinyGo accepted `wafer:pipeline@0.2.0/transform-message-node` | Pass only as a WASI P2 core module | Blocked: componentization rejects its `wasi:cli/environment@0.2.0` import against the P3 world | Blocked: no P3 component exists |
| componentize-go 0.4.3 at `148dba505f8c6c64ad84db777cfde5e34e25098b` | Pass: generated async Go bindings using wit-bindgen-go 0.61.1 | Fail: its downloaded patched Go 1.27.1 toolchain is rejected as not supporting async | Blocked | Blocked |
| Existing P2 TinyGo uppercase | Existing checked-in bindings | Pass | Pass | Not applicable; all five P2 host-boundary tests pass |

The generated componentize-go surface is retained under `componentize-go/message/`. It is not a maintained WAFER plugin and does not replace `plugins/go/uppercase`.

## Evidence

- `evidence/tinygo-p3-message.txt`
- `evidence/componentize-go-install.txt`
- `evidence/componentize-go-message.txt`
- `evidence/componentize-go-compile.txt`
- `evidence/p2-tinygo-regression.txt`
- `verdict.json`
