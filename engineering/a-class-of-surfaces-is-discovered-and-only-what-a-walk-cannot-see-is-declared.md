---
type: principle
created_date: 2026-08-26T17:28:42Z
last_modified: 2026-08-26T18:28:45Z
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

## Exceptions

A list is acceptable where the class is closed by construction and the closure is itself mechanically checked: an enum's variants, a generated file set whose generator is the walk. The distinguishing question is whether a new member can join the class without the list's author acting.

A walk that reaches zero members is a failure, not a pass. `scripts/check-describing-surfaces.py` reports it as one, because the recorded instance is a scan entry naming `public/engineering` in a repository where `public/` is a gitlink: the entry matched nothing and reported clean for as long as it existed.

## Consequences

A checker written this way costs more up front (the walk, the exemption list, a reason per exemption) and stops costing after that: a new member is covered the day it lands, and a departed member is reported at the moment it departs rather than silently skipped.

Exemptions become the reviewable surface. Each one is a claim about a file, and it is reviewed where the walk is rather than forgotten somewhere else.

Where a class's substrate is absent from a given checkout (the private mem-repo in a public CI lane), the class is reported as unmeasured by name and never counted clean, which is [[a-claim-about-running-state-is-measured-against-the-running-system]] applied to coverage.


A hand-kept list fails in two directions, not one, and the second is easy to miss. It can omit a member the class contains, which is the failure everything above is about. It can also name a member that does not exist, and nothing notices, because the check that reads the list only ever asks whether each entry is present somewhere else. Recorded instance: the MCP server instructions, the most-read prose the project ships, advertised five error codes (`MEM_NOT_WRITABLE`, `MEM_BRANCH_MISSING`, `VCS_ERROR`, `EXPORT_ERROR`, `WORKSPACE_SCHEMAS_ERROR`) with no construction site anywhere in the workspace, while the same hand-kept list was missing two codes the engine can produce. An agent was told to expect refusals that cannot arrive, and given no entry for two that can. Both directions are cheap to derive once the class has a mechanical definition, and neither is detectable from the list alone.
