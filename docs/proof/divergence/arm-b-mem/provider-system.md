---
type: spec
created_date: 2026-07-15T07:26:44Z
last_modified: 2026-07-15T07:34:09Z
level: M1
stability: evolving
tags: providers, resources, effects, di
---

# Provider System

## Identity
Kāra's typed-resource provider mechanism: `with_provider[R]` installs an implementation of a resource `R` on a dynamically-scoped provider stack, and `R.method(...)` calls dispatch to the innermost installed provider.

## Purpose
To let effectful resources (Stdout, Stdin, Clock, RandomSource, a database, an HTTP client) be injected and intercepted — enabling testing, mocking, and dependency inversion — without threading handles through every call.

## Relationships
- **REFERENCES**: [[standard-library]]
- **USES**: [[standard-library]]

## Realization

- Runtime provider-stack ABI: runtime/src/lib.rs; codegen provider vtable: src/codegen/provider.rs
- Escape analysis: src/provider_escape.rs, tests/provider_escape.rs
- I/O providers: println/print/eprintln routed through Stdout/Stderr; with_provider[Stdin] interception
- Examples: examples/parallax/src/providers.kara

## Specifies

- `with_provider[R] { ... }` lowering; nested providers resolve innermost-wins.
- R.method dispatch through the provider stack via an emitted vtable.
- Provider-stack inheritance into par blocks so parallel work sees the same providers.
- Spawn-escape detection so a provider reference cannot escape its dynamic scope.

## Constraints

- A provider reference must not escape the `with_provider` scope that installed it (provider-escape + spawn-escape checks).

## Rationale

Underpins the AI-writes-Kāra Mend demo and testable I/O; part of the [[standard-library]] baked I/O migration (CR-202).
