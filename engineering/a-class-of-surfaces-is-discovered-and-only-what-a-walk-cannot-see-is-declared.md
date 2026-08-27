---
type: principle
created_date: 2026-08-26T17:28:42Z
last_modified: 2026-08-26T20:43:14Z
authority: accepted
universality: domain-wide
tags: checkers, surfaces, discovery, manifests, coverage
---

# A class of surfaces is discovered, and only what a walk cannot see is declared

## Statement
A checker that asks a question of a class of surfaces DISCOVERS its class by walking it, and holds a short list of named exemptions beside the walk. It never carries a hand-kept list of members. Where a surface genuinely cannot be reached by a walk of tracked files (a mem that lives on a git branch, prose sealed inside a tarball, text that exists only inside a compiled function), the declaration names it explicitly, and the declaration is itself held against a walk of the substrate that CAN see it: a branch listing, an archive glob, a grep for the marker that betrays the prose.

The split is the rule. Discovery covers what a walk can see; declaration covers only the remainder, and the remainder is never assumed to be stable.

## Scope
Every program that asks one question of many surfaces: vocabulary lints, disclosure guards, prose checkers, freshness gates, coverage inventories. It governs the checker's scan set, not what the checker asserts about a member it finds.

It does not govern a set another program already computes: naming that program as the set's owner is the correct declaration, because a second copy of a list is a second thing to keep current.

## Relationships
- **REFERENCES**: [[a-test-gate-that-exists-must-gate]]
- **REFERENCES**: [[a-claim-about-running-state-is-measured-against-the-running-system]]

## Justification

The failure is not hypothetical and it is not rare. `public/scripts/check-restart-disclosure.sh` shipped as a hand-kept list of surfaces; three consecutive grading rounds each found one more surface the list did not know about, twice a published crate readme in a class the list already covered. Its header carries the conclusion it was rebuilt on: a list can only hold what its author remembered.

The same shape recurred at campaign scale. A grading flywheel ran eight rounds on one plan and twelve on another, against a campaign norm of one to two, because each round's inventory of describing surfaces had been assembled from memory and each grade discovered one more surface outside the previous round's reach. A separate plan took seven grades against a norm of two for the same reason over an open class of input shapes rather than surfaces.

The cost of the discipline is small and the cost of skipping it compounds: every missed member is found by a later round, and every later round costs a full verification pass.

This is [[a-test-gate-that-exists-must-gate]] read one level up: a gate whose scan set is smaller than its class exists without gating the part it never sees.


A gate's scan set can be right while the gate is still blind, and only one test shape catches it. Planting a token on each surface class proves the MECHANISM: the reader reaches the class and the pattern fires. It says nothing about whether the pattern matches the wording a real finding actually used. Reproducing each closed finding on its own class, in the pre-correction wording recovered from git, proves the REACH. Recorded instance: the retired-form gate's plant-a-token self-test was green on every class while the gate was silent on four of the five verify-claim findings the sweep had just closed, because those surfaces said "read-only" and "never mutates" where the pattern expected the word "verify" nearby. The fixtures found it in one round; six earlier rounds of reading source files had not.

A retired term is a claim only in its predication, and proximity is the wrong proxy for it. The same word can be the defect or ordinary correct prose depending on what it qualifies: "read-only" is accurate for a mount, a mem, a command and a commit-mining run, and wrong only where it qualifies a verification noun. Measured rather than argued: matching a bare "read-only" anywhere near a subject word produced 26 findings on the workspace and every one was a false positive, while requiring it to qualify fidelity, verification, verify or a measurement surface produced all five real findings and nothing else across 1751 surfaces. A pattern tuned by proximity buys exemptions; a pattern tuned by predication buys coverage.

## Exceptions

A list is acceptable where the class is closed by construction and the closure is itself mechanically checked: an enum's variants, a generated file set whose generator is the walk. The distinguishing question is whether a new member can join the class without the list's author acting.

A walk that reaches zero members is a failure, not a pass. `scripts/check-describing-surfaces.py` reports it as one, because the recorded instance is a scan entry naming `public/engineering` in a repository where `public/` is a gitlink: the entry matched nothing and reported clean for as long as it existed.

## Consequences

A checker written this way costs more up front (the walk, the exemption list, a reason per exemption) and stops costing after that: a new member is covered the day it lands, and a departed member is reported at the moment it departs rather than silently skipped.

Exemptions become the reviewable surface. Each one is a claim about a file, and it is reviewed where the walk is rather than forgotten somewhere else.

Where a class's substrate is absent from a given checkout (the private mem-repo in a public CI lane), the class is reported as unmeasured by name and never counted clean, which is [[a-claim-about-running-state-is-measured-against-the-running-system]] applied to coverage.


A hand-kept list fails in two directions, not one, and the second is easy to miss. It can omit a member the class contains, which is the failure everything above is about. It can also name a member that does not exist, and nothing notices, because the check that reads the list only ever asks whether each entry is present somewhere else. Recorded instance: the MCP server instructions, the most-read prose the project ships, advertised five error codes (`MEM_NOT_WRITABLE`, `MEM_BRANCH_MISSING`, `VCS_ERROR`, `EXPORT_ERROR`, `WORKSPACE_SCHEMAS_ERROR`) with no construction site anywhere in the workspace, while the same hand-kept list was missing two codes the engine can produce. An agent was told to expect refusals that cannot arrive, and given no entry for two that can. Both directions are cheap to derive once the class has a mechanical definition, and neither is detectable from the list alone.


One source rendered several ways needs the same treatment, and reading the source is the check that cannot do it. memstead.ai's entry page, its agent runbook and the header of its whole-graph document are three renderings of one link table, and the table exists so they cannot drift. Comparing the table against itself proves nothing: what drifts is the prose written around it. Rendering all three and differencing the URLs they actually contain found four surfaces named on the entry page and nowhere else a reader might arrive, including the discovery manifest and the one-line installer. A 2026-08 sealed newcomer had already missed two working surfaces for this reason. The rule is the same one level down: compare what is rendered, never what is declared.
