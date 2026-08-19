---
type: decision
created_date: 2026-08-19T13:29:17Z
last_modified: 2026-08-19T13:29:17Z
status: accepted
decided_on: 2026-08-19
deciders: operator (plan 09a constraint), execute-plan loop
scope: subsystem
tags: export, archive, sealed-schema, trust-boundary, whitelist
---

# The sealed schema an archive carries is the language, never the install package

## Decision
A `.mem` archive's `.memstead/schema/` subtree carries exactly the sealed LANGUAGE: the manifest (`schema.yaml`), the sealed format marker, and the type definitions (`types/*.yaml`). Install-time package scaffolding — `mem-template.json`, `README.md`, the workspace-local install-provenance stamp — never travels. The boundary is enforced twice, on both sides of the wire: the export collector selects language members by allowlist (a denylist of known scaffolding would re-break on the next scaffolding file), and the reader's strict archive whitelist refuses anything else under the schema prefix. The whitelist is a reader-trust boundary and grows only for content readers need.

## Context
The git-branch export collector (`schema_files_from_memstead_ref`) copied the sealed package from the `__MEMSTEAD` ref stripping only the install-provenance file, while `schema install` stages the FULL builtin package — template and README included. The strict archive whitelist then refused the export's own output: every git-branch mem pinned to a template-shipping builtin (engineering/planning/project/software, all generations) could not `export --format mem` at all (found by the plan-05 grading probe; predates the correctness quartet). Widening the whitelist was rejected — the trust boundary is the reader's, and setup convenience is not schema content.

## Consequences
Every builtin-pinned git-branch mem exports again, and the produced archive re-reads cleanly in a fresh workspace. A receiving workspace can trust that nothing under `.memstead/schema/` is executable setup material — the subtree is a vocabulary, reviewable as one. Future install-time scaffolding additions cannot silently re-break export or smuggle content into archives: the collector's allowlist ignores them and the reader's whitelist refuses them. Pinned by an e2e test per template-shipping builtin family plus a smuggle complement.

## Options



## Notes


