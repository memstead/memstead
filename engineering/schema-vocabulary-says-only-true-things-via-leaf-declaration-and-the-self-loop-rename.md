---
type: decision
created_date: 2026-08-08T11:37:33Z
last_modified: 2026-08-08T11:37:33Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust bundle plan 06; rename pre-approved 2026-08-08)
scope: subsystem
tags: schema, vocabulary, leaf, rename, agent-surface
---

# Schema vocabulary says only true things via leaf declaration and the self-loop rename

## Decision
Two vocabulary-honesty acts. **Leaf declaration:** a type may declare `leaf: true` — its entities are terminal by construction, so the orphan axis exempts their edge-less entities and health reports them as a separate visible `leaf_entities_by_type` population instead. Leaf means "no edges required", never "edges forbidden". **Rename:** `propagating_relationships` — whose name promised propagation the engine never performed — is `no_self_loop_relationships` (optional), naming its single real effect: `memstead_relate` refuses a self-loop on the listed rel-types. The old key refuses at authoring/install load with a typed error naming the new key; SEALED content (compiled built-ins, installed refs) loads with the old key translated and every served payload emits only the new name — the install-time-strict / sealed-tolerant doctrine. Broken authored schema packages in general now quarantine only the mems that pin them (the schema-dir walk skips-and-records; the pin miss carries the load failure as its typed reason) — never the workspace. Every built-in carrying the old key ships a new version with the new spelling (default@1.1.0, engineering@0.2.0, ingest@0.3.0 — which also declares its three entry types leaf — planning@0.3.0, project@0.2.0, software@0.2.0); old versions stay sealed and loadable per the retention manifest.

## Context
For an agent-first surface, names and declarations are API. The vocabulary could not SAY something true (the shipped ingest schema said "leaf observations" in prose at four sites while health reported its entries as orphans — 7 false orphans per field report, noise masking real debt) and it SAID something false (`propagating_relationships`: two external schemas were designed around the propagation myth; two channel findings were raised and withdrawn on it). The agent-toolbox constraint-vocabulary plan had recorded the revisit clause — "revisit only when a real holding writes a check none of the five express" — and the leaf declaration is exactly that sixth declarative form. The rename's collision with the retention manifest (sealed bytes must keep loading) forced the strict/tolerant split rather than an unconditional refusal.

## Consequences
Migration off the old key is one mechanical rename per authored schema, prompted by a typed error — never a silent alias that would keep the myth alive. A binary upgrade cannot strand a workspace on this break: old-key authored schemas quarantine their pinning mems with the rename error as the reason while healthy mems serve (plan 04's posture made binding). Fresh workspaces (`memstead quickstart`) pin the current default generation. Bare built-in names are now ambiguous everywhere (every built-in ships two or more versions) — pins are explicit, which the exact-pin doctrine already demanded. The graph workspace's own mems still pin old sealed versions and keep working; repinning them to the new generation is optional follow-up, not forced migration.

## Options

A deprecation alias (accept + warn) was rejected — pre-1.0, and the field's entire problem was its surface outliving its truth. Silently dropping leaf entities from health entirely was rejected as the inverse dishonesty. An unconditional old-key refusal was rejected because sealed built-in bytes must keep loading (retention manifest) — the strict/tolerant split preserves both honesty and retention.

## Notes


