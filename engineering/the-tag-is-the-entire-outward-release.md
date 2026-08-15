---
type: decision
created_date: 2026-08-15T08:41:56Z
last_modified: 2026-08-15T08:41:56Z
status: accepted
decided_on: 2026-08-15
deciders: operator, implementing agent
scope: system
tags: release, ci, distribution, automation, drift
---

# The tag is the entire outward release

## Decision
Pushing a version tag performs every outward act of a release: binaries, attestation, the GitHub Release, the Homebrew tap, the six crates on crates.io, and `@memstead/wasm` on npm. The two registry legs are cargo-dist custom publish jobs wrapping the same scripts that were previously run by hand — the scripts remain the contract, the workflows supply only a runner, a toolchain and a token. `release.yml` is regenerated through dist config rather than hand-edited, because a hand-edit is erased by the next `dist generate`.

Both registry jobs FAIL the release when their secret is absent. They do not skip. A silent skip is the exact mechanism by which the registries fell behind, so the polarity is deliberate.

The private half — gitlink, the private crates' locks, a build of each private consumer, the `.ai` deploy, and a rebuild of the maintainer's own binaries — is one command, `scripts/adopt-release.sh <tag>`, which refuses rather than guesses on a dirty tree, an absent tag, a consumer that does not build, or a red gating check.

Two supporting guards make the automation safe to trust: `xtask release` now bumps every hand-set version manifest (plugin, marketplace, wasm package) alongside Cargo, and the npm job refuses to publish when the package version disagrees with the tag.

## Context
Every channel that ever drifted drifted for one reason: updating it was a separate act somebody had to remember. Not negligence — the acts were documented, and each was skipped by a person who had just done twelve other things correctly.

The record is unambiguous. The plugin shipped a version behind in 0.6.0 and cost a second commit on top of the tagged one. `@memstead/wasm` sat at 0.1.2 while the CLI shipped 0.7.0, could not read a single archive that CLI wrote, and a stranger's cold-start run called it the biggest wall in the product. crates.io sat two minor versions back under a label reading 'deliberate skip' that had stopped being a decision and become a habit. Three private crates' lock files went stale at every version bump and were each discovered by a red CI lane days later. The maintainer's own machine ran 0.4.0 across two releases — including during a run measuring whether a newcomer copes, which therefore measured the wrong engine.

The release chapter had named this capstone 'future-proof' for weeks. It kept being deferred because mid-release the pressure is to ship, and cutting the release by hand was always the cheaper thing in the moment. It cost an afternoon twice before the operator asked whether it had to.

## Consequences
A release is `xtask release X` → commit → push → tag → `adopt-release.sh`. Registries are no longer a step; they are a consequence.

The failure mode moves from silent to loud. An absent token, a mismatched package version, or a private consumer that does not compile now stops a release instead of producing a channel that quietly serves the wrong thing. That is the intended trade: a red job is cheap, and a version skew nobody sees costs a field report.

Two obligations follow for anyone adding a distribution channel. It publishes from the tag, or it is a documented skew that `release-verify.sh` reports by name — 'somebody will remember' is no longer an accepted design. And a channel that cannot be published from CI still gets a line in the verifier, as the maintainer's own machine now does, because the point is visibility rather than automation for its own sake.

The secrets are the one part a machine cannot own: `CARGO_REGISTRY_TOKEN` and `NPM_TOKEN` live on the public repo, and the npm token expires. Rotation is now a release-blocking event rather than a silent skip, which is the right direction and still a calendar item.

## Relationships
- **ENABLES**: [[published-libraries-ride-the-engines-version-line]]

## Options

**Keep publishing by hand, documented better.** Rejected: it was already documented, in a chapter that also told you to do it. Documentation was never the missing part.

**Publish from a scheduled job rather than the tag.** Rejected: it decouples the artifact from the release that produced it, and the failure it protects against — a channel serving a version nobody chose — is exactly what a schedule can also produce.

**Trigger the private half from the public repo via repository dispatch.** Rejected: it requires a token with write access to the private repository stored as a secret in the OPEN one. The gain over one local command did not justify putting that credential where it would live.

**Hand-edit `release.yml` rather than going through dist config.** Rejected outright: the file carries a generated-by header, and the edit would vanish at the next regeneration — reintroducing the drift under a new name.

## Notes


