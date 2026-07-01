# Refactor — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- analyze the graph for structural issues and propose improvements
- never execute changes without explicit user approval

## What to look for

- granularity: entities with too many headings (split candidates) or too little content (merge candidates)
- orphans: real entities with zero relationships — likely missing connections
- stubs: unresolved wiki-link targets — either create the entity or fix the link
- incomplete entities: missing identity, purpose, or specifies
- relationship issues: wrong types, missing connections between related entities

## Presentation

- findings grouped by category, prioritized by impact
- each finding includes: what's wrong, why it matters, suggested fix
- skip categories with no issues

## Safety

- propose first, execute only after user approval
- one change at a time — show what changed after each step
- scoped mode: analyze one entity and its neighbors instead of the full graph
