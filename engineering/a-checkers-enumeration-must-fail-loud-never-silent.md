---
type: principle
created_date: 2026-08-28T02:58:43Z
last_modified: 2026-08-28T03:38:22Z
authority: accepted
universality: domain-wide
tags: gates, static-analysis, fail-closed, release
---

# A checker's enumeration must fail loud, never silent

## Statement
Where a checker contains a hand-written list of shapes it recognises, a shape missing from that list must produce a REPORT, never a pass. A list whose gap costs silence is a hole that closes only when someone happens to write the missing shape; a list whose gap costs noise is caught by the next run. Testable: delete any entry from the list and the checker must go red on an input that entry used to cover.

## Scope
Every static checker the project ships or runs as a gate: the release-dependency gate, the docs-vs-binary guard in [[engine--xtask-crate]], CI guards, hooks, and any future analyser that reads source it does not execute. It does not bind runtime code, where an unrecognised input often has a correct silent default.

## Relationships
- **GOVERNS**: [[engine:xtask-crate]]
- **REFERENCES**: [[engine:xtask-crate]]

## Justification

Thirty independent grading rounds against the release-dependency gate produced one repeated finding: every hand-written enumeration of shell shapes was a hole, and each was repaired twice by GROWING it before being deleted. Three were eventually deleted in favour of one shared word walker. The ones that survived did so because the fail direction was fixed, not because the list was completed. The strongest case is the wrapper-option pair: an option in neither the value table nor the flag table is treated as ambiguous and BOTH readings are emitted, so `sudo -X payload deploy.sh` reports the option value as noise and still reports the real command. The earlier policy chose one reading and lost the command whenever the guess was wrong. Also load-bearing: the same defect recurred because two walkers held two hand-rolled copies of the policy, so a repair kept landing in one and not the other. One policy, one implementation.

## Exceptions

A list that enumerates what the checker REFUSES rather than what it recognises already fails loud by construction and needs no inversion. A gate whose false positives would be unactionable (nobody reads its output) is not exempt; it is not a gate.

## Consequences

Three things follow. A checker may not resolve an ambiguity by picking the likelier reading when the unlikely one costs silence: it emits both and lets the declaration or a human close it. False positives rank equal to misses in grading, because a gate that calls correct code defective trains its readers to override it, so the noise a fail-loud list produces must stay small enough to act on. And the classification is inverted wherever it can be: enumerating the ways to reach a host failed three times because the set is open, while requiring every command to be classified and failing the unknown ones is closed by construction.
