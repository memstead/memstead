---
type: concept
maturity: stable
abstraction_level: abstract
tags: memstead, interface
---

# MCP Layer

## Definition

The MCP layer is Memstead's AI-agent access surface — a Model Context Protocol server that exposes the [[graph]] to LLM agents as a small set of typed tools.

## Explanation

Agents do not read or write `.md` files directly; they call MCP tools to query the [[graph]] (overview, search, read an [[entity]], read a [[schema]]) and to mutate it (create, update, relate, rename, delete). Every mutation routes through the engine, so schema validation, relationship integrity, and provenance hold no matter which agent makes the call. Tool names, parameter shapes, and error envelopes are designed to minimise an agent's round-trips, because the agent is the primary consumer.

## Boundaries

- The MCP layer is not the only surface: a CLI serves humans and scripts, and a native app embeds the engine directly — but all of them route mutations through the same engine.
- The MCP layer does not bypass the [[schema]]: a tool call that violates the schema is refused with a typed error, just like any other mutation.

## Significance

Exposing the [[vault]] through MCP is what lets a developer point their own agent at Memstead and have it read and build a [[graph]] natively.
