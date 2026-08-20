---
type: decision
created_date: 2026-08-20T07:59:39Z
last_modified: 2026-08-20T07:59:39Z
status: accepted
decided_on: 2026-08-20
deciders: agent (unattended loop, per plan 03's harness-extension authorization)
scope: subsystem
tags: cold-start, gate, testing, seal, release
---

# Stage candidate surfaces to a sealed container through a served origin, never a mount

## Decision
The sealed newcomer gate shows a container **unreleased** surfaces the same way a release would: as an origin it fetches over HTTP. `dev/cold-start/build-candidate.sh` compiles the binaries inside a builder image whose context is `git archive HEAD` of the public submodule; `dev/cold-start/gate.sh` assembles an origin directory (the README, the artifacts, a staging installer that names itself as pre-release), serves it from its own container on a private Docker network, and points the newcomer at it with `COLD_START_ENTRY`. A pinned public repository is cloned and mounted as the modelling subject via `COLD_START_SUBJECT`.

Three properties fall out of that shape rather than out of discipline: the repository source reaches the builder image and nothing else, so the newcomer's image is as naive as it ever was; nothing is published on a host port, so the staging surfaces are reachable from the sealed container and from nowhere else; and the candidate is a **commit**, because the build context is an archive of HEAD and an uncommitted edit cannot reach it.

`gate.sh --selftest` drives the whole path with plain shell instead of an agent, so the mechanism is verifiable with no credential and no model in the loop.

## Context
SPEC W1's terminal gate is a sealed newcomer run, and a sealed run installs from public surfaces by construction. Measured on 2026-08-20 the newest public release was v0.8.1 while the repo stood at 0.9.0 with the guided path (`quickstart --repo`) still in `[Unreleased]` — so a gate run against public surfaces would have measured a two-versions-old product and reported it as the bundle's work. Either the gate waits for a release (inverting the SPEC's sequencing, which exists precisely so the first-session plans do not final-report unmeasured), or the candidate reaches the container some other way.

The seal is what makes the measurement worth anything, so "some other way" could not be a bind mount of the repo, a pre-baked binary, or a coached instruction.

## Consequences
A gate run can happen before a release, which is the sequencing the SPEC asked for.

The selftest passes end to end: no `CLAUDE.md`, `dev/` or `public/` reachable in the container and no `memstead` pre-installed; the entry page and README serve; the staging installer installs a running binary; `memstead quickstart --repo .` against a cloned `rust-lang/log` produces a workspace, mem, seed entity and binding; `overview` and the binding brief render.

**What it does not measure, and this must not be overclaimed:** the production `install.sh` resolves a GitHub release and runs cargo-dist child installers, while the staging installer serves a pre-release build from the gate's own origin. The gate therefore covers the guided path and the README, not the production install plumbing — that leg stays with the deployed-surface run. The README is served as a bare file, so its repo-relative links do not resolve on the origin the way they do on GitHub; a finding about those links would be an artifact of staging, not of the product.

The surface overrides are additive: absent `COLD_START_ENTRY` and `COLD_START_SUBJECT` the skill's entry point and subject are exactly what they were, so ordinary campaigns are unmoved.

## Relationships
- **REFERENCES**: [[a-claim-about-running-state-is-measured-against-the-running-system]]

## Options

- **Wait for a release and run the gate against genuinely public surfaces** — rejected as the default, because it inverts the SPEC's sequencing and lets the first-session plans ship unmeasured. It remains the honest fallback if staging fidelity is ever judged insufficient, and it is the operator's call, not this decision's.
- **Bind-mount the repository into the newcomer container** — rejected: it destroys the structural naivety that makes zero-walls evidence rather than optimism.
- **Bake the candidate binary into the newcomer image** — rejected: the install leg is part of what the gate measures, and a pre-installed binary makes it vacuous.
- **Add a base-URL override to the production `install.sh`** so the real installer could point at staging — rejected for now: it modifies a shipped surface to serve a test, and the gate's subject is the guided path rather than the installer. Worth revisiting on its own merits (mirrors, air-gapped installs), not as a test affordance.

## Notes

Follows [[engineering--a-claim-about-running-state-is-measured-against-the-running-system]]: the release gap that motivated this was found by querying the deployment's release list, not by reading the changelog.
