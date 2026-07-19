---
type: memo
created_date: 2026-07-15T07:25:28Z
last_modified: 2026-07-15T07:25:28Z
status: closed
tags: observation, lesson, performance, http
---

# HTTP Layer Performance Investigation

## Claim
The Kāra HTTP server's throughput was limited by per-request overhead in the handler trampoline and by a blocking-strategy choice; the investigation cut intermediate String allocations and A/B-tested an in-place blocking mode.

## Context
- Opened alongside the Parallax work (docs/investigations/http_layer_perf.md) to separate HTTP-layer cost from the general codegen/runtime cost tracked in [[parallax-performance-investigation]].
- The v1 runtime uses blocking I/O (see [[phased-runtime-model]]), so the HTTP path's per-call cost is directly on the hot path.

## Relationships
- **REFERENCES**: [[parallax-performance-investigation]]
- **REFERENCES**: [[phased-runtime-model]]

## Substance

- H2 step 1: killed intermediate String allocations in the handler ABI trampoline that bridges the generated Kāra handler to the runtime server.
- H1 probe: added `KARAC_HTTP_BLOCK_IN_PLACE` as an env-var A/B to compare blocking the accepting thread in place vs handing off.
- Benchmark harness hardened separately (connection sweeps, multi-run stats, percentile distributions, cold-start vs steady-state) so the numbers are trustworthy.

## Alternatives



## Outcome

- Trampoline allocation cuts landed; blocking-mode probe available behind an env var.
- Fed the broader lesson that bench robustness (warmup, percentiles, multiple runs) must precede optimization claims.
