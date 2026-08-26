---
type: principle
created_date: 2026-08-26T17:50:26Z
last_modified: 2026-08-26T17:50:26Z
authority: accepted
universality: domain-wide
tags: archives, publishing, drift, gates, seals
---

# Detecting that a published archive lags its mem is a gate; re-cutting it stays a decision

## Statement
A sealed archive of a mem is compared, mechanically and on every run of the gate, against the mem it was cut from, and a head-tracking archive that has fallen behind is reported as stale by name. Re-cutting it is never automatic. The comparison is a detector: it reads the archive out of its container and writes nothing, ever.

The archive's class is what a difference means. A head-tracking archive is published as the mem's current state, so a difference is staleness. A version-pinned publish lags its mem by design and is never stale. A sealed record is deliberately frozen and is never stale. An archive whose class is undeclared is itself the finding, because nothing can then say what a difference means.

## Scope
Every artifact that seals a mem's prose at a moment and is then served, downloaded, mounted or shipped: the memstead.ai downloads, the archive baked into a deploy image, a registry publish. It governs the detection and the classification, not the decision to publish.

It does not govern derived artifacts that are regenerated from a source on every build (a generated reference page, a site bundle): those are held in lockstep by their generator's own check, which is a different mechanism for a different failure.

## Relationships
- **REFERENCES**: [[a-class-of-surfaces-is-discovered-and-only-what-a-walk-cannot-see-is-declared]]

## Justification

Six consecutive grading rounds swept the claim that verify is a pure read. Each corrected the source surfaces it could see and declared the class closed, and each was wrong, because the retired sentence sat inside a published archive that nothing in the project compared against anything. Reading source trees is exactly the check that passed six times over stale text.

A deliberate re-cut is not sufficient on its own either. On 2026-08-24 the `engineering` archive was re-sealed at 00:17 and the correction it existed to carry landed at 14:53 the same day, so the re-seal published the wrong text and left no trace that it had.

The other half of the rule is why detection and publishing are separated. Re-sealing on every mem write would be a publish decision made by a timer, and a re-export brings the archive forward to the mem's CURRENT state: it publishes every change since the last cut, not only the one that prompted it. That is the right semantics for an archive and the wrong thing to trigger automatically.

## Exceptions

A pinned publish is exempt from staleness by construction, not by allowance: its whole purpose is to lag. The same is true of a record sealed as evidence, where correcting it would falsify the measurement it backs.

Where the mem lives on a substrate a given checkout cannot reach (a private mem-repo in a public CI lane), the archive is reported as unmeasured by name rather than counted clean, per [[a-class-of-surfaces-is-discovered-and-only-what-a-walk-cannot-see-is-declared]].

## Consequences

A correction to a mem now has a visible debt attached to it: the gate names every archive that no longer carries it, so the choice to publish is taken deliberately instead of forgotten.

The session that re-exports owes an account of what else it published. That is a real cost and it is the correct one: an archive republished as though it carried only the intended change misrepresents a publish as a patch.

The comparison is byte-for-byte over entity markdown, so it needs no version stamp, no seal timestamp and no cooperation from the export format. A mem's own README and the engine bookkeeping under `.memstead/` are excluded by asking each file whether it is an entity rather than by matching its name.
