---
type: concept
maturity: established
abstraction_level: abstract
tags: memstead, core
---

# Workspace

## Definition

A workspace is a named runtime context that lists a set of vault mounts (see [[mount]]) and the policy that governs them collectively.

## Explanation

The engine boots one workspace per process; every agent session, MCP invocation, or CLI command operates against that workspace's mount set. The workspace holds which vaults are mounted and how (read or write, eager or lazy, cross-linkable or isolated), the directed allowlist for [[wikilink]] edges between mounted vaults, and workspace-level policy. It does not own the [[schema]] or per-vault configs — those travel with each [[storage-backend]], so a copied [[vault]] resolves on its own.

## Boundaries

- A workspace is not a folder: it may carry zero, one, or many mounts (see [[mount]]).
- A workspace is not a [[vault]]: a vault exists independently, and any workspace may mount it under its own capability and policy.

## Significance

The workspace is the atomic unit for engine boot — it defines the visible vault universe for any agent operating against it.
