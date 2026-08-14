---
type: memo
created_date: 2026-08-14T16:06:16Z
last_modified: 2026-08-14T16:06:16Z
status: closed
tags: registry, install, schema, evidence, cold-start
---

# Three of four published mems were uninstallable because two readers disagreed

## Claim
From the first release until 2026-08-14, `memstead install` could only install mems pinned to a schema the installing binary already carried — in practice, only the built-ins. Three of the four mems published on memstead.io (1 007 of 1 133 entities, including the largest) refused with `SCHEMA_NOT_FOUND`. The registry's whole proposition — third parties publishing typed models under their own vocabulary — was unreachable for its entire life, and nobody noticed because the one mem that worked, `github:dasboe/engine`, happened to pin the built-in `software@0.1.0`.

## Context
Found by the 2026-08-13 cold-start run — a newcomer installing from memstead.io with no prior knowledge of the project. The newcomer's protocol attributed the failure to a renamed schema key. That was a coincidence of the sample: all three failing mems shared one non-built-in schema, `expertise@0.1.0`, and the key rename was only the second of two independent defects.

## Substance

The archive was never at fault. Every published archive carries its schema at `.memstead/schema/`, and the archive validator read and ACCEPTED that schema on the way in. Two defects sat behind the refusal:

1. **Nothing staged it.** A written, documented, atomic extraction helper existed and had never been called — dead since the genesis commit. Worse, it targeted `.memstead.cache/schemas/`, a directory the failing pin resolver does not consult; wiring it as written was measured and still failed with the identical error. The apparatus for staging existed in a form that could not have worked.
2. **The two tiers ran different readers.** The archive validator read the package with sealed rules; every directory read applied authoring rules. So a package accepted minutes earlier was refused on the way back in — and the installing user could not fix it, because the bytes were a third party's.

`SCHEMA_NOT_FOUND` misreported both: it said no source held any version of the schema and advised obtaining the package and running `memstead schema install`, while the package sat inside the archive the user had just handed the engine.

## Alternatives



## Outcome

Fixed 2026-08-14. Install stages the archive's embedded package into the workspace's own schema storage before registering the mount, and one sealed reader now serves admission and read-back alike. Confirmed against the live catalogue from a clean workspace with no prior `schema install`: all four mems install, mount, and read — 456 + 287 + 264 + 126 = 1 133 entities. The dead helper and the never-written cache layer were deleted rather than wired, so the tree holds one staging path instead of two with one inert.

Two lessons outlive the fix. A capability with no end-to-end test over its own live artifacts can be absent for a whole release lifetime while looking present in the code. And a dead function is not neutral — this one advertised a solution that would not have worked, which is worse than no function at all.
