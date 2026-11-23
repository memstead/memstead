---
type: decision
created_date: 2026-11-22T22:11:51Z
last_modified: 2026-11-22T22:21:20Z
status: accepted
decided_on: 2026-11-22
deciders: operator (stuck-mem bundle, go of 2026-11-22), implementing agent
scope: subsystem
tags: mem-repo,roster,quarantine,cli
---

# A quarantined mount leaves the roster by unregister, deciding by no mem's content

## Decision
We chose to let `memstead mem unregister` take a quarantined mount of a branch or a folder off the roster under either capability, whatever put the mem out of service. The removal decides by no mem's content and takes no write lock of the mem. The roster row goes. A write mount whose storage is still there and is the workspace's to keep (a branch of its own mem-repo, a folder) gets the marker a mount in service gets; a mount of another git directory gets nothing written there, and storage that is missing gets nothing written at all. Entities of other writable mems in service that hold edges into the mem are reported as detached referrers and are no reason to refuse. The unregister of one quarantined mount runs beside other quarantined mounts. A storage-destroying deletion of a quarantined mount stays refused, with `MEM_QUARANTINED` and before any lock.

## Context
While one mount is quarantined, every verb that reads the whole workspace refuses, by design: the quarantined mem's referrers are unread. The refusal told the caller to repair the mem or remove it, and removing worked for a read-only mount only. A write mount refused `UNKNOWN_MEM`, so with its storage gone for good nothing but a hand edit of the roster file freed the workspace, and two quarantined mounts barred each other's removal. Found while the mount verb of 0.27.0 was graded.

## Consequences
- The refusal a quarantined mount causes has a remedy that works, and both refusals name it.
- What pointed into a mem that is removed while out of service is known to the caller, as a report.
- The report covers mems in service: what another quarantined mem holds is unread and missing from it.
- Over MCP a mem deletion is always the storage-destroying form, so removing a quarantined mount stays administration on the CLI.
- In a workspace of the plain folder shape the unregister verb refuses `UNSUPPORTED_WORKSPACE_SHAPE`, so a quarantined folder mem cannot be removed there.

## Relationships
- **IMPLEMENTS**: [[degrade-never-disappear]]
- **DERIVED_FROM**: [[a-quarantined-mount-is-tried-again-whenever-the-roster-is-written]]

## Options

- Let the workspace-wide verbs run past a quarantined mount: rejected, their guards would be evaluated without that mem's referrers.
- A separate verb or a force flag: rejected, unregister already means the row goes and the storage stays.
- Refuse while other mems hold edges into the quarantined mem: rejected, with the storage gone the refusal could never be satisfied.
- Never write the marker: rejected, it is what lets a later init reattach a branch without a flag.
- Unregister removes the row and reports the referrers: chosen.
