---
type: decision
created_date: 2026-08-19T09:22:20Z
last_modified: 2026-08-19T09:22:20Z
status: accepted
decided_on: 2026-08-19
deciders: backlog-sweep plan 07 (operator decision 20, 2026-08-18)
scope: subsystem
tags: folder-mems, merge-conflicts, guards, provenance
---

# Merge conflicts get an engine door — the agent judges, the engine writes, the guards stay closed

## Decision
Git merge conflicts in folder-mem entity files are resolved through the engine, never through git verbs or raw edits: `memstead conflicts list` identifies conflicted entities, `memstead conflicts resolve <id> --side ours|theirs` keeps one side per entity. The chosen side is validated before anything lands (residual nested markers, unknown section keys, and content-format violations refuse typed; missing required sections stay soft, matching the update surface's posture — resolution is an update-kind mutation on an entity that already exists, and a strict gate could leave both sides unresolvable). The resolution commits as an attributed, note-carrying mutation in the provenance ledger. Merged-content resolution is deliberately out of scope: resolve to the better base, edit through `memstead update`, and record the manual merge in the note. Folder backend only — the git-branch mem-repo is engine-managed and refuses CONFLICT_RESOLVE_UNSUPPORTED_BACKEND; a non-conflicted target refuses NOT_CONFLICTED.

## Context
Surfaced by the 2026-07-19 merge-conflict session: a merge conflicted a hand-committed folder mem, the loader could not parse the files, the guard hooks (correctly) refused shell and raw-edit repair, and the only escape was operator hands — the resource the operator-obsolescence trajectory says to stop spending. The rejected alternative — a guard allowance for `git checkout --ours/--theirs` on conflicted paths — would have punched the first hole in [[engine-owns-mem-repo-state]], bypassed entity validation entirely, and left no engine-side provenance. Discovery rides the failure: the loader's conflict refusal names the resolve operation at the exact moment an agent needs it.

## Consequences
A machine path replaces a human gate without weakening any guard — the plugin's no-git-against-mem-repo assertions are untouched. Auxiliary honesty fix that fell out: a per-mem reload now REPLACES that mem's load-error entries instead of accumulating them forever (folder-mount error paths normalized to absolute for the purpose), so a repaired file stops reporting its old refusal. Caveat recorded: conflict detection evaluates code-fence-masked content, so a documentation entity showing example markers in a fence stays legal, at the cost of missing the rare conflict whose markers all fall inside one fence — that shape degrades exactly as it did before the detector existed.

## Relationships
- **REFERENCES**: [[engine-owns-mem-repo-state]]

## Options



## Notes


