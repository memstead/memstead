---
type: decision
created_date: 2026-08-08T07:07:24Z
last_modified: 2026-08-27T06:55:57Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust bundle plan 02)
scope: subsystem
tags: schemas, builtins, retention, versioning, boot-honesty
---

# Built-in schema versions are append-only and mems stamp their mutating engine version

## Decision
Two guarantees. **Retention:** a `(name, version)` once shipped as a built-in schema exists in every future binary, with byte-identical content. `builtins/MANIFEST.toml` is the append-only ledger of every ever-shipped version with a sealed SHA-256 content hash; `tests/builtin_retention.rs` fails CI on removal, in-place edit, or an unlisted new version — appending the printed `[[shipped]]` block is the whole release ceremony. Behaviour changes are a NEW side-by-side version directory, never an edit. `ingest@0.1.0` is restored to the catalogue. **Stamps:** after any successful mutation, a mem's engine-owned config records which engine version and which resolved schema performed it (`MemConfig.mutationStamp`, written only when the value changes, riding the `__MEMSTEAD` ref so mem-branch cursors never move). Divergence surfaces as the warn-tier `ENGINE_VERSION_SKEW` hint — informative, never fatal; a stamp-less mem is silent, a read-only load writes nothing. **Amended 2026-08-27 (consistency-sweep 04/04):** divergence means a SEMVER difference, not the raw string inequality the first implementation used. Build metadata (`+g<sha>`, `-dirty`) is ignored, so a rebuild between releases is no longer reported as skew — on any workspace whose binary is built from source, which is every dogfood workspace, that was the common case and it drowned the real signal. The hint now carries a direction (the mem was last written by a newer or an older binary), because that is the half that changes what the reader should do. It also fires at WRITE time against the stored stamp, not at boot only: boot-only detection meant the first mutation both revealed the skew and, by restamping, hid it. Never fatal, and never a refusal: a deliberate downgrade is the operator's business.

## Context
On 2026-08-06 the `ingest` built-in was bumped 0.1.0 → 0.2.0 *in place* — the version string edited inside the one existing directory — so 0.1.0 ceased to exist in every binary built afterward, and the sibling plenum workspace, pinned to `ingest@0.1.0`, was down for a day. The diff was byte-identical except for a `cross_mem_relationships` wildcard; exact-pin resolution correctly refused. The bump's migration survey said "no mem anywhere in the project pins ingest" — true inside the repo, false one directory over: built-ins are compiled into every distributed binary, so an in-repo survey clears nothing. Before this decision nothing enforced the retention pattern (`planning` kept versions side-by-side by convention; `ingest` didn't), and no mem recorded what it last ran against.

## Consequences
A rebuild of the binary can never strand a workspace — the failure class is prevented at compile time rather than softened after the damage. Any built-in change's migration survey must assume out-of-repo consumers (handbook release chapter records both rules). The constraint from agent-toolbox plan 07 ("a built-in adopting a new constraint is a version bump, not an in-place edit") is now machine-enforced. The stamp is the substrate any future migration machinery would consult — deliberately built without that machinery; skew never blocks. One new engine-owned config field follows the `sync_state`/`review_mark` precedent; the stamp write is best-effort and steady-state cheap (compare-and-return when unchanged).

## Options

Warning-plus-migration machinery as the primary mechanism was rejected: retention prevents the class for near-zero cost, warnings only soften the landing — the stamp half is kept because it serves honesty beyond this failure. Nearest-compatible-version auto-resolution (0.1.0 → 0.2.0) was rejected as silent semantic drift; exact pins are a correctness feature. Stamping on load rather than mutation was rejected: read paths stay write-free.

## Notes


