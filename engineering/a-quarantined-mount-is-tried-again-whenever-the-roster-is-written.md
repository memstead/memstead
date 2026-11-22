---
type: decision
created_date: 2026-11-22T13:15:46Z
last_modified: 2026-11-22T13:15:46Z
status: accepted
decided_on: 2026-11-22
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: mem-repo,reload,roster,quarantine
---

# A quarantined mount is tried again whenever the roster is written

## Decision
We chose to have a running engine try every quarantined row again when the roster file was written, also when its rows end exactly as they were, and on a full reload of the workspace. A row that comes back is reported as added; one that fails again for the storage and pin it had is not reported again. A quarantined row follows its roster row: it attaches under the capability, storage and pin the roster gives at that moment.

## Context
A mount quarantined because its git directory was away stayed quarantined in a running engine after the directory was back and the operator had unregistered and mounted the name again: the roster's bytes ended as they were, so the engine saw no change. Only a reload that named the mem brought it back, and the operator's two commands had both reported success. Found by an independent grader on 2026-11-22.

## Consequences
- The repair an operator reaches for, unregister and mount again, works without a restart.
- A roster write by any process makes every engine in the workspace attempt its quarantined rows once; each attempt costs a cold mount.
- A row whose cause stands produces no repeated notice.
- Without a roster write nothing is retried: a storage that comes back on its own stays quarantined until the roster is written or the mem is reloaded by name.
- The workspace-wide reload without `full` does not retry.

## Relationships
- **IMPLEMENTS**: [[degrade-never-disappear]]
- **DERIVED_FROM**: [[a-roster-row-is-the-same-mount-only-while-its-capability-storage-and-pin-stand]]

## Options

- Retry on every operation: rejected, a failing cold mount per operation for as long as the cause stands.
- Leave it to the reload that names the mem: rejected as the only path, the operator's commands looked successful and the server still refused.
- A revision counter in the roster file: rejected, a format change where the file's modification time already tells a write.
- Retry on every write of the roster: chosen.
