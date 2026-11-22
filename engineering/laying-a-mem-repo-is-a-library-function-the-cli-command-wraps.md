---
type: decision
created_date: 2026-11-22T13:15:47Z
last_modified: 2026-11-22T13:16:03Z
status: accepted
decided_on: 2026-11-21
deciders: operator (shared-engine bundle, go of 2026-11-21), implementing agent
scope: subsystem
tags: mem-repo,library,cli
---

# Laying a mem-repo is a library function the CLI command wraps

## Decision
We chose to make laying a mem-repo a public function of the git-branch crate. The CLI command is that function plus its own output and the ignore file of an enclosing repository; the function does not touch an enclosing repository. The initial commits keep their fixed signature and time, so a laid mem-repo is byte-reproducible.

## Context
The routine behind `memstead mem-repo init` was private to the CLI crate, while creating a mem was already an engine operation. A program that links the engine crates had to run the binary to lay a mem-repo, and a binary of another version than the crates it links is a skew that shows the day a release moves the local binary.

## Consequences
- A program that links the engine never runs a binary whose version can differ from its crates.
- The command and the function cannot drift: one is the other.
- The enclosing repository's ignore file stays the command's concern; a program that wants it writes it itself.

## Relationships
- **DERIVED_FROM**: [[engine-owns-mem-repo-state]]
- **ENABLES**: [[the-engine-crates-state-one-supported-library-surface]]

## Options

- Leave linking programs to run the binary: rejected, that is the version skew.
- An MCP tool: rejected, laying a repository is administration and the tool count is kept small on purpose.
- A public function the command wraps: chosen.
