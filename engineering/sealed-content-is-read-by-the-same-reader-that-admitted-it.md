---
type: principle
created_date: 2026-08-14T16:05:57Z
last_modified: 2026-08-14T16:05:57Z
authority: accepted
universality: domain-wide
tags: schema, sealed-content, archive, install, compatibility
---

# Sealed content is read by the same reader that admitted it

## Statement
When content is sealed by someone else — a published archive, an installed package, anything whose bytes this engine cannot ask its author to change — the surface that ADMITS it and every surface that later READS IT BACK must run the same reader, under the generation the content was sealed in. Two readers over one sealed artifact is a defect, not a layering choice: it produces content that is valid to publish and invalid to install.

## Scope
Sealed third-party content on any ingress: published `.mem` archives and their embedded schema packages, installed schema packages on a backend's storage, built-in version directories. It does NOT extend to authoring surfaces — a package directory an author is editing is read under the CURRENT language, and retired keys refuse there loudly by name. The distinction is agency: an author can act on a refusal, an installing user cannot.

## Relationships
- **REFERENCES**: [[engine:read-mem-install-and-cache-pipeline]]
- **REFERENCES**: [[three-of-four-published-mems-were-uninstallable-because-two-readers-disagreed]]

## Justification

Sealed formats outlive the rules they were sealed under. Every key rename, every polarity flip, every retirement moves the current language forward while the sealed bytes stay where they were — and the publisher is unreachable. If admission and read-back are separate code paths, they drift silently at exactly the moments the language changes: the archive validator keeps accepting what the reader has started refusing. The failure is invisible until someone installs.

The evidence is [[engineering--three-of-four-published-mems-were-uninstallable-because-two-readers-disagreed]]. One reader for one class of content makes the divergence unrepresentable rather than merely tested-against.

## Exceptions



## Consequences

- A single named function owns sealed reading (`load_sealed_package`), and every sealed surface calls it — the archive validator, the git-branch schema ref, the install-time staging pass in [[engine--read-mem-install-and-cache-pipeline]]. Adding a sealed surface means calling it, never re-deriving it.
- Sealed bytes are never rewritten on the way in. No migration, no normalisation, and specifically no injecting a format marker — the marker's presence IS the content's generation, so writing one would restate the publisher's meaning as our own.
- The generation flag travels with the content, not with the reader's calendar. A reader that decides the generation from the reading context instead of the artifact has re-created the two-reader split under a new name.
- A sealed artifact that genuinely cannot be read refuses under its own code naming the reader's diagnosis — never a code whose recovery advice is "obtain the thing you are holding".
