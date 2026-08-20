---
type: principle
created_date: 2026-08-20T07:50:23Z
last_modified: 2026-08-20T07:50:23Z
authority: accepted
universality: domain-wide
tags: deployment, docs, honesty, verification, feature-flags
---

# A claim about running state is measured against the running system

## Statement
When a statement describes what a running system does — which route answers, when a gate fires, what a deployed flag is set to, what a published artifact contains — it is established by querying that system, never by reading the code that would decide it. Source tells you what the default is; only the deployment tells you what is true.

The failure is not carelessness. Reading the code produces a confident, well-reasoned, wrong answer, and the reasoning is what makes it survive review: a correction derived the same way is as likely to be wrong as the claim it replaces.

## Scope
Any assertion about runtime or deployed behaviour, in docs, plan premises, session logs, or code comments: which URL a deployment serves, which of two mutually exclusive branches of a feature flag is live, whether a published release carries a feature, at what moment a validation refuses.

It does not govern statements about the source itself ("this function refuses when X"), which the source does settle. The distinguishing question is whether a configuration, flag, or release boundary sits between the code and the behaviour being claimed.

## Relationships
- **REFERENCES**: [[a-command-a-surface-prints-is-built-never-formatted]]

## Justification

Measured on 2026-08-20 across one bundle. A plan's own Current state asserted that the soft-launch flag defaults ON, so `memstead.ai/agent` 404s on the live deployment and `/try/agent` answers. The default is indeed ON — and the deployment runs it OFF, so the live truth is exactly inverted: `/agent` returns 200, `/try/agent` 404s. That bullet was itself a *correction*, made the day before "against the live code", and it made the documentation worse: it retargeted the harness onto the one URL that does not answer.

Same day, same class, three more: [[engineering--a-command-a-surface-prints-is-built-never-formatted]] records a docs claim that web-medium sync is "refused at declaration time, not at run time" (declaration in fact succeeds with a warning; the refusal arrives at run time), a preparation-slot claim denying mid-run discovery of a gap that is only discoverable mid-run, and a brief promising an anchor refusal that a deliberate tolerance does not fire. Every one was written from the author's model of the system; every one was corrected by executing it.

A release boundary is the same seam: a feature merged and changelogged is not a feature a stranger can install. `quickstart --repo` sat in `[Unreleased]` while the newest public release was two versions behind, so any test against public surfaces would have measured something else entirely and reported it as the product.

## Exceptions

A statement explicitly scoped to the default ("the flag defaults ON") is a source claim and needs no probe — but it may not be restated as what the deployment does, which is the substitution that produced the inversion above.

## Consequences

Deploy-dependent facts get a probe, not a citation: a `curl` and its status code, a version query, a run that provokes the gate. The evidence goes in the log beside the claim, so the next reader can tell a measurement from an inference.

Where a surface must hardcode a posture-dependent value, the documentation says so and names the detector — `serve/scripts/live-check.sh` auto-detects the live soft-launch posture rather than encoding an answer, which is why it never went stale while the prose around it did.

A correction to a runtime claim is held to the standard of the claim: re-derived from source, it is not a correction, and its confidence is unearned.
