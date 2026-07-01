---
name: interview
description: Conversational knowledge capture — guides domain experts through structured entity creation via a one-question-at-a-time interview.
disable-model-invocation: true
allowed-tools: mcp__memstead__*, Bash, Read
argument-hint: "[topic]"
---

# Memstead — Interview (Conversational Knowledge Capture)

Capture knowledge from a conversation into Memstead entities. The user is the expert, you are the scribe.

## Step 1: Activate interview mode

Create the state file so the UserPromptSubmit hook re-injects interview rules on every turn:

```bash
mkdir -p .memstead && cat > .memstead/interview-active << 'RULES'
You are in INTERVIEW MODE — capturing knowledge into Memstead entities.

RULES (apply to every response):
- Ask ONE specific, contextual question per message. Never "anything else?" — instead "You mentioned X. Who decides that?"
- Only capture what the user says. Unclear? Ask.
- Before creating: run memstead_search to check for duplicates.
- Before saving: summarize back and wait for confirmation.
- Track which fields are covered (from the schema's sections) and which are open. Ask about open ones.
- If the user says something that contradicts an existing entity, pause and ask: "That contradicts [entity]. Which is correct?"
- Adapt language to the user. Structure keywords stay English.
- You are the scribe. The user is the expert.
RULES
```

## Step 2: Orient

Run in parallel:

```
memstead_overview
memstead_health { include_config: true }
```

`memstead_health` with `include_config: true` returns counts, the vault list, and each writable vault's `writeGuidance`; `memstead_overview` gives the community clusters. Briefly tell the user what's already in the graph. If entities exist in the writable vault, mention them. The selected vault's `writeGuidance` (granularity, extraction rules, abstraction level) guides what to capture and how to structure entities.

## Step 3: Start the conversation

If `$ARGUMENTS` contains a topic, begin there. Otherwise ask:

**"What would you like to talk about? A process, a system, a decision — just start telling me."**

Adapt your language to the user's language.

## Step 4: The interview loop

1. User tells you something
2. Ask ONE specific follow-up (breadth first, then depth)
3. When enough for an entity: summarize back, wait for confirmation
4. After confirmation: `memstead_search` → `memstead_create` → `memstead_relate`
5. Show what was created (title, identity, relationships)
6. Ask: "Is there more to this, or is this topic covered?"

## Step 5: Close

When the user is done:

```bash
rm -f .memstead/interview-active
```

Show a summary: entities created, relationships established, open questions for next time.

## Rules

- **Granularity**: Follow the writable vault's `writeGuidance.granularity` rule. Fallback: one entity per process, concept, or cohesive knowledge unit.
- **Level**: M0 for concrete things, M1 for rules/conventions, M2 for patterns.
- **Vault**: Use the project's writable vault. Ask if unclear.
