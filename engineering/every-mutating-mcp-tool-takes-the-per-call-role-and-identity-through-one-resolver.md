---
type: decision
created_date: 2026-09-11T08:03:23Z
last_modified: 2026-09-11T08:03:23Z
status: accepted
decided_on: 2026-09-11
deciders: one-provenance bundle plan 02 (agent, under the standing engine-change rule)
scope: subsystem
tags: mcp, tool-shape, role, identity, provenance
---

# Every mutating MCP tool takes the per-call role and identity through one resolver

## Decision
We will give every MCP tool that mutates the same two optional parameters, `role` and `identity`, described by one constant each and resolved by one server-side resolver that validates both before any mutation and falls back to the session's declared defaults. The five lifecycle tools (mem create, configure, delete, set-schema, set-version) join the entity mutation tools on that shape, and the commit each produces carries the per-call values in its Role and Identity trailers. A tool that mutates and takes no per-call provenance is the defect, not a variant.

## Context
The entity mutation tools took `role` and `identity` per call since the independence gate landed, because a grader must record its checks under an identity distinct from the author's within one server session. The five lifecycle tools carried `note` only; since the seams follow-up (2026-09-11) their commits recorded the session's defaults, so a mem created or retired by a checker during a grade was attributed to the session's author. Their descriptions were copied per struct (six copies each, one of them abbreviated), and each handler resolved the pair inline. The principle that a guard on one write path exists on all of them with one implementation ([[engineering--a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]) names the class; the one-context decision ([[engineering--every-commit-the-engine-writes-carries-the-mutations-context-built-in-one-place]]) covers the engine side of the same provenance, this decision the wire side.

## Consequences
- Twelve mutating tools share one description of each parameter and one resolver; a new mutating tool declares the two fields with the constants and calls the resolver, and cannot drift on validation or fallback.
- The generated MCP reference lists the two parameters on the five lifecycle tools with the same text as on the entity tools; the parity matrix is unchanged because its rows are operations.
- The CLI needs no flag: its global `--role` and `--identity` already reach every verb through the session context.
- `memstead_mem_set_schema` accepts and validates the pair although it produces no commit today; when the pin moves into mem config its commit will carry them without a shape change.
- Every existing call shape keeps working: both parameters are optional and absent means the session's defaults, as before.

## Relationships
- **REFERENCES**: [[a-guard-on-one-write-path-exists-on-all-of-them-with-one-shared-implementation]]
- **REFERENCES**: [[every-commit-the-engine-writes-carries-the-mutations-context-built-in-one-place]]

## Options

- Leave the lifecycle tools on the session's defaults: rejected, the grading gate needs a distinct identity for lifecycle acts inside one session, and a lifecycle act under the wrong identity is the one that cannot be re-attributed later.
- A shared parameter struct flattened into every tool's params: not chosen; the constraint is one description and one resolver, and two fields with shared constants keep each tool's schema flat and readable in the reference.
- Validate the identity only at the ledger: rejected, the entity tools refuse INVALID_IDENTITY before mutating and the lifecycle tools must refuse identically.
