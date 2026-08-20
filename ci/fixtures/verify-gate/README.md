# `verify-gate` — the fixture the documented CI gate runs against

A two-artifact Memstead workspace whose mem is fully anchored to its
source tree. It exists so the GitHub Actions example in
[`guides/verify-in-ci`](../../../docs-site/src/content/docs/guides/verify-in-ci.md)
is *exercised* rather than decorative: `ci/verify_gate.py` runs the same
commands the guide prints, in both polarities, on every push.

## What is here

| Path | What it is |
|------|------------|
| `.memstead/workspace.toml` | workspace marker |
| `.memstead/state/mounts.json` | one folder mem, `docs`, stored at `mem/` |
| `.memstead/projections/docs/graph.json` | the binding: source `src/**/*.md`, git change-detection, exhaustive coverage, a `verify` operation |
| `mem/.memstead/anchors.json` | two `anchored` file-grain anchors **with their recorded content hashes** |
| `src/alpha.md`, `src/beta.md` | the source tree |

The recorded hashes are content hashes, not commit ids, so they survive a
fresh clone and a re-`git init`. That is what lets a checkout verify clean
on the first run with no warm-up pass — the fixture is an already-anchored
mem, which is the state a real user's gate runs in.

## The one seam between fixture and reality

A git repository cannot be committed inside another git repository, so
`src/` here is not itself a git checkout. `ci/verify_gate.py` copies the
fixture to a temp directory and runs `git init` there before verifying.

A real user needs no such step: their source tree *is* the checkout, which
is already a git repo. The harness's setup is an artifact of shipping a
fixture inside this repo, not part of the documented workflow — the
commands the harness then runs are exactly the ones the guide prints.

The copy also keeps the working tree clean. Verify is not a pure read: it
writes a findings store, backfills prepared hashes into the anchors
sidecar, and records a `#verified` baseline. On CI's ephemeral checkout
that is harmless; running it in place here would dirty tracked files on
every local `run-tests.sh`.

## Deliberately not committed

The `#verified` baseline and the mutation stamp are machine-specific (a
git commit id and an engine build sha), so `mem/.memstead/config.json`
ships without them. The findings store and friction ledger gitignore
themselves; `.memstead.cache/` is engine scratch.

## Changing this fixture

Editing `src/*.md` without re-recording the hashes in `anchors.json` turns
the clean polarity red — which is the fixture working as designed, not a
bug. Re-record by running `memstead projection verify docs/graph` in a
copy and lifting the resulting `anchors.json`.
