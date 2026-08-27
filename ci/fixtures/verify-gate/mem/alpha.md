---
type: note
created_date: 2026-08-21T00:00:00Z
last_modified: 2026-08-21T00:00:00Z
---

# Alpha

## Description

Describes the first thing, mirroring `src/alpha.md`. This entity exists so the
fixture's clean polarity is a real anchored mem: the sidecar names
`docs--alpha`, and a green verdict should never rest on an anchor pointing at
an entity that does not exist.

The filename matters. An entity's id is its mem plus its file stem, so this file
must be `alpha.md` and not `docs--alpha.md`: the latter minted
`docs--docs--alpha` while the sidecar named `docs--alpha`, which made both rows
of this "clean" fixture dangling. Nothing detected it until the entity end of an
anchor was checked at all (consistency-sweep 03/02).
