# Start — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- quick dashboard of the graph's current state
- no task execution — just show what's there

## Output

- stats: node count, edge count, stub count, edge type distribution
- directory: entities grouped by vault with level and identity
- health: stubs and orphans listed, or confirmed absent

## Principles

- concise — this is a quick status check, not a deep dive
- run all queries in parallel for speed
