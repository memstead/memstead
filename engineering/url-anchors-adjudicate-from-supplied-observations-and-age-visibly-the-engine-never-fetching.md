---
type: decision
created_date: 2026-09-02T01:09:24Z
last_modified: 2026-09-02T01:09:24Z
status: accepted
decided_on: 2026-09-02
deciders: execute-graph-plan loop, evidence-engine bundle
scope: subsystem
tags: anchors, url, verify, sidecar, provenance
---

# Url anchors adjudicate from supplied observations and age visibly, the engine never fetching

## Decision
We chose to make the `url` grain observable without the engine ever fetching: an observer supplies what it retrieved, and the engine adjudicates and remembers. `memstead verify-anchors --mem <m> --observations <file>` takes rows `{artifact, hash | content | absent: true, observed_at?}`; a url row with a supplied observation enters the one observation funnel (`observe_anchor`) and resolves through `resolve_anchor` exactly like a file anchor (equal hash resolves, a differing hash drifts under `stable` and rechecks under `unstable`), while a supplied `absent` resolves `recheck`, never `orphaned`, because an observer failing to retrieve a web resource is not the medium saying the artifact is gone and prune must never act on it. `content` is hashed under the same canonicalization the write path applies to a url anchor's `content`. Matched observations are recorded on the sidecar rows as `last_observed {at, hash, state}` (anchors sidecar version 2; version 1 files load unchanged and are rewritten as version 2 on the next anchor write; an unknown higher version still refuses typed), so from then on every surface that shows a url row's state shows its age too, `unobserved for N days`: the per-entity anchors read, `verify-anchors`, health's `anchors` axis (an `aging` list) and `open_questions` axis (`anchors_aging`, counted as open work), and the fidelity report. A url row with no observation, supplied or recorded, stays `unobserved` and is never scored. A malformed observation row refuses the whole run with `INVALID_OBSERVATION` before any state changes; rows naming no url anchor are reported as unmatched and change nothing. The `url` grain is admitted beside every medium, so a mem with one filesystem binding accepts url anchors; a path-shaped grain whose artifact is a URL refuses `INVALID_ANCHOR` naming the rule (a URL never enters a path namespace). The run brief's anchor instruction and the docs tell authors to set `hash_stability: stable` on immutable documents.

## Context
The engine's refusal to perform network I/O is a standing decision, and it left the `url` grain half-alive: writable with observer-supplied `content` at write time, then permanently `unobserved`, never drifting, never aging, and refused outright on any mem whose single binding source was path-shaped (`GrainNamespaceUnsupported`). An investigative evidence mem whose anchors are coordinates into public web documents, edited by several models over months and checked by a party other than its author, needs exactly the states file anchors have, plus one file anchors do not: how old the last look was, because a web observation is only as current as the observer who made it. The pilot that motivated the bundle kept its URL coordinates as free text in an Evidence section for want of this. The design question was where the observation lives: the findings store never records unobserved rows by design, so nothing there could age; the row itself is the only home that makes `unobserved for N days` derivable everywhere the row is read.

## Consequences
- Url anchors carry the same four states as file anchors and one more attribute, the age of the observation they rest on; a state observed months ago is visibly old rather than silently current.
- The engine stays network-free; observation is the caller's act, on the CLI, and the recorded observation is dated by the observer (`observed_at`) or by the engine clock.
- Path and entity rows carry no `last_observed`: they are observed live on every pass, and recording them would produce a sidecar commit per verify for no information.
- A supplied `absent` cannot orphan a url row, so a transient retrieval failure never feeds prune a deletion.
- Sidecar readers must accept version 1 and 2; the first anchor write after upgrade rewrites the file's version number and nothing else on existing rows.
- The url namespace refusal on single-path-source mems is gone; the new refusal (`PathGrainOnUrlArtifact`) catches the actual confusion, a page coordinate written as a path span.
- Deferred, deliberately: a `<url>#<unit-key>` page-coordinate form under the url grain with a generic text-units preparation; this decision treats the artifact string as opaque and leaves any fragment untouched, so nothing forecloses it.

## Relationships
- **INFORMED_BY**: [[sealed-content-is-read-by-the-same-reader-that-admitted-it]]

## Options

- The engine fetches URLs at observation time: rejected, network in the deterministic kernel, and explicitly refused by standing decision.
- Record observations only in the findings store: rejected, unobserved rows never become findings by design, so nothing would age; the sidecar row is the observation's home.
- Supplied `absent` resolves `orphaned` like a missing file: rejected, a retrieval failure is not the medium's statement that the artifact is gone, and prune acts on orphaned.
- Record `last_observed` on every row of every verify (path rows too): rejected, a commit per verify pass for information the live observation already carries.
- Change the url default to `stable`: rejected, most web pages change; the brief puts the choice where the knowledge is, with the author.
- Allow `span` in the url namespace for page coordinates: deferred to a later bundle in the smaller `<url>#<unit-key>` form; nothing here forecloses it.
- Chosen: supplied observations through the one funnel, recorded on the row, aged on every surface, engine network-free.

## Notes

Landed in the engine's 0.15.0 line with the CLI flag, the health and fidelity-report renderings, the brief instruction, the changelog entry, the fidelity-contract concept page and the glossary sentence. Tests cover every state, the aging, the version-1 upgrade, the version-3 refusal, the namespace admission and the new refusal.
