---
type: spec
created_date: 2026-01-15
last_modified: 2026-04-12
level: M0
tags: backend, api
---
# Basic Entity

## Identity

A basic test entity for the spec schema.

## Purpose

Testing the markdown pipeline (parse, generate, roundtrip).

## Relationships

- **USES**: [[other-entity]]
- **DEPENDS_ON**: [[dependency]]

## Specifies

This entity specifies the behavior of the basic test fixture.

It includes [[inline-link]] references and `inline code`.

## Constraints

- Must roundtrip through parse/generate without data loss
- Must handle UTF-8 content correctly

## Rationale

Created as a shared test fixture for memstead-git-branch integration tests.
