---
type: decision
created_date: 2026-08-23T13:43:26Z
last_modified: 2026-08-27T14:41:58Z
status: accepted
decided_on: 2026-08-23
deciders: operator, implementing agent
scope: system
tags: health, strict, mounts, gates, cli
---

# health --strict refuses configuration defects, and a mount that resolves to nothing says so

## Decision
We will make `memstead health --strict` refuse on the workspace describing itself wrongly, with no include needed: `SCHEMA_PIN_MISMATCH` (a mount's expectation disagrees with the mem's own config pin), `SCHEMA_UNSTAMPED_SOURCE_ROT` (a pinned schema's sealed package no longer passes authoring validation) and the new `MOUNT_UNBACKED`; and on the consistency findings `ORPHAN_STUB` and the dangling-link family when `integrity` is included. (2026-08-27: that family was one code, `DANGLING_LINK`, until consistency sweep 04/06 split it into `DANGLING_LINK_TARGET_MISSING`, `DANGLING_LINK_NOT_RELATED` and `DANGLING_RELATION_TARGET_MISSING`. Which conditions refuse is unchanged; all three participate, and the strict counter filters on the enum's own roster rather than a literal, so the split could not quietly drop one.) Stale entities, drifted anchors and `SCHEMA_GENERATIONS_BEHIND` stay advisory: a stale entity is a fact about time, a drifted anchor is the sync loop's input, and a generations-behind pin keeps working. `MOUNT_UNBACKED` is a boot and reload warning for a mount that resolves to nothing, with `details.reason` in `missing_ref` (a git-branch mount whose branch was never created or was deleted), `missing_path` (a folder or archive mount whose path is gone) and `empty` (storage that exists and holds no entity); a mount serving at least one entity is silent. The backend trait gained `storage_present()` so boot can tell "never created" from "empty": `list_entities` folds a missing branch into an empty list and cannot. The nested-prefix warning says what it can tell instead of guessing: "target missing in mem X" when the link's prefix is a mounted mem, "prefix 'X' is not a mounted mem" when it only matches a mem name's last segment.

A fourth class joined the strict set on 2026-08-27 (consistency sweep 04/07): `CROSS_MEM_EDGE_UNGRANTED`, an existing cross-mem edge the workspace grant table no longer permits. Cross-mem links are default-deny and gated on write, so such an edge is a state the engine would refuse to create today; before this it survived boot untouched and exited zero under `--strict`, so the policy file could stop describing the graph with nothing forcing the two back into agreement. The posture decided with it: reported and strict, NEVER a load refusal and never a quarantine, because a policy edit that takes a mem offline blocks its own remedy, and the revocation that orphans an edge names it at the moment it happens rather than leaving it for a later gate run. Revocation itself is not refused and needs no force flag: making policy hostage to data would stop an operator saying two mems should no longer link until they had already cleaned up, and the cleanup is easier once the policy states the intent. The mem-deletion precedent, which does refuse, differs because deleting a mem destroys the target where revoking a grant destroys nothing.

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

Quarantine posture settled 2026-08-27 (consistency sweep 04/05, operator ruling): a folder or archive mount whose storage is gone QUARANTINES rather than serving an empty graph, and every roster surface renders the quarantine roster; a git-branch mount whose ref does not exist keeps the loud `MOUNT_UNBACKED`/`missing_ref` warning and STAYS SERVING. The asymmetry is deliberate, not an omission: a missing ref is also the normal state of a mem that has never been pushed, and push/fetch/pull are the repair path a quarantine would strand (three transport layers assume a serving mount: the lookup, the pre-push schema resolution, and pull's pre-fast-forward validation, which has no resolved schema for a mem being cloned). The honesty obligation is carried by the warning, the roster flag and the strict-health refusal, which this decision already made always-on.
