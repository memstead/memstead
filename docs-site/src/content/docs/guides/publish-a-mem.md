---
title: Publish a mem
description: "Share a mem through the memstead.io registry: describe it, dry-run, publish, and the install line others run."
sidebar:
  order: 3
---

A mem is the packaged unit of sharing: a whole typed model — entities, relationships, and the schema they conform to — sealed into one `.mem` archive. Publishing puts that archive on the [memstead.io](https://memstead.io) registry under your GitHub handle, where anyone can install it with one command. This guide walks the first publish end to end.

You need a workspace with a mem worth sharing (see [Getting started](../../guides/getting-started/)) and a GitHub account. Nothing else — no registry signup; authentication is GitHub Device Flow, triggered automatically on first use.

## 1. Give the mem its card text

The registry renders each mem with a one-line description embedded in the archive. Set it before publishing (an empty string clears it):

```bash
memstead mem set-description recipes "Worked example: a tiny cookbook mem"
```

Versions come from the mem config too — seeded as `0.1.0` at init. Check both with `memstead mem list`.

## 2. Dry-run first

`--dry-run` assembles and resolves everything — mem, version, scope, archive size — but POSTs nothing and mutates nothing:

```bash
memstead publish --mem recipes --dry-run
```

```text
# Dry run — would publish

- Mem: `recipes`
- Version: `0.1.0`
- Scope: derived from your GitHub login
- Archive: 3153 bytes
- Registry: https://memstead.io

Nothing was published and nothing was changed.
```

`--mem <name>` names which mem to export-and-publish; it works in every workspace shape. (A single-mem folder workspace can also run bare `memstead publish`, which wraps up the surrounding folder.) The scope is not chosen by you — the registry derives it from your GitHub login at upload time, so the dry run can only tell you it will.

## 3. Publish

Same command, without the flag:

```bash
memstead publish --mem recipes
```

On first use this starts GitHub Device Flow: the CLI prints a code, opens the verification page, and stores the resulting token in `~/.config/memstead/credentials` — subsequent publishes are silent. On success:

```text
# Published github:dasboe/recipes v0.1.0

- URL: https://memstead.io/v/github:dasboe/recipes
```

You can also log in ahead of time with `memstead login`, or run non-interactively by setting `MEMSTEAD_TOKEN` (a GitHub token) — CI has no TTY for the device flow.

## 4. The install line

That's what you put in your README. Anyone pulls your mem into their own workspace with:

```bash
memstead install github:dasboe/recipes
```

The installed mem mounts read-only: its entities and schema structure are readable and linkable, but the engine treats non-first-party content as untrusted input — its schema's instruction prose is withheld and every read surface tags the content's `origin`.

## Ship an update

Bump the version and publish in one step (`--version` persists the bump to the mem config, like `npm version` + `npm publish`):

```bash
memstead publish --mem recipes --version 0.2.0
```

The registry serves the highest published version as `current`. Publishing an *older* version succeeds — it's retained and resolvable — but the output notes that `current` stays where it was.

To take a mem down: `memstead unpublish github:<handle>/recipes` (permitted to the original uploader). The same `<scope>/<name>` becomes immediately re-publishable.

## The refusals a first publisher actually hits

Every refusal carries a typed code (add `--json` and branch on `.code`):

- **`NOT_AUTHENTICATED`** — no token and no TTY for the device flow: ``not logged in and stdin is not a TTY — set MEMSTEAD_TOKEN or run `memstead login` first``. Also the shape a 401 from the registry maps to (expired/revoked token): re-run `memstead login`.
- **`WORKSPACE_NOT_INITIALISED`** — you ran publish outside any workspace: `no workspace found from <cwd> or any ancestor (missing .memstead/workspace.toml)`. `cd` into the workspace, pass `--workspace <path>`, or supply a pre-built archive path.
- **`INVALID_INPUT`** — `--version` without `--mem` (the bump needs to know which mem to re-version), or `--version` combined with a pre-built archive path (its version is already baked in).
- **`INVALID_VERSION`** — `--version` that isn't a semver.
- **`REGISTRY_VALIDATION_FAILED`** — the registry rejected the archive's content (a 400); the message carries the validation variant, the offending path inside the archive, and the detail.
- **`ARCHIVE_TOO_LARGE`** — the archive exceeds the 2 MB publisher cap. Slim the mem or split it.
- **`RATE_LIMITED`** — too many publishes in a window; the message says how many seconds to wait.
- **`FORBIDDEN`** — you tried to publish into a scope that isn't yours (`--scope` overrides are reserved for registry admins; normal publishes never need it).

## Publishing a pre-built archive

If you already have a `.mem` file — e.g. from `memstead export --format mem -o my.mem` — publish the bytes directly:

```bash
memstead publish my.mem
```

## Where next

- The [Registry HTTP reference](../../reference/registry/) documents the wire API behind these commands.
- The [Glossary](../../glossary/#mem) defines mem, archive, and mount precisely.
