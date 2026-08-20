---
type: principle
created_date: 2026-08-20T02:26:12Z
last_modified: 2026-08-20T02:26:12Z
authority: accepted
universality: domain-wide
tags: ingest, briefs, claims-honesty, agent-surfaces
---

# A brief states the conditions of its own execution

## Statement
A run-brief is an instruction handed to an agent that cannot inspect the workspace before acting on it. So the brief carries the conditions its own instructions depend on: if the destination mem does not exist, it says so and names the command that creates it; if a rule it states is enforced only under conditions the brief does not itself establish, it says which. It never describes a precondition as satisfied because the common case satisfies it.

The test is not "is each sentence defensible" but "could an agent following this brief exactly be surprised by the workspace?" A surprise means the brief asserted something the agent had no way to check and no reason to doubt.

## Scope
Governs every engine-rendered brief — build, sync, verify — and any surface that hands an agent a prompt it will act on without first verifying. Does NOT govern human-facing receipts, which are read by someone standing in the workspace who can look; their discipline is [[engineering--a-command-a-surface-prints-is-built-never-formatted]].

## Relationships
- **REFERENCES**: [[a-command-a-surface-prints-is-built-never-formatted]]

## Justification

Two live examples, both found by executing the loop rather than reading it. The build brief's whole mandate is "create, update, relate and delete entities in the destination mem" — rendered for a binding scaffolded before its mem existed (an order `projection init` deliberately allows), it described a destination that was not there, and the agent discovered this on its first write. And the provenance block promised that an `source` name outside the binding's declared list "refuses INVALID_ANCHOR with the declared names in the recovery payload"; that check fires only when the anchor also carries its producing binding hash, which the brief never asks the agent to set. Following the brief exactly, an undeclared source name either refused on the artifact path with no mention of source names, or was accepted outright.

Both are the same failure: a claim true of the maintainer's mental model, false of the workspace the agent was actually handed. An agent has no way to tell a brief that is wrong from one that is right — that asymmetry is what makes the obligation one-sided.

## Exceptions



## Consequences

- A conditional enforcement is stated with its condition, or not stated as enforcement at all. Promising a refusal that may not fire is worse than promising nothing: it invites the agent to rely on a gate that is not there.
- Where the engine can detect the unmet precondition, it names it in the brief rather than refusing to render. Refusing would break the scaffold-then-create order the CLI supports; the honest render keeps the workflow and removes the surprise.
- The check that catches this class is executing the brief's own instructions against a fresh workspace, not proofreading it. Both defects here survived every reading and failed on first execution.
