---
type: decision
created_date: 2026-07-13T16:43:03Z
last_modified: 2026-07-13T16:44:00Z
status: accepted
decided_on: 2026-07-01
deciders: memstead maintainers
scope: org
tags: licensing, open-core, mit, apache, permissive, engine
---

# Dual-license the open engine MIT OR Apache

## Decision
We chose to license the open Memstead engine under dual **MIT OR Apache-2.0** at the user's option — the Rust-ecosystem standard — and to reject both copyleft (AGPL/GPL) and source-available (BSL/SSPL) alternatives. The Claude Code plugin under `plugins/claude-code/` is MIT-only, lighter weight for the plugin-ecosystem case. Anyone, including commercial actors, may embed the engine provided copyright notices stay intact.

## Context
The open-source launch needed a license for the engine crates, CLI, MCP server, schemas, and the `.mem` format/client. The choice is downstream of [[engineering--open-closed-boundary-runs-by-trust-not-engine-capability]]: once the whole engine is open, the only question is *which* permissive terms. Adoption-first posture means the license must not bar enterprise use — many enterprise policies forbid copyleft (AGPL/GPL) and source-available (BSL/SSPL) licenses outright — and users want protection from patent-litigation risk.

## Consequences
- Maximal license acceptance, including by enterprises whose policies forbid copyleft and source-available licenses — the adoption on-ramp stays unobstructed.
- The Apache-2.0 option's patent grant protects downstream users from patent-litigation risk; the MIT option preserves simplicity for those who want it.
- Third parties may embed Memstead in their own products, including commercial ones, keeping notices intact — which is the point, since adoption is the moat.
- Accepted tradeoff: a large distributor could legally bundle the open engine; this residual risk is answered by execution speed and registry/brand ownership, not by the license.
- The dual-license terms propagate to the published artifacts — crates.io packages and the `@memstead/wasm` npm package both carry `MIT OR Apache-2.0`, as recorded on [[engine--binary-release-and-installer-packaging]].

## Relationships
- **REFERENCES**: [[open-closed-boundary-runs-by-trust-not-engine-capability]]
- **REFERENCES**: [[engine:binary-release-and-installer-packaging]]
- **MOTIVATED_BY**: [[open-closed-boundary-runs-by-trust-not-engine-capability]]

## Options

- **Dual MIT OR Apache-2.0** (chosen) — maximally accepted; the Apache option's explicit patent grant covers users where bare MIT/BSD does not, the MIT option keeps maximum simplicity.
- **Bare MIT only** — rejected: lacks the explicit patent grant that the Apache-2.0 option adds; dual gives users that protection without giving up MIT's simplicity.
- **AGPL / GPL (copyleft)** — rejected: taxes adoption, and enterprise policies frequently forbid copyleft outright.
- **BSL / SSPL (source-available)** — considered and rejected: it taxes adoption and trust to protect the wrong thing. The one real residual risk (a large distributor bundling the open engine) is better answered by speed plus owning the registry/ecosystem/brand, not by restricting the engine license.
- **Closed / proprietary or capability-crippled engine** — rejected per the governing principle: the engine was never the moat, and a crippled core would kill the adoption that builds the real moat.

## Notes


