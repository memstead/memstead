---
type: decision
created_date: 2026-08-23T15:50:35Z
last_modified: 2026-08-30T00:32:29Z
status: accepted
decided_on: 2026-08-23
deciders: operator, implementing agent
scope: system
tags: docs, prose, ci, gates, release, plugin
---

# Documentation is guarded in two tiers: hard against the built binary, report-only against the published tag

## Decision
We will guard documentation in two tiers with one checker. TIER ONE, hard: `ci/check_prose.py` (built-ins only, the Rust extractor is gone) resolves every `memstead` invocation in fenced shell blocks and `yaml` `run:` lines, every flag attached to one, and every relative link (docs-site trees resolve by route) against the binary BUILT from the tree, as a `run-tests.sh` leg over the public prose set, as the `xtask release` guard over the flagship at whole-file scope, and as a job of the workspace repo's hygiene lane over the private prose set against the submodule's built binary; prose phrases and placeholders are allowlisted by the existing `docs-guard-allow.txt` format (now with `flag:` and `re:` entries) plus a private sibling allowlist. TIER TWO, report-only: `release-verify.sh --prose` downloads the highest published tag's CLI once into a cache, runs the same checker's user-facing subset against it and prints the gap per file and flag (exit 2), never trusting a local binary's version string; the same script reports changelog headers without a tag or a "never published" note and compare links naming unknown refs. A flag on a plugin skill path is gated on the RECORDED binary version (`REPO_MIN`, `CONSUME_MIN` beside `ANCHORS_MIN`): below the minimum the skill drops the flag and says which version it found and which it needs; a missing record degrades the same way; flags on no skill path stay ungated. Counts leave prose for pointers wherever the source is machine-readable; the three that stay (crates, test legs, MCP tools) are derived and compared by a `--check` script, beside a vocabulary lint for retired forms and a check that the constitution's structure table equals the tracked tree. The hygiene lane's jobs landed under `continue-on-error` and flip hard once the defects they find are repaired. Corrected 2026-08-30: that flip has happened. Every job in `.github/workflows/repo-hygiene.yml` is hard since consolidation plan 05 cleared the defects on the 2026-08-23 tree, and no `continue-on-error` key remains in the workflow; a red job in that lane is a real finding.

## Context
Prose about the binary drifted three ways and no gate saw any of them. A proof README documented `memstead stats`, a command that never existed. The flagship's docs-vs-binary guard lived as a Rust extractor inside `xtask release` and ran only at release time, over one document set, so every other document (the README, the guides, the plugin's skills, the private handbook) could name a flag the binary does not have. The published release and the tree disagree by design between a landing and its tag, so a hard check of prose against the published binary would be red most days and would be ignored (see [[engineering--a-test-gate-that-exists-must-gate]]). The plugin's skills pass flags such as `--repo` and `--consume` to whatever binary the user recorded, and a recorded binary older than the flag failed with a parse error instead of a sentence. Counts restated in prose ("8 crates" while the tree held 7) are a second source of truth that drifts, against the handbook's own rule, stated as [[engineering--a-surfaces-claim-about-itself-is-derived-or-absent]].

## Consequences
- Every public Markdown that names a command or flag is checked on every test run, and the flagship is checked at release cut; a new command or flag must land with its documentation or the leg refuses.
- The gap between the tree and the last release is a readable report, never a red lane; the release run itself prints it after announce.
- Installing a newer plugin over an older recorded binary degrades with a sentence instead of a parse error; recording the binary version is what turns the gated flags on.
- A count in the handbook that disagrees with the tree is red in the hygiene lane; the cure is a pointer to the source, not a corrected number.
- Cost: the engine is built once per hygiene run (cached); the allowlists need review on every addition, since an entry is a claim that a phrase is prose.
- Proof: the checker's fixture self-test (unknown command, unknown flag in a fence, a `run:` line, a broken link, a routed link, a non-`memstead` flag, a placeholder, whole-file versus fenced scope), the release-verify tests for `--prose` (ahead, at tag, offline) and the changelog check, the plugin gate tests (below, at, above, missing record), and each outer script's own fixture suite.

## Relationships
- **REFERENCES**: [[a-test-gate-that-exists-must-gate]]
- **REFERENCES**: [[a-surfaces-claim-about-itself-is-derived-or-absent]]

## Options

- A hard gate against the published tag: rejected. Red by design between a landing and its release; a gate that is red on most runs is decorative.
- Keeping counts in prose and re-deriving them by hand: rejected. A second source of truth against the handbook's own rule; correcting the number resets the clock on the same drift.
- A Node checker: rejected. The checker must run where the Rust toolchain and the release tooling run, without a package install; Python's standard library is already required there.
- Probing the binary for capabilities at skill time: rejected. The plugin's gate fails closed on the recorded version by intent, so a stale record degrades predictably instead of a live probe deciding differently per machine.
- Landing the hygiene lane hard: rejected. It would be red on the default branch until the repairs land; `continue-on-error` with a summary keeps the findings visible without teaching anyone to ignore red.
- One checker in two tiers, with version-gated flags and derived counts: chosen.

## Notes

First catch at landing: `docs/proof/reconstruction/README.md` documented `memstead stats` (the command is `status`). Findings recorded at landing, all since repaired (verified 2026-08-30, correcting the present-tense phrasing this sentence used to carry): the retired slash prefix in the local-LLM notes folder is gone, the engine chapter now states 7 crates against a tree of 7 with the count gated by `scripts/derived-facts.py --check`, and `scripts/constitution-check.py --check` passes on all 13 tracked top-level directories. The private prose set was clean against the built binary once the checker learned that a scheme-prefixed link target points outside the tree.
