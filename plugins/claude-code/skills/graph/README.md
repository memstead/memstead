# Graph — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- interactive graph manipulation — the user's direct interface to create, query, update, and connect entities
- general-purpose: handles any graph task the user asks for

## Bootstrap

- always read memstead_overview first — never assume prior knowledge of the graph state
- the overview contains community clusters and system documentation

## Principles

- never invent — if information is missing, ask the user
- never edit entity markdown directly — always mutate via MCP tools
- ask, don't assume — clarify before making changes the user didn't explicitly request

## IDs and links

- IDs are vault-prefixed: vault--entity-name
- wiki-links: [[name]] or [[path/to/name]] — always resolve within the current vault
