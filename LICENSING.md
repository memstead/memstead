# Licensing

This repository contains code under multiple licenses. The open engine is
dual-licensed **MIT OR Apache-2.0** (the Rust-ecosystem standard, at the user's
option); the commercial layer carries different terms.

The open-core cut line is **engine + plugin + `.mem` format/client public;
registry server + macOS app private** — *not* a capability split inside the
engine (the `memstead-git-branch` git-backed backend is open too).

## Per-folder license map

| Path | License | Notes |
|---|---|---|
| `/` (everything not listed below) | MIT OR Apache-2.0 ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE), [NOTICE](NOTICE)) | Engine, CLI, MCP server, schemas, the `.mem` format/protocol + publish/install client, build tooling, docs — the open core. |
| `engine/` | MIT OR Apache-2.0 | The Rust workspace and all open engine crates (incl. `memstead-git-branch`). |
| `docs-site/` | MIT OR Apache-2.0 | Auto-generated Astro site for the public API reference. |
| `inspector/` | MIT OR Apache-2.0 | Developer-facing browser UI. |
| `local-ai/` | MIT OR Apache-2.0 | Wrapper for running skills against a local LLM. |
| `plugins/claude-code/` | MIT ([plugins/claude-code/LICENSE](plugins/claude-code/LICENSE)) | Plugin code that extends Claude Code with memstead-aware skills and hooks. MIT chosen to encourage broad ecosystem use. |
| `registry/` | Proprietary | The registry **server** — the network moat and seed of the commercial layer. Lives outside the open engine workspace (its own private cargo workspace) and depends on the open engine crates by path. The `.mem` format, authority protocol, and publish/install client stay open; the server code does not. |
| `macos/` | Proprietary ([macos/LICENSE](macos/LICENSE)) | The macOS application — commercial product. All rights reserved; not open. |

## Rationale

The project follows an **open-core model**, and the cut runs by *trust*, not by engine capability:

- The **full engine** — the `memstead-git-branch` backend, multi-mem, collaboration, and history — plus the **CLI**, **MCP server**, **schema definitions**, and the **`.mem` format/protocol + publish/install client** are open source under **dual MIT OR Apache-2.0** (the Rust-ecosystem standard; users pick either). There is **no crippled core** — the collaboration story *is* the differentiator, so it must be free to experience. Anyone may embed Memstead into their own products, including commercial ones, keeping copyright notices intact.
- **Plugins** for third-party tools (Claude Code today, others later) are MIT — lighter weight than Apache for the plugin-ecosystem case.
- The **registry server** and the **macOS app** are the commercial layer and stay private. The registry server is the network moat (open-sourcing it would invite a fork that fragments the network); the app is a human oversight surface. Launch posture is **adoption-first** — the open engine drives adoption; revenue layers (a private/enterprise registry, team features) come later, once the graph is embedded in real workflows.

## Why open the whole engine (and not AGPL or BSL)

- **The engine was never the moat.** In the AI age anyone can have a model rebuild a commodity markdown-graph engine from the public `.mem` format + MCP surface + observable behaviour, *regardless of the license* — so closing it buys no real protection while taxing the adoption that builds the actual moat (network + accumulated mems/data + brand).
- **Adoption priority.** Dual MIT/Apache is maximally accepted, including by enterprises whose policies forbid copyleft (AGPL, GPL) and source-available (BSL, SSPL) licenses.
- **No crippled core.** The collaboration backend is open precisely because the collaboration story is what must be experienced for free.
- **Patent grant.** The Apache-2.0 option's explicit patent grant protects users from patent-litigation risk that bare MIT/BSD does not address; the MIT option keeps maximum simplicity.
- **A source-available license (BSL) was considered and rejected** — it taxes adoption and trust to protect the wrong thing. The one real residual risk (a large distributor bundling the open engine) is answered by speed + owning the registry/ecosystem/brand first, not by closing the engine.

## Contributing

This project accepts external contributions, with a few guardrails — see [CONTRIBUTING.md](CONTRIBUTING.md) for how to get set up, run the tests, and open a pull request (discuss large changes first; a test plan is required; no CLA or DCO). Contributions are subject to the same license as the file they modify; pull-request author attribution remains intact.

## Questions

For licensing questions, including commercial licensing of the macOS application or the registry server, or alternative-license requests for the engine, contact hello@memstead.com.
