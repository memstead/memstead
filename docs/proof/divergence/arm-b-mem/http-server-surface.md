---
type: contract
created_date: 2026-07-15T07:30:33Z
last_modified: 2026-07-15T10:22:58Z
protocol: library_api
version: 0.1.0-pre
stable_since: 2026-07-01
deprecation_status: draft
tags: http, stdlib, runtime
---

# HTTP Server Surface

## Summary
Kāra's std.http surface: a `Server.serve(addr, handler)` server (handler-ABI trampoline, manual dispatch routing, handler-set response headers, keep-alive + chunked, HTTPS via `Server.serve_tls`, HTTP/2 via hyper `auto::Builder`) plus a chained-RequestBuilder client (`Client.get` / `Client.post`). Used by the Parallax benchmark server.

## Relationships
- **REFERENCES**: [[phased-runtime-model]]
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]

## Request Shape

```kara
// user surface
Server.serve(addr, handler)
// handler: a free Kāra fn taking a request and returning a response,
// passed as a value (free-fn-as-value)
```
The address is lifted to the user surface (Server.serve(addr, handler)); the handler is compiled and invoked via an ABI trampoline that avoids intermediate String allocations.

## Response Shape

The handler's return value is serialized as the HTTP response by the runtime server. v1 uses blocking I/O on the worker pool.

## Errors

Runtime-level; the v1 server path is blocking and does not yet expose an async error surface. `KARAC_HTTP_BLOCK_IN_PLACE` toggles the blocking strategy (A/B probe).

## Versioning

Part of std.http (stdlib Slice B). Pre-1.0, evolving with the [[phased-runtime-model]] (v1 blocking → v1.1 event loop).

## Deprecation



## Notes

Realization: runtime/stdlib/http.kara, src/codegen/http.rs (HTTP handler ABI trampoline + client codegen), runtime/src/lib.rs; tests/http_server.rs, tests/http_client_codegen.rs.

Server: manual dispatch is the v1 routing answer; handler-set response headers; verified keep-alive + chunked; `Server.serve_tls` for HTTPS; HTTP/2 via hyper `auto::Builder` (ALPN h2 + h2c); Request.headers()/query() and Response.headers() full-map iteration; `serve_static` is marked `#[unstable]`.

Client: chained `RequestBuilder`, `Client.get`/`Client.post` end-to-end, `Response.text()`/`.bytes()`/`.header(name)`/`.headers()`.

WebSocket surface: RFC 6455 framing, HTTP upgrade handshake (§4.2.1 validation + slowloris/handshake-duration bounds), control + binary frames, fragmentation reassembly + client-side masking (runtime/stdlib/ws.kara). TLS / wss:// via `WebSocket.accept_tls` + rustls server-side FFI. See [[network-runtime-and-cooperative-scheduling]].
