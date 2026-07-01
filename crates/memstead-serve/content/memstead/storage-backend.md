---
type: concept
maturity: stable
abstraction_level: abstract
tags: memstead, storage
---

# Storage Backend

## Definition

A storage backend is the mechanism that holds one [[mem]]'s bytes — a folder of files, a branch of a git repository, or a sealed `.mem` archive.

## Explanation

The three kinds map to a mem's lifeforms. A **folder** backend keeps `.md` [[entity]] files in a directory and is simple, single-context, and writable. A **git-branch** backend keeps entity bytes as git blobs on a named branch, adding full history, drift detection, and multi-actor safety. An **archive** backend is a content-addressed `.mem` zip — immutable and read-only, the form a mem takes for transport and publication. Each backend carries its own [[schema]] definitions and per-mem config alongside the content, so a mem stays self-contained when copied.

## Boundaries

- A storage backend is not a [[mount]]: the backend holds bytes; the mount attaches a mem with a capability.
- Capability is not purely a backend property: a folder or git-branch can be mounted read or write, while an archive is always read-only.

## Significance

Because every backend speaks one engine interface, the same [[graph]] operations work whether a [[mem]] lives in a folder, a git branch, or a sealed archive.
