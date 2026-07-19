---
type: decision
created_date: 2026-07-15T10:19:52Z
last_modified: 2026-07-15T10:26:03Z
status: accepted
decided_on: 2026-07-15
deciders: kara-maintainers
scope: org
tags: licensing, open-source, project
---

# Dual-License Under MIT and Apache

## Decision
We chose to dual-license Kāra under MIT OR Apache-2.0 and to canonicalize the project's public identity under the karalang/kara repository.

## Context
Kāra is preparing to ship publicly and needs an open-source license. The Rust ecosystem convention is MIT/Apache-2.0 dual licensing: MIT gives maximum permissiveness while Apache-2.0 adds an explicit patent grant, and downstream consumers pick whichever fits.

## Consequences
- LICENSE-MIT and LICENSE-APACHE added at the repo root; README updated to state the license and sharpen the AI-first positioning.
- Public references repointed to karalang/kara (repo, headings, docs).
- Aligns Kāra's licensing with the Rust-ecosystem norm its systems-language audience expects.

## Relationships
- **REFERENCES**: [[backend-first-v1-positioning]]

## Options

- MIT-only — rejected: no explicit patent grant.
- Apache-2.0-only — rejected: less permissive, and some ecosystems prefer MIT.
- MIT/Apache-2.0 dual — chosen: the Rust-ecosystem standard, covering both permissiveness and patent concerns.

## Notes

Part of preparing Kāra for public launch alongside the [[backend-first-v1-positioning]] story; the README rewrite that carried the license also sharpened the AI-first positioning.
