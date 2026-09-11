---
type: decision
created_date: 2026-09-11T03:19:44Z
last_modified: 2026-09-11T03:19:44Z
status: accepted
decided_on: 2026-09-11
deciders: architecture-seams bundle plan 08 (agent, under the standing engine-change rule)
scope: subsystem
tags: kernel, write-gate, refactor, module-layout
---

# The write gate is cut by phase: resolve, validate, compose, stage, commit, outcome

## Decision
We will keep the kernel's two write-gate files, create and update, as directory modules cut by the phases every mutation shares, one file each, run in this order: resolve (canonical inputs, mount and capability, schema and type, and for update the entity and the optimistic lock), validate (the input gates), compose (the entity or the delta applied, the alias pass, render, the composed-state gates, the no-op and dry-run outcomes), stage (the write and its sidecars into the mount's pending buffer, one function the single and the batch path share), commit (commit, provenance, store application), outcome (the wire shapes an operation ends in without a commit, and the batch receipts); batch drives the same phases for the batch form, and the tests sit beside them split by concern. The cut moved code verbatim into phase functions that run in the original order, so every gate fires where it fired before ([[engine--create-mutation]], [[engine--update-mutation]], [[engine--batch-update-atomic-mutation]]).

## Context
On 2026-09-10 the two files measured 7,990 and 6,870 lines, the largest in the kernel and the gate every write passes; a single 660-line and a 955-line prepare function held resolve, validate and compose in one body. The plan named five phases (resolve, validate, stage, commit, compose the outcome). The mutation module already uses stage for the backend's pending buffer (stage_anchors_sidecar, stage_derivation_sidecar, discard_all_pending), so entity composition took its own name, compose, rather than overloading stage; outcome kept the fifth. The evidence that the cut changed nothing observable is a golden of one fixture mutation sequence over the MCP binary (create, update, relate, delete, a hash mismatch, a reload notice from a sibling CLI write; 33 responses with timestamps, hashes and SHAs normalised), byte-identical before and after each split, beside the unchanged wire-shape, tool-surface, axis-coverage and hash-mismatch tests.

## Consequences
- A change inside one phase touches one file of a few hundred lines; the largest files left in the two modules are test files under 2,000 lines.
- The mutation-path entities in the engine mem anchor the two directories at tree grain, so a later edit anywhere in a module drifts one anchor and the entity re-verifies as one; the engine binding verified CLEAN after the inventory.
- The single and the batch path stage through the same function but keep their own predicate for when the anchors sidecar is staged (the single path only when the merge changes the sidecar, the batch whenever the item carries anchors or unsets); the cut preserved that divergence rather than deciding it, and a later change may fold it.
- The batch refusal and receipt builders exist once per mutation (create, update) and a third time in relate; folding them across mutations is out of this cut's scope.
- The projection crate gained its first crate-boundary suite in the same plan, so the maintenance writer is covered from outside as well as by its in-module tests ([[engine--memstead-projection-crate]]).

## Relationships
- **REFERENCES**: [[engine:create-mutation]]
- **REFERENCES**: [[engine:update-mutation]]
- **REFERENCES**: [[engine:batch-update-atomic-mutation]]
- **REFERENCES**: [[engine:memstead-projection-crate]]

## Options

- Cut by entity kind (a create for specs, a create for decisions): rejected, the kernel is schema-agnostic and knows no kinds; phases are the seams the operations share.
- One prepare file holding resolve, validate and compose: rejected, a thousand-line file with no seam between the input gates and the composed-state gates.
- Reuse stage for entity composition to keep the plan's five names: rejected, the name already means the backend's pending buffer in this module and a second meaning beside it would mislead the next reader.
- Cut first, split the tests later: rejected, the two files were three quarters tests, and a code-only cut would have left two 6,000-line test files beside 400-line phase files.
