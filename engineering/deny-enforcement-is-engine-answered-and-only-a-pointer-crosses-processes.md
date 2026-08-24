---
type: decision
created_date: 2026-08-21T17:51:04Z
last_modified: 2026-08-21T17:51:04Z
status: accepted
decided_on: 2026-08-21
deciders: agent session (flywheel W6/02) under the SPEC decision and the standing engine-change directive
scope: subsystem
tags: projection-pipeline, deny-paths, plugin, hooks, one-dialect
---

# Deny enforcement is engine-answered and only a pointer crosses processes

## Decision
The deny dialect has one implementation, in the engine: `memstead projection check-path` answers "is this path denied for this binding" — single-path and `--batch` stdin forms, verdicts naming the matched deny entry — evaluated by `ingest::check_path::check_deny_paths` with the same globset machinery the enumeration path uses, plus the literal-base directory-prefix rule ported from the hook (`dev/**` also blocks a read of `dev` itself; a legacy bare name degrades to a prefix block). The plugin's PreToolUse deny hook shrank to a thin subprocess caller.

The cross-process channel changed shape with it: consuming brief renders publish only an ACTIVE-BINDING pointer (`.memstead.cache/projection/active-binding.json`, the canonical binding id — never a deny list). Every check reads the active binding's record fresh, so a stale deny list can no longer be enforced by construction; the retired cache's remove-then-write stale-safety re-expresses as "a failed pointer write leaves no pointer, and an unanswerable check fails open". With `--binding` the command answers for any named binding — generic by design, the hook is one consumer.

## Context
The deny enforcement was split across a language boundary: the engine resolved `deny_paths` with Rust globset while the plugin hook re-implemented those semantics in 167 lines of JavaScript, pinned together by a shared fixture with parity guaranteed only for `*`, `**`, `?` and literals — character classes and brace alternates were silently treated as literals. The engine-written deny-list cache had already produced one real incident (a peek render pointing the hook at the wrong binding's list, fixed 2026-08-19). SPEC W6 decided the retirement; the batch stdin form is the amortization that keeps a per-tool-call consumer viable (~3 ms per engine check on the dogfood workspace, release build; hook end-to-end ~48 ms vs ~44 ms before, node startup dominating both).

## Consequences
Retired together: the JS dialect clone, the shared dialect fixture, its Rust consumer (a cross-boundary relative-path reach from the engine crate into the plugin tree), and the deny-list cache with its remove-then-write machinery. Dialect parity is total where it previously held and wider where it did not — patterns beyond the old JS parity boundary now follow engine semantics everywhere. Any future consumer (editors, other hooks, CI) gets the same authoritative answer from the same seam. The peek/consume purity rule ([[engineering--rendering-a-rotation-brief-is-a-pure-read-and-taking-the-slot-is-an-explicit-consuming-act]]) carries over unchanged: only a consuming render moves the pointer. Enforcement is now only as available as the `memstead` binary — an unavailable binary fails open with a stderr note, the same default-open posture the missing cache file had.

## Relationships
- **REFERENCES**: [[rendering-a-rotation-brief-is-a-pure-read-and-taking-the-slot-is-an-explicit-consuming-act]]

## Options

Rejected: keeping the JS fast path beside the engine check (two enforcement paths is the defect; the cache's wrong-binding incident showed the parallel implementation was not merely redundant but wrong until guarded). Rejected: porting the missing globset features to JS (deepens the second implementation; the parity burden grows with every dialect feature, forever). Rejected: a resident check server/daemon (a process to manage, leak, and version inside a plugin hook — the engine-free binding load plus batching reaches the same latency class without one). Rejected: a `--from <file>` batch form per house convention (the consumer holds candidates in memory per tool call; a tempfile round-trip per invocation is the cost the batch form exists to avoid — deliberate, documented deviation).

## Notes


