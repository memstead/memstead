---
type: decision
created_date: 2026-07-13T16:43:07Z
last_modified: 2026-07-13T16:43:07Z
status: accepted
decided_on: 2026-05-19
deciders: memstead-core
scope: component
tags: file-watcher, coherence, cross-process, notify, engine
---

# Use a polling file watcher for mem-repo ref changes

## Decision
We chose `notify::PollWatcher`, polling `<gitdir>/refs/heads/` at a fixed 50 ms interval with `with_compare_contents(true)`, as the v1 backend for the [[engine--cross-process-mem-repo-file-watcher]] — rejecting the per-platform native backends that `notify::RecommendedWatcher` would select (FSEvents on macOS, inotify on Linux, ReadDirectoryChangesW on Windows).

## Context
Sibling engine processes share one git-branch mem-repo; each engine learns that another writer advanced a branch tip by watching the loose ref files under `refs/heads/`. That watcher must behave identically in the environments the engine actually runs in — tempdir-based test suites, sandboxed embedders, network volumes — across three operating systems, while hitting a 10–50 ms detection-latency target. The native notify backends each carry per-platform quirks in exactly those environments, and the watched tree is tiny (a handful of loose ref files per mem branch), so the usual cost argument against polling does not apply.

## Consequences
- Deterministic cross-platform behaviour: no FSEvents/inotify/RDCW code path is compiled in, so tests and embedders see the same event timing everywhere.
- Detection latency is floored at the poll interval (50 ms) instead of native push latency — accepted because the band the design targets is 10–50 ms anyway.
- Content comparison (not just mtime) catches two ref updates landing within one OS-level mtime tick (1 s granularity on some filesystems), at negligible cost on the small `refs/heads/` tree.
- Cost: a permanently running poll thread per watched mem-repo, and the latency floor cannot be lowered without revisiting this decision.
- Follow-up trigger: a production deployment that needs lower latency switches to `RecommendedWatcher` behind a config knob — deliberately left out of v1 to keep the surface small.

## Relationships
- **REFERENCES**: [[engine:cross-process-mem-repo-file-watcher]]

## Options

- **`RecommendedWatcher` (per-platform native backends)** — rejected: FSEvents/inotify backend quirks under tempdir, sandbox, and network-volume conditions make event delivery nondeterministic in precisely the environments the test suite and embedders run in.
- **`PollWatcher` with mtime-only comparison** — rejected: ref writes within the same OS-level mtime tick (1 s on some filesystems) would be missed, and a busy sibling writer can advance a ref twice inside that window.
- **`PollWatcher` with `with_compare_contents(true)` at 50 ms** — chosen: deterministic everywhere, meets the latency band, content comparison is cheap on the small ref tree.
- **Configurable backend (knob between poll and native)** — deferred, not rejected: named in the construction-site comment as the escape hatch for production deployments needing lower latency; omitted from v1 to keep the surface small.

## Notes

The choice and its rationale live as an inline comment at the `PollWatcher` construction site in `crates/memstead-base/src/engine/file_watcher.rs`; the mechanics (event loop, RAII handle, loose-refs-only limitation) are specified by [[engine--cross-process-mem-repo-file-watcher]]. The module-doc header at that site records the same choice — `PollWatcher` chosen over `RecommendedWatcher` for cross-platform determinism.
