---
type: decision
created_date: 2026-09-06T09:57:01Z
last_modified: 2026-09-06T09:57:01Z
status: accepted
decided_on: 2026-09-06
deciders: claims-in-sight plan (blind grading of 2026-09-04); agent decision under the engine-changes rule
scope: subsystem
tags: sync, brief, anchors, mentions, advance
---

# The sync brief steers by mention as well as by anchor, and a mention-steered artifact waits for the agent's disposition

## Decision
Under its changed slice, the sync brief lists for every changed artifact the destination entities that anchor it and, headed as steered by mention, the entities whose prose names the artifact without anchoring it: by path, under the same two spellings the anchor resolver accepts, or by a symbol the change defines or removes, named in an inline code span and read lexically from the file's definitions against its baseline (a keyword followed by an identifier, plus enum variants; ubiquitous names such as `new` or `default` and anything under four characters never steer). The index is built from entity bodies at brief time and nothing is stored in the mem; matching is lexical and deterministic, no embedding and no model call. An entity that anchors the artifact is listed once, under anchors. The mention lines are the brief's heavy content: `projection brief --sync` takes a token budget and an include key, and when the lines do not fit they degrade to one line stating the count of mention-steered entities and the include hint, never to silence, while the anchored lines always ship. At `projection advance`, an artifact with mention-steered entities is no longer auto-disposed by its anchors: its disposition is the agent's after the walk, so a pass cannot close with a mentioned entity unjudged. This is the steering half of [[engineering--a-claim-about-an-artifact-the-entity-does-not-anchor-is-a-warn-level-verify-finding-matched-by-the-source-join]]: verify names the exposure, the brief hands it to the pass that repairs it.

## Context
Found 2026-09-04: the engine mem's reload spec was synced twice on 2026-09-02 for drifts in its anchored files while its constraint about the folder backend, falsified on 2026-07-16 in a file it never anchored, survived both passes; the storage-backend concept and the filesystem-writer spec anchored nothing and were never presented. The brief's changed slice listed paths only, the auto-worked rule at advance disposed an artifact on the strength of any anchoring entity's write, and the brief's stale-claim paragraph asked the agent to search the mem for changed facts in its head, the discipline that missed the same sentence three times. On the engine binding's slice of 2026-09-06 the block lists 98 mention-steered entities over 42 changed artifacts beside the anchored ones.

## Consequences
A changed file now reaches every entity that talks about it, anchored or not, in the one pass that repairs claims; the agent walks mention-steered entities like anchored ones and anchors the claim while there, so the mention class shrinks with use. The advance gate is stricter by exactly the artifacts that carry unanchored claims. The brief grows by the block; under the default budget the engine binding's block fits, and a larger mem sees a count line and the include hint instead. Identifier steering is lexical: a symbol renamed in a file steers every entity naming the old or new name, which is the intended over-approximation, judged by the agent.

## Relationships
- **REFERENCES**: [[a-claim-about-an-artifact-the-entity-does-not-anchor-is-a-warn-level-verify-finding-matched-by-the-source-join]]

## Options

Auto-anchor every mentioned file: rejected, an anchor is a claim of provenance the author makes; the engine may point the author at what they named, never invent the claim. Keep the in-head search as the sole mechanism: rejected, the engine can do the search and present the result. Widen by full-text search over the diff's words: rejected as too noisy on common words; paths and defined identifiers are the signal, and the ubiquitous-name filter keeps the identifier half honest. Store the mention index in the mem: rejected, it is derivable from bodies at brief time and would drift.
