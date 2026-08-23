---
type: decision
created_date: 2026-08-23T13:43:26Z
last_modified: 2026-08-23T14:48:18Z
status: accepted
decided_on: 2026-08-23
deciders: operator, implementing agent
scope: system
tags: health, strict, mounts, gates, cli
---

# health --strict refuses configuration defects, and a mount that resolves to nothing says so

## Decision
We will make `memstead health --strict` refuse on the workspace describing itself wrongly, with no include needed: `SCHEMA_PIN_MISMATCH` (a mount's expectation disagrees with the mem's own config pin), `SCHEMA_UNSTAMPED_SOURCE_ROT` (a pinned schema's sealed package no longer passes authoring validation) and the new `MOUNT_UNBACKED`; and on the consistency findings `DANGLING_LINK` and `ORPHAN_STUB` when `integrity` is included. Stale entities, drifted anchors and `SCHEMA_GENERATIONS_BEHIND` stay advisory: a stale entity is a fact about time, a drifted anchor is the sync loop's input, and a generations-behind pin keeps working. `MOUNT_UNBACKED` is a boot and reload warning for a mount that resolves to nothing, with `details.reason` in `missing_ref` (a git-branch mount whose branch was never created or was deleted), `missing_path` (a folder or archive mount whose path is gone) and `empty` (storage that exists and holds no entity); a mount serving at least one entity is silent. The backend trait gained `storage_present()` so boot can tell "never created" from "empty": `list_entities` folds a missing branch into an empty list and cannot. The nested-prefix warning says what it can tell instead of guessing: "target missing in mem X" when the link's prefix is a mounted mem, "prefix 'X' is not a mounted mem" when it only matches a mem name's last segment.

## Context
On the 0.10.0 tree `health --strict` participated five axes (`missing_required_outgoing`, `constraints` with the format defects, warn-level `signals`, `schema_authoring_drift`) and exited 0 on the dogfood workspace with two mounts pointing at branches that did not exist (`institute`, `exec-launch-claims`, listed as writable with zero entities and no warning), three pin mismatches, two rotted schema packages, seven stubs and fourteen dangling-link findings. Every one of those was a rendered warning or an include-gated finding, none a refusal, so no runner could fail on them and the graph had no referee. The nested-prefix warning called all eight of its hits "almost certainly mem-rename drift"; all eight were missing targets in mounted mems. The bundle this belongs to repairs the graph next and needs a gate that bites before the repair, recorded against the unrepaired state.

## Consequences
- A CI or local strict run is red on configuration defects without anyone remembering an include; the dogfood referee (`scripts/graph-health.sh` in the workspace repo) builds on it.
- An empty mem warns. A freshly created mem with no entity is `MOUNT_UNBACKED`/`empty` until its first write; strict runs on such a workspace refuse, which is the point for a referee and a surprise for a scratch workspace. Two engine tests that asserted "no warnings on a clean empty mount" now expect exactly that one warning.
- Lazy mounts are judged on storage presence only at boot (the entity walk is deferred), so `empty` is reported for eager mounts.
- `details` of the nested-prefix warning gained `target_mem` and `prefix_mounted`; the message changed, the code did not.
- The CLI's `--strict` help and the generated reference name the set; the MCP instructions name the new boot warning. No MCP strict mode exists, and none is added: refusal is the CLI's job, the MCP surface reports.
- Cost: one more ref or directory probe per mount at boot and reload (metadata only).

## Relationships
- **INFORMED_BY**: [[built-in-schema-versions-are-append-only-and-mems-stamp-their-mutating-engine-version]]

## Options

- Make stale entities and drifted anchors strict-failing too: rejected. Advisory by nature; the runner asserts an anchor ceiling for the one outward-facing mem instead.
- Keep `MOUNT_UNBACKED` to the two missing-storage reasons and let an empty mount pass: rejected. A mount that serves nothing is the same silence for every consumer; the reason field tells the cases apart.
- Detect the unbacked mount by counting entities after the walk, without a backend probe: rejected. It cannot distinguish a deleted branch from an empty one, and the repair differs (restore versus author).
- Refuse on `SCHEMA_GENERATIONS_BEHIND`: rejected. The pin works; refusing would make every built-in schema release a red run for every user.
- Always-on configuration refusals plus include-gated consistency refusals, the unbacked-mount warning with three reasons, an honest nested-prefix message: chosen.

## Notes

Fixtures: one CLI test per strict class and the advisory complement (`crates/memstead-cli/tests/health_strict_config.rs`), the probe's three reasons on the folder backend, `storage_present` on the git backend, both nested-prefix classes. The pre-repair state of the dogfood workspace is recorded as the workspace repo's pre-repair fixture.
