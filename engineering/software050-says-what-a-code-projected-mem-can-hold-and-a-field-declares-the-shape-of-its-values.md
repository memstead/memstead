---
type: decision
created_date: 2026-09-02T20:10:21Z
last_modified: 2026-09-02T20:10:21Z
status: accepted
decided_on: 2026-09-02
deciders: coordinating session (bundle B), implementing agent, operator directive 2026-09-02
scope: subsystem
tags: schema, software, binding, vocabulary
---

# software@0.5.0 says what a code-projected mem can hold, and a field declares the shape of its values

## Decision
The software schema's guidance says that in a mem projected from code `spec` is the home type for a surface and dominates by design, the signal worth watching being a cluster with no non-spec entity beside its specs (read by the vital-signs axis from `spec`'s `last_resort` declaration); `requirement` belongs to mems with a normative source and is absent from a code mem by design; the failure-mode prose names only relationships the vocabulary carries. `contract` gains `protocol: engine_state` and `version_axes` (name=constant pairs) for engine-owned durable state files; `spec` gains a `notes` catch-all for standing remarks. The schema language gains `value_pattern` on any metadata field, a regex checked in full per value (per member on csv fields) and compiled at install. The engine binding's intent says the same, gated by a workspace check against the vocabulary and the crate roster.

## Context
Bundle B plan 6 (2026-09-02), the storey-2 model-truth findings the operator delegated: the intent named a phantom crate family and a relationship the vocabulary lacked, the dominance guidance counted specs as a defect in a mem that is specs by design, and engine-owned state files had no type that carried their version obligations.

## Consequences
The three software mems run on 0.5.0 conformance-clean; the anchors sidecar is modelled as an `engine_state` contract; four frozen specs carry their historical marker in Notes. `value_pattern` is generic, the `version_axes` shape check being its first use.

## Options

A new type for internal code surfaces rejected (the guidance was wrong, not the mem); docstrings as a normative source for requirement rejected; modelling state files as spec rejected (loses the format and version obligations).
