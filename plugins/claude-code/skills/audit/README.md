# Audit — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- detect where entities have drifted from their realizations (code, schemas, etc.)
- report findings with evidence — never claim drift without reading the actual code
- read-only — never modify entities, only report

## Rigor

- every drift claim must have evidence: code snippets, commit hashes, concrete differences
- respect abstraction levels — a variable rename is not drift for a high-level entity
- when uncertain, report as "suspect" not "drifted"
- git history is a hint, not proof — always read the actual code to confirm

## Adversarial reasoning

- cross-reference entity claims against code with skepticism
- check: do claimed data structures still match? were new capabilities added? were constraints removed?
- treat commit messages with skepticism — "fix bug" could mean anything

## Output

- findings grouped by severity: critical drift, structural gaps, broken references, suspect
- suggested fixes for each finding — but never apply them
- if nothing found, say so explicitly

## Modes

- default: cheap signals first (health/drift), then deep analysis on flagged entities
- full: validate every entity regardless of signals
- single: deep analysis of one specific entity
