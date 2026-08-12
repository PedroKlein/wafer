# Security

WAFER is a research prototype produced for an undergraduate thesis.
It is **not a production runtime**. The code has not undergone a
security review, has no supported release schedule, and should not be
placed on the public internet without wrapping it in a hardened
transport layer.

## Threat model in scope

The runtime is designed around one active threat: an untrusted Wasm
Component plugin loaded into the pipeline. Plugins run in a wasmtime
Store with fuel and epoch limits, no ambient WASI capabilities, and
no shared linear memory. Failures in that model — for example, a
plugin that escapes its sandbox, drains fuel without accounting, or
crashes the host through the WIT boundary — are treated as security
issues.

## Not in scope

- Denial of service against the HTTP control plane. It binds to
  `127.0.0.1` by default and expects an operator-provided reverse
  proxy in front of it.
- Confidentiality of MQTT payloads. Transport encryption is the
  broker's responsibility.
- Supply-chain attacks on plugin OCI registries. Plugin authenticity
  is delegated to the registry the operator chooses.

## Reporting a vulnerability

If you believe you have found a security issue, please open a private
GitHub Security Advisory on this repository, or email
`pedroklein1@hotmail.com` with `[wafer-security]` in the subject.
Please do not file a public issue for anything that looks like a
sandbox escape until it has been triaged.

You can expect a first response within two weeks. Because this is a
thesis artefact, patch turnaround will depend on academic deadlines,
and there is no guarantee of a fix. If the issue is severe, a public
advisory will be published on the repository regardless.
