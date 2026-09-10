---
type: decision
created_date: 2026-09-10T14:07:42Z
last_modified: 2026-09-10T14:07:42Z
status: accepted
decided_on: 2026-09-10
deciders: operator (in-session), implementing agent
scope: subsystem
tags: prune, never-clobber, binding-format, projection-pipeline, simplification
---

# Prune proposes and never merges: the never-clobber guarantee vocabulary is removed

## Decision
We will remove the never-clobber / conflict-flag guarantee vocabulary from prune and from the binding format. A binding's `prune` block is an empty object that enables deletion proposals; the `guarantee` key, the `PruneGuarantee` type, `prune_guarantee_for_medium`, the `PRUNE_GUARANTEE_UNSUPPORTED` refusal, the `clean-delete` disposition, the proposal's `base_retrievable` flag and the fidelity report's `base-version-unretrievable` degradation are gone. Prune keeps two dispositions, `proposed` (both sides shown, the agent decides) and `derived-flagged`, and the sync brief states the rule the agent decides by: delete when the subject is gone from the source and no knowledge mem cites it, keep the entity as a frozen historical record when one does. A record written with the old `guarantee` key still loads; the key is ignored, because conflict-flag was already the only behaviour. Base retrievability stays a fact the capability matrix and the fidelity report state about a source medium; nothing gates on it. Realized in [[engine--prune-proposals]].

## Context
Prune was built (backlog-sweep plan 09b, August 2026) with the vocabulary of a git merge: base, ours, theirs, a three-way comparison that would tell a clean removal from a model-side edit. The wiring attempt found no mechanical source for the merge input, and the operator decided on 2026-08-19 that conflict-flag-only is the accepted posture, not a temporary degradation ([[engineering--the-never-clobber-merge-input-has-no-mechanical-source-build-writes-are-unattributed]]). The code was not rewritten after that decision: `prune.rs` kept saying "not wired this cycle" and "a hand edit", the generated binding reference advertised `never-clobber` for every git-backed medium, and the blind battery of 2026-09-06 graded exactly that sentence as design intent presented as behaviour. The 2026-09-10 review of the outside-in analysis traced the confusion to the vocabulary itself. The merge picture does not fit prune: a source artifact and the agent-authored entity about it share no common ancestor a merge could compare, and the sync loop edits anchored entities as its ordinary work, so "has the model side changed since the build" has no answer and no meaning as a guard. In Memstead every mutation comes from an agent through the engine; the protection the vocabulary promised is already given by the provenance classes (an `authored` entity is never a prune target) and by the rule of [[engineering--a-modelled-subject-that-left-the-tree-is-deleted-unless-a-knowledge-mem-cites-it-then-kept-as-a-frozen-historical-record]].

## Consequences
- The binding format changes (pre-1.0): `prune` is `{}` or absent; the JSON schema, its examples, the generated binding reference and the plugin's sync skill say so.
- A public promise that the engine did not keep is gone, with no gate left behind to keep prose and behaviour aligned: the vocabulary that could diverge no longer exists.
- The attribution channel the August memo sketched (a `Binding:` trailer on mutations enacted from a brief) stays a possible future design, but no code or vocabulary waits for it.
- Graph merging between two writing machines is a separate question, decided the same day in its own record; nothing here addresses it.
- One workspace binding (`flagship/public-docs`) still carries the retired key until it is rewritten through `projection edit`; it loads unchanged.

## Relationships
- **MOTIVATED_BY**: [[the-never-clobber-merge-input-has-no-mechanical-source-build-writes-are-unattributed]]
- **REFERENCES**: [[the-never-clobber-merge-input-has-no-mechanical-source-build-writes-are-unattributed]]
- **REFERENCES**: [[a-modelled-subject-that-left-the-tree-is-deleted-unless-a-knowledge-mem-cites-it-then-kept-as-a-frozen-historical-record]]
- **REFERENCES**: [[engine:prune-proposals]]

## Options

- Keep the vocabulary and mark the docstring "accepted posture": rejected, the dead option would stay in the code and in the public reference with the same confusion for the next reader.
- Wire the merge input via an attribution channel: rejected by the operator on 2026-08-19 as over-engineering for a mixed-write flow the product does not encourage, and re-rejected on 2026-09-10 once the category error (no common ancestor) was named.
- Remove the vocabulary (chosen).
