---
type: principle
created_date: 2026-08-19T07:02:07Z
last_modified: 2026-08-19T07:02:07Z
authority: accepted
universality: domain-wide
tags: diagnostics, honesty, scaffolding
---

# The engine never calls its own output a typo

## Statement
A diagnostic that lints user input must exempt content the engine itself wrote. When a lint fires on scaffold-written defaults, every fresh surface opens with a false alarm — and false alarms train dismissal, which mutes the lint exactly where it matters (a user's real mistake). The dual obligation binds both ways: the engine never flags its own output as a user error, and it never mutes a user-authored entry to achieve that.

## Scope
Every warning or lint that judges configuration or content which the engine's own scaffolds also produce. First applied to the brief's dead-deny lint: `projection init` writes default hygiene `deny_paths` that can never match on a git-enumerated source, and the lint called them "usually a typo" on every fresh binding (WOENENN field finding, 2026-08-18). The collector now exempts exactly the scaffold default set; user-authored zero-match entries keep the loud warning.

## Relationships
- **REFERENCES**: [[a-test-gate-that-exists-must-gate]]

## Justification

The first external field user's first brief opened with the engine calling its own scaffold output a typo — the fastest possible way to teach a user that warnings are noise. A lint's value is its signal-to-noise ratio, and the engine controls one side of that ratio completely: it knows what it wrote. Related discipline: [[a-test-gate-that-exists-must-gate]] — a surface's claims must be true of the surface itself.

## Exceptions



## Consequences


