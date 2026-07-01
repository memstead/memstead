# vault-repo

This directory is a **vault-repo-git** repository — a single multi-root git
repo that carries the data, the schemas that describe it, and the identity
of every vault it contains. One database, one git repo, portable: copy the
`.git/` directory, push it to a remote, fork it for an experiment — wherever
the bytes go, the database goes with them.

## What this is

A vault-repo-git is **data + DDL + identity** in one self-describing git
object store:

- **Data** lives on per-vault branches. Each vault is a top-level branch
  whose tree is the vault's content (markdown entities, attachments,
  whatever the vault chooses to carry).
- **DDL** (the schemas the data conforms to) lives on the unified
  `__MEMSTEAD` registry ref under `schemas/<schema-name>/`. Vaults reference
  a schema by name in their config; the schema definition is part of the
  same repo as the data, so there is no out-of-band coupling.
- **Identity** (per-vault metadata: the vault's display name, schema
  pin, belongs-to relationships) lives on `__MEMSTEAD` under
  `vaults/<vault>/config.json`. The presence of that blob plus the
  per-vault content branch `refs/heads/<vault>` is what makes a vault.

Everything needed to read, validate, and reason about the database is
inside the `.git/` directory. There is no auxiliary state file, no
sidecar database, no required external service.

## Branch convention

The repo uses three classes of refs:

- **`main`** — operator-facing docs only (this README). The engine does
  not read content off `main`.

- **`__MEMSTEAD`** — the unified registry ref. Carries `schemas/...` and
  `vaults/<vault>/config.json` blobs. Engine-managed; operators do not
  edit this ref directly.

- **One orphan branch per vault**, named identically to the vault,
  carrying that vault's content and only that vault's content. Vault
  branches are **orphan** — they share no history with `main`, with
  `__MEMSTEAD`, or with each other. Vault branches **never merge** into
  any other ref.

The orphan-branches design lets a vault's history be inspected,
exported, or pruned without entangling unrelated vaults.

## Reading content

Three ways to inspect what is in here, depending on the audience:

- **GitHub branch switcher** — push the repo to a remote and visitors can
  switch between branches in the GitHub UI. Each branch presents as a
  flat tree of that vault's content. No tooling required on the reader's
  side.

- **Local git checkout** — run `git checkout <vault>` (or
  `git switch <vault>`) to materialise that vault's working tree.
  Standard git ergonomics apply.

- **Memstead tooling** — the engine reads vault-repo-git natively via gix, with
  no working-tree materialisation required. Agents query through the MCP
  surface (e.g. `memstead_overview`, `memstead_search`, `memstead_entity`); CLI users
  drive the same surface through `memstead-cli`.

The engine and Memstead tooling read directly from the object store — they do
not require a checked-out branch and do not modify the working tree. A
vault-repo-git can be perpetually parked on whichever branch the human last
checked out (or with no working tree at all) and the tooling is unaffected.

## Embedding in another git repo

When `vault-repo/` lives inside another git repository (a code repo,
documentation repo, monorepo — anything with its own `.git/`), the outer
repo will treat `vault-repo/.git/` as a **gitlink** by default. A gitlink
is git's mechanism for submodules: it records the inner repo's commit
SHA in the outer repo's tree, but does not track the inner repo's
content. This is almost never what you want for a vault-repo that is
meant to be portable on its own terms.

The fix is to add `vault-repo/` (note the trailing slash, denoting a
directory) to the outer repo's `.gitignore`:

```
# Outer repo's .gitignore
vault-repo/
```

This tells the outer repo to ignore the entire `vault-repo/` directory.
The vault-repo's own `.git/` continues to function normally; the outer
repo simply doesn't try to track it. The Memstead engine surfaces an
`OUTER_REPO_NOT_IGNORING_VAULT_REPO` warning via `memstead_health` when this
gitignore entry is missing, so the gitlink trap is caught early.

If you genuinely want the outer repo to track the vault-repo as a
submodule, that is a different setup (`git submodule add ...`) and is
outside the scope of this README.

## Schema and config layout

On `__MEMSTEAD`:

- **`schemas/<schema-name>/`** — each subdirectory is one schema. A
  schema declares the entity types, their fields, the validation rules,
  and the relationship surface that vaults using this schema must
  conform to. Vaults pick a schema by name in their config; multiple
  vaults can share a schema. Schemas are versioned alongside the data
  they describe, in the same repo.

- **`vaults/<vault>/config.json`** — one JSON blob per vault. The blob
  declares the vault's display metadata (description, write guidance,
  belongs-to relationships) and the schema pin it uses. The tree path's
  `<vault>` segment matches the corresponding `refs/heads/<vault>`
  branch.

Adding a vault is one `__MEMSTEAD` upsert (a new `vaults/<name>/config.json`)
plus an orphan branch creation; removing one is the reverse. The Memstead
engine and `memstead-cli` automate both.
