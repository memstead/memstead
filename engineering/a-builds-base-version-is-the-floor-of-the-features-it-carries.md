---
type: decision
created_date: 2026-09-11T06:25:30Z
last_modified: 2026-09-11T06:25:30Z
status: accepted
decided_on: 2026-09-11
deciders: seams follow-up bundle plan 01 (agent, on the operator's instruction of 2026-09-11)
scope: subsystem
tags: plugin, capability-gate, versioning, release-process
---

# A build's base version is the floor of the features it carries

## Decision
We will read a memstead binary's base version as the floor of the features it carries, build metadata or not: the plugin's version-gated capabilities (the anchors, repo and consume gates in binary-version.mjs, and any gate added on the same ladder) answer capable for a recorded binary whose base version is at or above the capability's threshold, and the reason names the build when the banner carries +g<sha> or -dirty. Two states stay fail-closed and keep their reasons: no record or an unparseable banner, and a base version below the threshold, which reads as predating the capability whatever metadata it carries ([[plugin--sync-skill]], [[plugin--plugin-skills-layer]]).

## Context
The crate version moves only in the release commit: `cargo run -p xtask -- release <version>` bumps every manifest, the changelog is cut, and the tag follows; features land under the previous version. So a build reporting X+g<sha> descends from the X release commit and carries every feature X shipped. The gate's 2026-08 design read the metadata as doubt, on the premise that a build could call itself X before the commit that added an X feature; the premise contradicts the release process. The cost: the engine's build script stamps metadata on every build except a clean release build, so every development workspace ran with every capability degraded, and the architecture-seams inventory sessions of 2026-09-11 (7 and 8) overrode the gate by hand, which is the failure a gate exists to prevent.

## Consequences
- A development workspace gets anchors, --repo and --consume from its own build without an override, and the reason still names the build so a reader sees it is not a release.
- The gate's wire shape is unchanged: {capable, version, reason}, with build present on a metadata-bearing record; callers branch on capable only.
- The rule binds any future capability on the ladder: a threshold is a release number, and a base version at or above it is enough.
- The one case the rule cannot see, a dev build of a version bumped ahead of its features, is ruled out by the release runbook rather than by the gate; changing the runbook to bump early would reopen it.

## Relationships
- **REFERENCES**: [[plugin:sync-skill]]
- **REFERENCES**: [[plugin:plugin-skills-layer]]

## Options

- Keep the metadata as the doubt signal, as designed in 2026-08: rejected, it degrades every development workspace forever, since between releases every build from the tree carries metadata.
- Confirm a dev build by comparing its sha against the release tag through git: rejected, the plugin has no git and no checkout of the engine on a user's machine; the banner is the only evidence it holds.
- A per-workspace override declaring a dev build capable: rejected, a knob for a rule the release process already guarantees.
- Treat -dirty as doubt even at or above the threshold: rejected, a dirty tree at base X still descends from the X release commit and cannot remove a feature X shipped.

## Notes

Revisit if the release process ever bumps the crate version before the features of that release land, or a capability is added between a version bump and its tag: the gate cannot see that case; the runbook rules it out.
