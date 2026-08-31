---
name: remodel
description: >
  Restore a mem's model truth — is the mem still cut right for its subject:
  does every obligation have exactly one home entity of the right type, does
  substance sit in its declared sections, is the graph wired. Where /sync
  repairs what entities SAY, /remodel repairs what the mem IS. Run `--all` on
  a loop like /sync: a cheap signal scan walks every mem and descends into
  the expensive round only where signals justify it (quiescent otherwise —
  the loop ends itself). Target one mem with `remodel <mem> [<cluster>]`, or
  measure without writing via `--scan [<mem>]`. The round derives a target
  inventory from contract plus source, has it adversarially checked, diffs
  it against the live entities, rebuilds conservatively, and brackets big
  rebuilds with a before/after reconstruction probe. Rare and deliberate by
  design — the refactoring beside /sync's bugfixing.
allowed-tools: Bash, Read, Grep, Glob, Agent, mcp__memstead__memstead_schema, mcp__memstead__memstead_search, mcp__memstead__memstead_entity, mcp__memstead__memstead_overview, mcp__memstead__memstead_create, mcp__memstead__memstead_update, mcp__memstead__memstead_relate, mcp__memstead__memstead_rename, mcp__memstead__memstead_delete, mcp__memstead__memstead_check
argument-hint: "[--all | --scan [<mem>] | <mem> [<cluster>]]"
---
# Memstead Remodel

A mem is a typed model of its subject, measured against its CONTRACT:
schema (types, section contracts, relationship vocabulary — including
which section is a type's own definition test), binding intent where a
binding exists, writeGuidance, and subject. /remodel asks whether the
model still fulfils that contract — the cut itself, not just the
claims — and rebuilds where it does not. Two storeys, never mixed:
storey 1 (mem vs contract) is repaired here; storey 2 (the contract no
longer fits the source) is REPORTED to the operator, never repaired
around.

## Steps

1. `WS="$(node "${CLAUDE_PLUGIN_ROOT}/scripts/binary-version.mjs" root "$(pwd)")"` —
   every `memstead` call carries `--workspace "$WS"` (outside a plugin
   install, where that script does not exist, let `memstead` resolve
   the workspace by its own directory walk from cwd). Parse `$ARGUMENTS`:
   `--all` → step 2 over every writable mem, then work the single
   worst-signalling cluster (one round per invocation — loop-friendly);
   nothing signals → report quiescence in one line and stop (under a
   recurring loop, that ends the loop). `--scan [<mem>]` → step 2
   report only, no writes. `<mem> [<cluster>]` → steps 2-8 for that
   target, cluster hint optional. No argument → ask.

2. SIGNAL SCAN (cheap, read-only, per mem). Build the shape picture:
   per-cluster entity type distribution (collapse toward the schema's
   last-resort type signals a flat model); for bound mems the
   file-to-entity map (large source files no entity anchors or names;
   files claimed by 2+ entities and owned by none — read the anchors
   sidecar and Realization-style path claims); entities with zero
   outgoing relationships; empty definition-test sections (e.g. a
   decision type whose rejected-alternatives section holds nothing);
   repeat-repair hotspots from the check ledger. Fold zero-degree
   entities into their nearest SUBJECT before ranking — a cluster
   partition computed from the edge graph drops exactly the entities
   whose defect is "no edges", hiding the defect the round exists to
   repair. Rank candidates by BOTH uncovered mass AND model shape — a
   fully anchored cluster can still be badly cut. Report the ranking
   with evidence one line each.

3. CONTRACT FIRST. For the chosen cluster's mem, read the full schema
   prose (`memstead_schema`, verbosity full, scoped to the types in
   play), the binding intent, writeGuidance, and subject. The
   contract, not taste, decides every judgment below.

4. TARGET INVENTORY, derived blind — by a READ-ONLY SUBAGENT
   restricted to the contract and the source tree (you ran the scan,
   so YOUR blindness is spent; the subagent's is not, and it may
   refute your cluster hypothesis — let it). It reads the cluster's
   SOURCE deeply (module docs, public surfaces, versioned formats —
   the whole cluster, not samples; for graph-authoritative mems the
   repo and its git history are the source) without ever enumerating
   live entities, and writes the target inventory: which entities this cluster needs, in
   which types, at which grain, with which main relationships — one
   line each with the owned artifacts. Apply the schema's cut rules
   strictly: one entity per obligation/concept, never file-named, the
   specific types where the schema provides them, the generic type
   last. Name deliberate non-entities.

5. ADVERSARIAL CHECK. Spawn ONE read-only subagent (disallow Write and
   Edit) with a refutation mandate over the inventory: per target —
   justified from contract plus source? duplicate of a live entity's
   legitimate scope (now it enumerates the live mem)? right type and
   grain? Discards need grounds. Only the surviving inventory becomes
   the worklist; where you disagree with a discard, note why and
   follow your own judgment, logged.

6. DIFF AND REBUILD, conservatively, via the MCP mutation tools only:
   missing → CREATE under the two-gate discipline (gate 1: read the
   nearest neighbours in full first — an owner gets extended, never a
   sibling minted; gate 2: every symbol-level claim traced to a source
   line before the write, code blocks copied from source, never
   composed — then mechanically verify: every backticked identifier in
   the new body must grep in the cluster's source, zero misses);
   straddles → SPLIT with relationships re-pointed and anchors
   moved, then the RECEIVER CHECK before the split counts as done:
   walk the predecessor's body claim by claim and show each factual
   statement has a receiving owner among the successors or is
   demonstrably false — a split that sheds a true statement is the
   one collateral this round can cause, and it was measured once;
   wrong type → today this branch REPORTS, always: no surface can
   retype (`type` is read-only and delete+create breaks incoming
   refs), so a mis-type diagnosis goes to the storey-2 report AND a
   `memstead_check` failed-verdict on the entity, so the next round
   inherits it instead of re-deriving it — never fake a retype;
   dissolved subjects → the schema's own history/supersession forms,
   never deletion of recorded knowledge, and never RENAMING or
   rewriting a frozen historical record onto its live successor
   (author the successor, link the supersession, leave the record
   standing — a frozen entity kept for its citations must keep
   describing what its citers cite);
   substance in catch-alls → relocate to declared sections; maintenance
   narration in normative sections → apply corrections silently (git
   carries the archaeology), supersession where the schema prescribes
   it. NO TRUE STATEMENT LOST: before removing or moving text, its
   factual content is preserved or demonstrably false against the
   source. Boundary claims ("the only X", "exactly N") only after
   tracing callers, never inferred. Anchor what you create or move.
   Edge note: `REFERENCES` forbids manual authoring by design — it is
   minted by body wiki-links, never declared explicitly; declare only
   the typed relationships the schema's vocabulary allows.

7. RECONSTRUCTION BRACKET, for a big rebuild (creations plus splits
   touching ten PERCENT or more of the cluster's entities — a share,
   not a count): BEFORE the rebuild,
   derive ~10 service tasks from the SOURCE (questions an agent
   serving the subject must answer), have a fresh read-only subagent
   answer them from the mem alone, score against the source; AFTER
   the rebuild, same battery, fresh subagent, score again. Both
   numbers go in the report. A small round may skip the bracket and
   say so.

8. REPORT AND GATE. Close with: the signal evidence, the inventory
   with adjudication verdicts, every write one line each with its
   derivation, the bracket scores where run, deliberate leaves with
   grounds, and the storey-2 findings routed to the operator.
   Git-branch mems are committed by the engine per write;
   FOLDER-BACKED mems land as files plus ledger rows and stay
   uncommitted — and a rename in ANY mem can rewrite incoming
   cross-mem links in a folder mem the round never targeted. Name
   every repo left with uncommitted changes explicitly in the
   report; PUSHING, committing folder-mem state, and any cross-repo
   adoption stay with the operator — present, never push.
   Record a `memstead_check` verdict on every entity you verified or
   rebuilt so /sync's sweep prioritizes correctly afterwards.

## Rules

- Rare and deliberate: one cluster per invocation, signals first —
  /remodel never becomes a second sweep. /sync stays the sole
  continuous maintenance writer; /remodel is the explicit act.
- Storey-2 findings are reported, never repaired around; the contract
  as it stands binds the round.
- Growth honesty: every creation traceable to the surviving
  inventory; when in doubt, don't create. Deletion only through the
  schema's own history forms.
