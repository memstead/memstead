---
type: decision
created_date: 2026-09-04T08:03:25Z
last_modified: 2026-09-04T08:03:25Z
status: accepted
decided_on: 2026-09-04
deciders: operator, implementing agent
scope: subsystem
tags: install, read-mem, schema-resolution, workspace-shape, archive, onboarding
---

# Resolve an archive-backed mount's schema from its own archive; the folder tier stays the authoring tier

## Decision
We will let an archive-backed read-only mount resolve its schema pin from the schema package sealed inside its own `.mem`, on every workspace shape and on every boot path (engine boot, runtime registration by `memstead install` and the MCP server's `--read-mem`, re-attach after quarantine). The embedded package is layered per mount, between the workspace tier and the built-ins, and never enters the shared catalogue: a sealed third-party vocabulary belongs to the mount that carries it, not to anything a writable mem may pin. Staging keeps its mem-repo extras (the `__MEMSTEAD:schemas/` ref, so `memstead schema <pin>` renders the installed package and a second mem may pin it); on a folder workspace staging reads and checks the package, writes nothing, and reports `CarriedByArchive`. The folder tier's `.memstead/schemas/` remains the authoring tier and never holds a third party's sealed bytes.

## Context
`memstead install` stopped being shape-gated in 0.13.0 so the shape `quickstart` and `init` produce could attach a published mem at all, but the archive's sealed schema was still staged into the mem-repo's schema ref, which a folder workspace does not have. A mem published under a vocabulary the workspace had never seen therefore refused on exactly the paved shape with a generic `MEM_ERROR` ("staging a schema requires a mem-repo workspace"), while the CLI reference promised install on every shape and the quickstart receipt promised an `UNSUPPORTED_WORKSPACE_SHAPE` refusal that no longer existed. The sealed newcomer run against the published 0.18.0 (2026-09-03) met all three stories on the headline registry command; the local 0.18.0 binary reproduced it. The engine already carried the mechanism: `Engine::from_archive_bytes` resolves a whole-engine-from-bytes mount through its embedded schema, and a read-only `ArchiveSchemaSource` reads the same package from a path. What was missing was applying it to disk-backed archive mounts. This continues [[engineering--every-workspace-creating-command-discloses-the-shape-it-just-made]]: that decision made the receipt tell the truth about the shape; this one makes the truth the CLI reference had been stating since 0.13.0 hold.

## Consequences
- A published mem installs, mounts, reads, uninstalls and re-installs on the folder shape, across process boundaries, with nothing written into the authoring tier; pinned by a CLI test on a folder receiver beside the existing mem-repo suite.
- The folder shape forgoes the staged copy's extras: `memstead schema <pin>` does not render an installed third-party package there, and a writable mem cannot pin a vocabulary that exists only inside some archive. Both are deliberate: the alternative lets an uninstall break a writable mem.
- Every boot of an archive-backed mount reads the archive twice (once for entities, once for the schema package); archives are small and the read is per mount, so the cost was accepted over a cached reader that would have widened the backend trait.
- The quickstart and init receipts now name what the folder shape really cannot do (the atomic `batch-*` commands and `recover`) and say that install works on either shape; the receipt test runs every printed command verbatim, which is why the sentence names `recover` without a program prefix.
- Shipped in 0.18.1; verified from the real channel: the installed binary put a registry mem into a fresh quickstart workspace and read it back on a fresh boot.

## Relationships
- **MOTIVATED_BY**: [[advertised-front-door-commands-serve-a-fresh-non-maintainer-workspace]]
- **INFORMED_BY**: [[every-workspace-creating-command-discloses-the-shape-it-just-made]]
- **REFERENCES**: [[every-workspace-creating-command-discloses-the-shape-it-just-made]]

## Options

**Resolve the mount's schema from its own archive, per mount (chosen).** The archive already carries the sealed package and the engine already had a read-only source for it; the folder tier keeps its boundary.

**Stage the sealed package into the folder workspace's `.memstead/schemas/` (rejected).** That directory is the authoring tier: its packages are validated under the current schema language and reported as rot when they carry retired keys, whereas a sealed third-party package must keep loading with its written meaning exactly because the installing user cannot fix it. Mixing the tiers would either break authoring validation or silently weaken it.

**Restore the shape gate and refuse install on folder workspaces with the typed code (rejected).** Cheapest, and it would have made the three surfaces agree, but by taking back a capability the reference had promised since 0.13.0 and the receipt decision had already treated as the direction; the quickstart shape would again have no way to attach a published mem.

**Add the embedded schemas to the shared workspace catalogue (rejected).** Simpler plumbing, but a writable mem could then pin a vocabulary that lives only inside a read-mem's archive and lose it on uninstall; the per-mount layering keeps ownership where the bytes are.

## Notes

Engine sites: `embedded_archive_schemas` in `memstead-base/src/engine/boot.rs`, consulted by `from_mounts_inner`, `register_writable_mem_inner` and `reattach_quarantined_mem`; `stage_sealed_schema` and `sealed_schema_gitdir` in `engine/lifecycle.rs`. Revisit if a backend-trait schema read ever lands (it would let the archive be opened once per boot).
