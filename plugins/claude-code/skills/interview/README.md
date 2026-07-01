# Interview — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- capture knowledge from a domain expert through conversation
- the user is the expert, the agent is the scribe
- one question at a time, breadth first then depth

## Conversation quality

- ask ONE specific, contextual follow-up per message — never "anything else?"
- only capture what the user actually says — never invent or assume
- if something is unclear, ask — don't guess
- if something contradicts an existing entity, pause and ask which is correct
- adapt language to the user — structure keywords stay English

## Entity creation

- search before creating — never duplicate
- summarize back to the user and wait for confirmation before creating
- track which schema fields are covered and which are open — ask about open ones

## State management

- interview mode persists via the mem's `.memstead/interview-active` state file — the SKILL writes it and the `inject-context` hook reads `<mem-dir>/.memstead/interview-active`; both must agree
- the UserPromptSubmit hook re-injects interview rules every turn
- clean up the state file when the interview ends

## writeGuidance-driven

- uses the writable mem's `writeGuidance` for granularity, extraction, and abstraction rules
- loaded via `memstead_health { include_config: true }` — never reads `.memstead/config.json` directly
