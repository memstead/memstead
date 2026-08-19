---
type: principle
created_date: 2026-08-19T04:11:32Z
last_modified: 2026-08-19T04:11:32Z
authority: accepted
universality: domain-wide
tags: testing, ci, gates, honesty
---

# A test gate that exists must gate

## Statement
Every stated test contract EXECUTES in the canonical gate, and every gate is positioned where it can stop the thing it guards against. A test file that never runs, a doc example that never compiles, a guard that fires only post-merge, a lane that is red on every run, or a green run that leaves behind an artifact that is not what its path says it is — each is a gate that exists without gating, which is worse than no gate: it teaches everyone that stated contracts are decorative.

## Scope
The public engine repo's test surface ([[engine--engine-ci-gate]]): `run-tests.sh` legs, CI jobs, docs-site guards, and any artifact a green run leaves behind. Applies when adding a test surface, when positioning a guard, and when a lane goes ambient-red.

## Relationships
- **REFERENCES**: [[engine:engine-ci-gate]]

## Justification

Backlog-sweep plan 04 removed four such gates in one pass, each found live: doctests ran in no leg anywhere; the wasm tests — asserting the one published-surface guarantee a consumer cannot check for themselves — had never executed; the docs-site guards could only fail after merge; and a green local run left a silently degraded lean binary at `target/debug/memstead` (two sessions paid a false-negative probe round each). The same pass diagnosed `app-ci`: a lane red on every run for a month, blocking nothing and warning nobody — the docs-drift gate recorded the identical pattern before its enforcement moved to the push boundary. Skipping is permitted only as a LOUD degraded mode that names what was not checked (the node-less docs-guard skip), never as a silent pass.

## Exceptions



## Consequences

Adding a test surface includes wiring it into `run-tests.sh` (the single definition of green) in the same change; a red lane is repaired or explicitly retired with a recorded decision, never left ambient; a demonstration that a new leg actually fails (break it, watch the gate go red, revert) is part of landing it.
