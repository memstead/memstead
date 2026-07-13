---
type: principle
created_date: 2026-07-13T16:43:02Z
last_modified: 2026-07-13T16:43:59Z
authority: established
universality: domain-wide
tags: plugin, skills, workspace-discovery, front-door, onboarding
---

# Advertised front-door commands serve a fresh non-maintainer workspace

## Statement
The plugin's advertised front-door commands (`/ingest`, `/setup`, `/start`, and any slash command a fresh user is invited to run) MUST work on the workspace the shipped `init`/`quickstart` actually produces: a folder-backed workspace carrying only the engine marker `.memstead/workspace.toml`, with no legacy `.memstead.toml` and no dumpable mem-repo. A command that hard-fails with "workspace not found" or a missing-`workspace dump` error on that shape is a broken front door, regardless of working correctly on the maintainer's own mem-repo workspace.

## Scope
Every user-facing skill that discovers or loads the workspace before doing its work — the workspace-root walk-up, the plugin-config loader, and any engine-dump consumer sitting behind an advertised command. It does NOT govern internal/maintainer-only tooling or hooks that fire silently, which may assume richer workspace state.

## Relationships
- **GOVERNS**: [[plugin:ingest-workspace-loader]]
- **GOVERNS**: [[plugin:ingest-situation-brief-assembler]]

## Justification

The plugin ships to arbitrary machines and users who ran `init`/`quickstart` and got the default folder-backed shape; the maintainer's mem-repo workspace is the exception, not the norm. An advertised command is often the first thing a new user runs — failing there with a confusing marker/dump error loses them before they reach any value. Recognising only the legacy `.memstead.toml`, or treating an unavailable `workspace dump` as fatal, silently scoped these commands to the maintainer's own machine.

## Exceptions

A command may still degrade its RESULT when workspace data is genuinely absent — e.g. reporting an honest "no ingests found" — as long as it reaches that result cleanly rather than erroring. The invariant is about not hard-failing the command, not about manufacturing content that isn't there.

## Consequences

Workspace discovery accepts the engine marker `.memstead/workspace.toml` as a root (not only the legacy `.memstead.toml`). Loaders that consume the engine `workspace dump` catch its unavailability on a folder-backed workspace and substitute an empty dump, letting the store walk still surface any configured work, rather than aborting the whole command. Both realizations trace to this rule: the ingest workspace-loader's dump-degradation and the ingest front-door assembler's marker-aware root discovery.
