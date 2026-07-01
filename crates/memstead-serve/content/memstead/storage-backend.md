---
type: concept
maturity: stable
abstraction_level: abstract
tags: memstead, storage
---

# Storage Backend

## Definition

A storage backend is the mechanism that holds one [[vault]]'s bytes — a folder of files, a branch of a git repository, or a sealed `.mem` archive.

## Explanation

The three kinds map to a vault's lifeforms. A **folder** backend keeps `.md` [[entity]] files in a directory and is simple, single-context, and writable. A **git-branch** backend keeps entity bytes as git blobs on a named branch, adding full history, drift detection, and multi-actor safety. An **archive** backend is a content-addressed `.mem` zip — immutable and read-only, the form a vault takes for transport and publication. Each backend carries its own [[schema]] definitions and per-vault config alongside the content, so a vault stays self-contained when copied.

## Boundaries

- A storage backend is not a [[mount]]: the backend holds bytes; the mount attaches a vault with a capability.
- Capability is not purely a backend property: a folder or git-branch can be mounted read or write, while an archive is always read-only.

## Significance

Because every backend speaks one engine interface, the same [[graph]] operations work whether a [[vault]] lives in a folder, a git branch, or a sealed archive.
