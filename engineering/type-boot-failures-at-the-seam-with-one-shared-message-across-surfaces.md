---
type: decision
created_date: 2026-08-08T06:34:48Z
last_modified: 2026-08-08T06:34:48Z
status: accepted
decided_on: 2026-08-08
deciders: operator (agent-trust bundle plan 01)
scope: subsystem
tags: errors, boot, agent-surface, recovery
---

# Type boot failures at the seam with one shared message across surfaces

## Decision
Every boot failure surfaces a typed `UPPER_SNAKE` code and a message whose final clause names the repair command — or states plainly that no mechanical remedy exists. The typed material lives at the seam itself: `BootError` carries `code()` / `details()` / `surface_message()` (delegating to `StoreError::code()`, `InstantiateError::code()`, and `EngineError::code()` for its wrapped layers), and every surface renders those same parts — the CLI lifts them into the standard `{code, message, details}` envelope of [[engine:typed-error-and-warning-envelope]]; the MCP server prints `memstead-mcp: ERROR [<code>]: <message>` to stderr before exiting, because its transport never comes up on a failed boot and stderr is the only diagnostic surface. `code: INTERNAL` is not producible from the boot seam. The schema-pin failure message ends in the command the source trail calls for: right-name/wrong-version pins name the concrete `memstead mem set-schema <mem> <name>@<installed-version>` repin; name-unknown-everywhere pins name the `memstead schema install` path even when no authoring package exists to point at.

## Context
The one place an agent most needs a typed, actionable error is the moment the engine refuses to start — and that was exactly the place producing an untyped `ERROR [INTERNAL]` with no next step. Two real outages hit this dead end (plenum 2026-08-06/07, expertise 2026-08-07): the typed code existed one layer down (`EngineError::SchemaNotFound`), but the CLI's setup path flattened boot errors through `anyhow`, erasing the code at the seam, and the MCP server died with `-32000 Connection closed` and no envelope at all. The `no_internal_leaks` regression suite covered per-verb recoverable paths only — the boot path was simply outside its coverage. A separately-tracked instance of the same class: a legacy pre-v2 projection config had good store-layer prose naming `memstead projection migrate` but leaked as `INTERNAL`, and its doc comment claimed a `PROJECTION_STORE_LEGACY` wire token that existed nowhere in the tree.

## Consequences
Agents meeting a boot refusal get a branchable code and a runnable final clause instead of a dead end. The boot path gains its own `no_internal_leaks` complement (`memstead-cli/tests/boot_typed_errors.rs`: bad pin both trail shapes, missing pin, duplicate mem, legacy projection config, unparseable store) plus a CLI/MCP parity test pinning both surfaces to the shared renderer. Store-layer classes each own a token (`WORKSPACE_STORE_PARSE`, `WORKSPACE_STORE_IO`, `WORKSPACE_STORE_FORMAT_MISMATCH`, `LEGACY_WORKSPACE_LAYOUT`, `PROJECTION_STORE_LEGACY` — now real — `UNKNOWN_BINDING_VERSION`, `WORKSPACE_STORE_ERROR`); classes with no mechanical remedy say "no memstead command repairs this" rather than inventing a command. Fatality is unchanged — this decision changes what a boot failure says, never what it admits; partial-boot quarantine and below-boot repair verbs are separate follow-on work in the same bundle.

## Relationships
- **REFERENCES**: [[engine:typed-error-and-warning-envelope]]

## Options

A single generic `BOOT_FAILED` code was rejected as typed-in-name-only: the agent's next step differs per class (pin repair vs projection migrate vs storage residue), so the code must too. Fixing only the projection case (the literal backlog item) was rejected because the class recurs per boot-failure source — the plenum incident was a different member of the same class. Keeping the install-hint suppression for wrong-version pins was refined rather than kept: when the pinned version is absent but the name exists at other versions, the honest hint is version repair, not silence.

## Notes


