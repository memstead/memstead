---
type: decision
created_date: 2026-10-02T22:32:16Z
last_modified: 2026-10-02T23:02:49Z
status: accepted
decided_on: 2026-10-03
deciders: operator, implementing agent
scope: subsystem
tags: mounts, install, archive, portability, multi-machine
---

# Record an installed mem in the roster by its content-addressed file, not a cache path

## Decision
We will record a read-only mem installed into the archive cache in the tracked mount roster as a `cached-archive` entry that names its content-addressed file (`<mem>-<first 16 hex of the archive's sha256>.mem`), and resolve that entry against the running machine's cache on every roster read. A missing file quarantines the mem `ARCHIVE_NOT_INSTALLED`, with the install command as the repair and any same-named archive of other content named as not mounted in its place. Archives at an explicit location keep their path. The roster format moves to `memstead-mounts-4` only when such an entry is present; the reader accepts V3 and V4. The cache directory is registered by the crate that owns the cache; a process without it resolves the entry to a placeholder that never holds the file and writes it back unchanged.

## Context
Installed mems were recorded by the absolute path of the cache file, which lives in the user's data directory. On 2026-10-02 a workspace cloned onto a second machine carried a roster entry pointing into the first machine's home directory: the mem could only ever quarantine there, under a generic `MOUNT_UNBACKED` or `MEM_ERROR` that said nothing about what to install. The content key already existed, only inside the cache file name.

## Consequences
- The tracked roster describes the workspace on every machine; a second machine installs the same archive and the roster does not change.
- A wrong archive under the same mem name never mounts: identity is the content key, not the name.
- Boot never reaches the network to fetch a missing archive; installing stays an explicit act.
- `memstead uninstall` and the engine's unregister now remove a quarantined read-only mount; before, the first refused it and the second left its roster entry for the next write to resurrect.
- Not recorded: the registry reference an archive came from (the mount type has no slot for it); the repair names both install routes instead.
- A process-wide registration point exists for the cache directory; the CLI, the MCP server and the git-branch loaders register it.

## Relationships
- **IMPLEMENTS**: [[engine:read-mem-install-and-cache-pipeline]]
- **IMPLEMENTS**: [[engine:workspace-store]]
- **INFORMED_BY**: [[resolve-an-archive-backed-mounts-schema-from-its-own-archive-the-folder-tier-stays-the-authoring-tier]]

## Options

- Keep the absolute path and search the local cache by file name as a fallback: rejected, the recorded path still lies on every other machine.
- Download a missing archive from the registry at boot: rejected, boot must not touch the network and file-installed archives have no registry source.
- Move installed mounts into an untracked per-machine roster: rejected, the tracked roster would stop saying a read-only mem is expected.
- Record only the registry reference: rejected, file installs have none; the content key is universal.
- Add a registry-reference field to the mount type: deferred, it touches every mount construction for a sharper repair sentence.
- Chosen: a `cached-archive` entry by content-addressed file name, resolved per machine.

## Notes

2026-10-03 amendment: the first independent grade found that on a folder workspace a mem under a sealed third-party schema reported a missing archive as `SCHEMA_NOT_FOUND`, because boot resolved the pin (which lives only inside the archive) before checking the archive was there. Boot now checks a missing archive first. The same grade showed that a roster already written on another machine, naming that machine's cache file, stayed broken; such a missing path whose file name is the mem's cache file name (`<mem>-<16 hex>.mem`) is now read as the identity and rewritten as one, so existing clones heal.
Known limitation (final grade, 2026-10-03): the healing rule keys on the cache file-name shape, so an archive mounted at an explicit location whose file name happens to be `<mem>-<16 hex>.mem` and whose file is missing is read as a cache identity (reported `ARCHIVE_NOT_INSTALLED`, rewritten as a `cached-archive` entry). No CLI or MCP verb creates explicit-location archive mounts; only a hand-edited roster can. Narrowing the rule to parents named `memstead/mems` was rejected because clones from a machine with an overridden cache would then not heal.
