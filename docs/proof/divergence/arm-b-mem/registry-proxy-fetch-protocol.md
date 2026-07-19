---
type: contract
created_date: 2026-07-15T18:13:52Z
last_modified: 2026-07-15T18:13:52Z
protocol: http
version: 0.1.0-draft
stable_since: 2026-07-04
deprecation_status: draft
tags: package-manager, registry, fetch, phase-5
---

# Registry-Proxy Fetch Protocol

## Summary
The wire protocol between `karac` and a Kāra registry proxy: how the compiler fetches package catalogs, resolves versions, and downloads dependency tarballs over HTTP. Documented in docs/registry-proxy-protocol.md and served by a reference proxy server.

## Relationships
- **PART_OF**: [[package-manager-and-dependency-resolution]]
- **REFERENCES**: [[package-manager-and-dependency-resolution]]

## Request Shape

Catalog + tarball fetch over HTTP. The client (HttpProxyClient) requests a package's catalog (available versions, with client-side yank awareness) and then a specific version's tarball. `--no-proxy` bypasses the proxy for direct-from-source registry fetch; `KARAC_REGISTRY_TOKEN` supplies proxy auth. The `[build].registry-proxy` manifest key pins the proxy endpoint with a defined resolution precedence.

## Response Shape

Package catalog (version list + yank flags) and dependency tarballs; the reference server assembles a store via a `build` subcommand. Diagnostics for proxy fetch are pinned to a stable `--output=json` shape.

## Errors



## Versioning

Pre-1.0 draft. Transport failures are retried with backoff (RetryingProxyClient); a client-side tarball cache (CachingProxyClient) avoids refetch. The lockfile records a proxy-mirror reference so resolved fetches are reproducible.

## Deprecation



## Notes

Realization: src/registry_proxy.rs (orchestration, retry/cache clients, live HttpProxyClient + reference server), src/registry_extract.rs (tarball extraction + registry candidate-set), src/pubgrub_solve.rs (version selection over the candidate set). Consumed by [[package-manager-and-dependency-resolution]]; wire tests in tests/registry_proxy_wire.rs and tests/registry_fetch_e2e.rs.
