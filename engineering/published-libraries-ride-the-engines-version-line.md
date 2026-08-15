---
type: decision
created_date: 2026-08-15T06:14:17Z
last_modified: 2026-08-15T06:14:17Z
status: accepted
decided_on: 2026-08-15
deciders: operator, implementing agent
scope: system
tags: release, versioning, npm, distribution, compatibility
---

# Published libraries ride the engine's version line

## Decision
A published library that reads what the engine writes carries the engine's version number. `@memstead/wasm` published at `0.7.0` — matching the engine crate version rather than continuing its own `0.1.x` line — and `release-verify.sh` compares it like every other channel instead of printing it as a bare number under "own track".

The package README states the rule where an installer will read it: this version is built from that engine generation, and reads the archives that generation's CLI writes. One number answers "will this read my file?".

This is a versioning policy, not a one-off bump: a library on its own line has to be *told* to stay in step, and nothing tells it.

## Context
`@memstead/wasm` sat at `0.1.2` while the CLI shipped `0.7.0`. It could not read any archive that CLI wrote, and neither registry page said so — crates.io showed one number, npm showed another, and the fact that actually decides compatibility (the archive format integer) appeared on neither. The 2026-08-14 cold-start run called it the single biggest wall it hit: one of four advertised programmable surfaces was dead on arrival for anyone who did not build from source, which no newcomer does.

The entry page warned honestly that libraries may lag and told readers to check the published versions — advice that could not be acted on when the answer was `0.1.2` against `0.7.0`. A mitigation, not a fix.

The deeper cause was structural: its own version line meant nothing connected the package's number to the engine's, so the skew accumulated silently across three releases. Plan 02 had already tried and failed to close the gap by *describing* it — a generated compatibility table proved underivable and was withdrawn (see [[engineering--a-surfaces-claim-about-itself-is-derived-or-absent]]). Matching the numbers removes the question instead of answering it.

## Consequences
`release-verify.sh` now fails when npm disagrees with the release, so the skew cannot accumulate unnoticed again — it is a compared channel, not a stated skip. crates.io remains a deliberate skip on its own terms.

The version jump `0.1.2` → `0.7.0` is deliberate and does not auto-upgrade anyone pinned to `^0.1.x`, which is correct: those consumers are on a package that genuinely cannot read current archives.

Every consumer's dependency range now tracks the engine generation. memstead.io's prepared switch had `^0.1.0` hard-coded and would have installed the stale package verbatim — a caret on `0.x` does not cross the minor. Any future prepared switch inherits that trap.

The cost: a library with no engine-visible change still gets a version bump each release, and the publish is one more act that must actually happen. `release-verify.sh` going red is the mechanism that makes forgetting visible rather than silent.

## Relationships
- **REFERENCES**: [[a-surfaces-claim-about-itself-is-derived-or-absent]]

## Options

**Match the engine version (chosen).** One number, no lookup, and the release-verify comparison becomes possible.

**Keep its own line and publish a stated compatibility range (rejected).** Plan 02 attempted the descriptive half and withdrew it: the statement could not be derived from data the build has, because the version cell describes a published artifact while the format cell describes the current tree. A hand-maintained range is the same defect one level up.

**Leave it stale and describe the staleness accurately (rejected, and was the status quo).** The entry page already did this. The run showed the advice is unactionable at that distance, and it leaves an advertised surface dead.

## Notes


