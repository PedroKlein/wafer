# ADR-0016: Default-Deny Outbound `wasi:http`

- **Date**: 2026-09-26
- **Status**: Accepted
- **Parent RFC**: [RFC-012](../rfcs/RFC-012-wasi-0.3-evaluation.md)

## Context

WAFER's production Component Model ABI remains Preview 2. A loaded Transform, Filter, or Router may need to call a remote HTTP endpoint, but granting unrestricted network access would let untrusted guest code reach arbitrary Internet, local-network, loopback, or link-local services. Destination checks performed only when configuration is parsed are insufficient: every guest request can choose a different scheme and authority, DNS can resolve a permitted name to a prohibited address, and an HTTP client may follow redirects to a second authority.

This capability is independent of the experimental Preview 3 work. It must fit the existing one-Store-per-node model, survive fresh-Store recovery, and remain unchanged across reconfigure and hot-swap.

## Decision

Add an optional, per-Wasm-node outbound HTTP grant. The grant is default-deny, contains only exact destinations, and is enforced by the host for every outgoing request at the `WasiHttpHooks::send_request` seam.

### Configuration interface

The planned field is `outbound_http` in `[nodes.NAME.capabilities]`:

```toml
[nodes.enrich.capabilities]
outbound_http = [
  { scheme = "https", host = "api.example.com" },
  { scheme = "http", host = "127.0.0.1", port = 8080 },
]
```

Omission and an empty array both grant nothing. The field is valid only for Wasm Transform, Filter, and Router nodes. It is rejected for native processing nodes. It is independent of `inherit_stdio`, `inherit_env`, and `allow_inference`.

A destination has this grammar:

```text
destination := { scheme, host, port? }
scheme      := "http" | "https"
effective-port(http,  omitted) := 80
effective-port(https, omitted) := 443
effective-port(_, explicit)    := explicit u16 in 1..=65535
identity    := (scheme, canonical-host, effective-port)
```

`host` is exactly one of:

- an ASCII DNS name with lowercase comparison, no trailing dot, no empty label, labels containing only letters, digits, and interior hyphens, and no wildcard;
- a canonical IPv4 literal; or
- a canonical IPv6 literal, written without URI brackets in configuration.

Unicode host names, user information, paths, queries, fragments, CIDR blocks, glob syntax, wildcard labels, host suffixes, port ranges, port zero, and unspecified schemes are rejected. Duplicate identities after normalization are rejected rather than silently collapsed.

Each guest request must explicitly provide a scheme. The host hook returns no default scheme. The request's normalized `(scheme, host, effective-port)` must equal one configured identity; matching the host while changing scheme or effective port is denied. Paths, queries, and ordinary methods do not widen destination authority. `CONNECT` is always denied because it can turn an allowed endpoint into a tunnel to another destination.

### DNS and IP policy

The destination name and the connected address are both policy inputs:

| Requested host | Resolution and connection rule |
|---|---|
| DNS name | Resolve once per request. Ignore no records: return `destination-not-found`. Connect only to an address from that exact resolution snapshot. Never perform a second name lookup in the connector. |
| DNS name resolving to loopback, private, link-local, unspecified, multicast, documentation, benchmarking, or otherwise non-global address space | Remove prohibited answers. If no globally routable unicast address remains, return `destination-ip-prohibited`. |
| Exact public IP literal grant | Connect to that literal without DNS. |
| Exact loopback, RFC 1918 IPv4, or IPv6 unique-local literal grant | Allow. The explicit literal is the operator's opt-in and is required for deterministic local tests. |
| Unspecified, multicast, IPv4 broadcast, IPv4/IPv6 link-local, or IPv4-mapped aliases of prohibited addresses | Reject even when written as a literal grant. Cloud metadata addresses such as `169.254.169.254` are therefore never grantable. |
| `localhost` or a DNS alias for a local/private address | Reject through the DNS rule. Use an exact permitted IP literal when local/private access is intentional. |

For HTTPS, certificate verification and SNI use the granted DNS name, while the TCP connection uses the already selected address. HTTPS IP-literal grants require a certificate valid for that IP. This prevents a policy check followed by a second DNS lookup.

### Redirects and repeated requests

The host connector does not follow redirects. A `3xx` response is returned to the guest. If the guest constructs another request from `Location`, that request passes through the same policy check as any other request. Redirects therefore cannot inherit or expand the authority of the original request.

### Enforcement seam and errors

The host always links the P2 `wasi:http` interfaces, including for an empty grant, so an HTTP-capable component can instantiate and receive a request-level denial instead of an import-resolution failure. Linking does not grant network authority.

`WasiHttpHooks::send_request` receives the final assembled `http::Request<WasiBody>` immediately before network I/O. The policy implementation validates method, scheme, authority, effective port, DNS results, and selected socket address there. Checks only in config validation, `is_supported_scheme`, or guest input are insufficient.

Policy denial returns `wasmtime_wasi_http::Error::HttpRequestDenied`, exposed to the guest as `wasi:http/types.error-code::http-request-denied`. A DNS name with no records maps to `destination-not-found`; a resolution with no permitted address maps to `destination-ip-prohibited`; transport and TLS failures retain their standard wasi:http error categories. Policy failures do not trap the Store.

Host logs may include node id and normalized destination identity. They must not include request or response bodies, query strings, authorization values, cookies, or arbitrary headers. WAFER does not inject credentials, inherit proxy settings, or expose host secrets through this capability. Managed credentials remain the responsibility of native Sources and Sinks.

### Lifecycle invariants

The normalized destination set is immutable for the lifetime of a loaded node:

| Transition | Required grant behavior |
|---|---|
| Initial launch | Parse, validate, normalize, and freeze the configured set before Store creation. |
| Guest call | Read the frozen set; never mutate it from guest state. |
| Trap or timeout recovery | Rebuild `WasiHttpCtx` and policy hooks from the node's exact frozen set. |
| Reconfigure | Reconfigure changes only guest lifecycle configuration. The host grant remains byte-for-byte equivalent. |
| Hot-swap preparation and adoption | Prepare the replacement with the currently loaded node's grant. The request body cannot supply or expand destinations. |
| Failed reconfigure or hot-swap | Continue with the prior Store and the same grant. |
| Process-time rollback | Rebuild the rollback Store with the same grant. |
| Pipeline restart | A changed static configuration may establish a new grant after normal validation. This is the only authority-change path. |

The runtime representation should be an immutable normalized collection shared or cloned into fresh Stores. It must not be derived from guest lifecycle JSON, environment variables, request headers, or component metadata.

### Pinned Wasmtime integration

All Wasmtime crates remain pinned to revision `e9f1ea232fd245aea338ab3eb7d73487ae75cab1`. The dependency is:

```toml
wasmtime-wasi-http = {
  git = "https://github.com/bytecodealliance/wasmtime",
  rev = "e9f1ea232fd245aea338ab3eb7d73487ae75cab1",
  default-features = false,
  features = ["p2"],
}
```

`default-send-request` stays disabled because its connector performs its own authority DNS lookup after the policy hook; WAFER requires one policy-controlled resolution and connection snapshot. The implementation uses the existing Hyper stack plus direct `rustls`, `tokio-rustls`, and `webpki-roots` dependencies for the policy-controlled TCP/TLS connection.

`WaferState` will own `WasiHttpCtx` and a policy hook alongside its existing `WasiCtx` and shared `ResourceTable`, then implement `WasiHttpView`. Since WAFER already calls `wasmtime_wasi::p2::add_to_linker_async`, linker construction must add only the HTTP interfaces:

```rust
wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
```

Using `add_to_linker_async` would re-add proxy interfaces already supplied by `wasmtime-wasi`. No sync or P3 linker is introduced.

Pinned source:

- [`add_only_http_to_linker_async`](https://github.com/bytecodealliance/wasmtime/blob/e9f1ea232fd245aea338ab3eb7d73487ae75cab1/crates/wasi-http/src/p2/mod.rs#L306-L320)
- [`WasiHttpView` and `WasiHttpCtxView`](https://github.com/bytecodealliance/wasmtime/blob/e9f1ea232fd245aea338ab3eb7d73487ae75cab1/crates/wasi-http/src/ctx.rs#L86-L110)
- [`WasiHttpHooks::send_request`](https://github.com/bytecodealliance/wasmtime/blob/e9f1ea232fd245aea338ab3eb7d73487ae75cab1/crates/wasi-http/src/ctx.rs#L238-L306)
- [`HttpRequestDenied` P2 mapping](https://github.com/bytecodealliance/wasmtime/blob/e9f1ea232fd245aea338ab3eb7d73487ae75cab1/crates/wasi-http/src/p2/error.rs#L318-L340)

The compile probe in the P2-T1 evidence bundle proves this dependency feature set, `WasiHttpView`, custom hooks with default sending disabled, shared `ResourceTable`, existing async WASI linker, and HTTP-only async linker compile together.

## Executable verification matrix

P2-T2 and P2-T3 must implement these tests against real P2 components and controlled local servers. A missing fixture or skipped row is a failure.

| ID | Setup and request | Required observation |
|---|---|---|
| H01 | Omit `outbound_http`; request `http://127.0.0.1:<server>` | `http-request-denied`; server receives nothing. |
| H02 | Configure `outbound_http = []`; make the same request | Same denial as omission. |
| H03 | Grant exact `http`, `127.0.0.1`, server port | Request succeeds and only that server receives it. |
| H04 | Change only scheme | Denied before DNS/connect. |
| H05 | Change only host | Denied before DNS/connect. |
| H06 | Change only explicit/effective port | Denied before DNS/connect. |
| H07 | Omit request scheme | `http-protocol-error`; no implicit HTTPS authority. |
| H08 | Use wildcard, suffix, CIDR, path, userinfo, Unicode host, malformed label, duplicate normalized destination, or port zero in config | Static validation rejects the node and identifies the destination index without echoing secrets. |
| H09 | Exact IP-literal loopback grant | Allowed, proving intentional local/private access. |
| H10 | DNS name resolves only to loopback/private/link-local | `destination-ip-prohibited`; server receives nothing. |
| H11 | DNS name resolves to mixed public and prohibited answers | Connector uses only an address from the permitted resolution subset; no second lookup occurs. |
| H12 | Literal unspecified, multicast, broadcast, link-local, or IPv4-mapped prohibited address | Static validation rejects it. |
| H13 | Allowed endpoint returns redirect to ungranted endpoint | Guest receives `3xx`; ungranted endpoint receives nothing. A guest-issued follow-up is denied independently. |
| H14 | Guest uses `CONNECT` | `http-request-denied`; no connection occurs. |
| H15 | Allowed request includes body/query/header data | Request succeeds; logs contain node id and destination only, not body, query, cookies, or authorization values. |
| H16 | First request traps, then recovery retries an allowed and a denied destination | Fresh Store keeps exactly the original grant; allowed succeeds and denied remains denied. |
| H17 | Reconfigure guest config while attempting to include destinations in lifecycle JSON | Grant remains unchanged; lifecycle JSON cannot add authority. |
| H18 | Hot-swap to a component that requests an ungranted destination | Replacement may load, but request is denied under the original grant. |
| H19 | Accepted hot-swap and process-time rollback | Replacement and rollback Stores both retain the exact original grant. |
| H20 | Enable inference and outbound HTTP together | Both explicit grants survive recovery without enabling any unconfigured capability. |

## Native transport boundary

Native Sources and Sinks remain WAFER's default transport modules and the owners of broker credentials, TLS client identity, protocol retries, long-lived connections, and ingress/egress lifecycle. Outbound wasi:http is an opt-in capability for a processing node to make a bounded request while processing; it does not add Wasm Source or Sink categories, guest MQTT, inbound HTTP, host credential injection, automatic retries, redirect following, pooling policy, or a generic networking interface.

Prefer a native Source or Sink when HTTP is the pipeline boundary or when credentials, retries, backpressure, connection reuse, or delivery semantics matter. Use outbound wasi:http only when request/response behavior is intrinsic to a Transform, Filter, or Router.

## Consequences

### Positive

- Network authority is explicit, immutable, and reviewed per node.
- Omitted configuration remains safely usable with deterministic request denial.
- DNS rebinding and redirect authority expansion are addressed at the actual network seam.
- The capability composes with the retained Preview 2 ABI and async Store model.

### Negative

- A policy-controlled DNS/TCP/TLS connector is required; the upstream default sender cannot satisfy the single-resolution invariant.
- DNS names resolving only to local or private addresses require an explicit IP-literal grant instead.
- There is no first-version wildcard, CIDR, redirect, proxy, credential injection, or connection-pool configuration.

### Neutral

- Static configuration changes may alter grants only after a full pipeline restart.
- Existing components continue to see the same WIT worlds and capabilities unless they import and call wasi:http.
- Preview 3 remains a separate experiment.
