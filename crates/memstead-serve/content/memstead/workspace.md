---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Workspace

## Definition

A workspace is a named runtime context that lists a set of mem mounts (see [[mount]]) and the policy that governs them collectively.

## Explanation

The engine boots one workspace per process; every agent session, MCP invocation, or CLI command operates against that workspace's mount set. The workspace holds which mems are mounted and how (read or write, eager or lazy, cross-linkable or isolated), the directed allowlist for [[wikilink]] edges between mounted mems, and workspace-level policy. It does not own the [[schema]] or per-mem configs — those travel with each [[storage-backend]], so a copied [[mem]] resolves on its own.

## Boundaries

- A workspace is not a folder: it may carry zero, one, or many mounts (see [[mount]]).
- A workspace is not a [[mem]]: a mem exists independently, and any workspace may mount it under its own capability and policy.

## Significance

The workspace is the atomic unit for engine boot — it defines the visible mem universe for any agent operating against it.
