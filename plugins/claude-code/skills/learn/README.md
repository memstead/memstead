# Learn — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- load knowledge from the graph into the current conversation context
- the agent reads entities so it has internalized knowledge before starting work
- knowledge stays in the main context — no forking

## Modes

- targeted: search by keywords, read top matches
- research: analyze current task, formulate queries, read relevant entities
- interactive: ask the user what to search for

## Reading discipline

- always read full entities via memstead_entity — search results are not enough
- deduplicate — the same entity may match multiple keywords, read it only once
- mem selection is mandatory — always ask the user first

## Output

- no lengthy summary — a one-line confirmation of what was read is enough
- the value is in the knowledge being in context, not in a summary of it
