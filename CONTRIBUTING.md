# Contributing to Memstead

Thanks for your interest in Memstead. Contributions are welcome — bug reports,
documentation fixes, and code alike.

Memstead is a schema-agnostic graph engine: each mem keeps a typed model of a
chosen subject as Markdown + git, readable by both humans and LLMs, with MCP as
the AI-agent access layer. The engine is the open core; a commercial
layer (a hosted registry) builds on top of it and is not part of
this repository. See [LICENSING.md](LICENSING.md) for the boundary.

## How this project is built

Memstead is an AI-driven project and stays one. Read these facts before you
decide to contribute:

- The maintainer writes no code by hand, and may never have read the file your
  pull request changes.
- Your change is judged by an agent against the machine gates: `./run-tests.sh`
  in CI, and consistency with the recorded decisions in
  [`engineering/`](engineering/README.md). The question asked of a change here
  is which gate checked it, not whether a human understood it.
- Anyone who needs every line of a codebase human-understood before it ships is
  in the wrong place.

Security is the exception and stays a human channel. Report a vulnerability
through [SECURITY.md](SECURITY.md), never as a public issue or pull request.

The operating layer that drives the loop (the plan walker, the plan style, the
session rules) is not in this repository and stays the maintainer's own asset
and the seed of the commercial layer. Its results are public: the decisions in
`engineering/`, the check records on them, and the commit history here.

## Fixes and features

A **fix** makes documented behaviour true. A doc page that describes something
the code does not do, an error message naming the wrong thing, a missing test,
a bug measured against the project's own specification: those are fixes, and a
fix goes straight to a PR.

Anything that changes or adds a surface is a **feature**: a CLI flag, an MCP
tool parameter, a schema field or type, a doc page, a binary. Correcting an
existing doc page is a fix; adding one, or making one promise something the
code does not yet do, is a feature. A feature needs an issue first. A feature
pull request with no issue behind it is closed with a pointer to this section,
and it is not reviewed.

Before you open that issue, read [`engineering/`](engineering/README.md). It is
a live Memstead mem carrying this project's standing decisions and principles,
and it answers "is this by design?" for much of what looks wrong from outside.
Its README says how to mount it as a mem; the entities are plain Markdown, so
reading them straight from this repository works too.

## What happens to your pull request

- The same gates apply to your change as to the maintainer's own work. Nothing
  lands past a red `./run-tests.sh`.
- First response within a week, in batches.
- A fix is merged as submitted, under your authorship, or returned to you with
  concrete change requests.
- An accepted feature issue is implemented by the maintainer's loop. The commit
  carries a `Suggested-by:` trailer naming you and the issue, and the changelog
  entry names you.
- `Co-Authored-By:` is used only for code taken from a diff you submitted.
  `Suggested-by:`, `Reported-by:` and `Tested-by:` cover an idea, a report, and
  testing.
- A pull request touching files that a running work bundle also touches waits
  for that bundle to land. You will be told which bundle and why.

## Ground rules

- **Every change ships with a test plan.** New behaviour needs a test; a bug fix
  needs a test that fails before and passes after. See *Testing* below.
- **No CLA and no DCO.** There is nothing extra to sign. By opening a pull
  request you agree that your contribution is licensed under the same terms as
  the file it modifies (see [LICENSING.md](LICENSING.md) — the engine is
  `MIT OR Apache-2.0`; the Claude Code plugin under `plugins/` is MIT).

## Getting set up

You need a stable Rust toolchain (edition 2024) and — for the plugin tests —
Node.js. Pre-built release binaries exist for end users (see the
[README quickstart](README.md#quickstart)), but development works against a
source build:

```bash
./build-engine.sh          # builds the workspace, installs the `memstead` CLI,
                           # builds the release `memstead-mcp` binary
```

See [docs/build.md](docs/build.md) for the details (which crate produces
which binary, output paths, troubleshooting).

## Testing

Run the full suite before opening a PR:

```bash
./run-tests.sh             # lint, guards, engine, docs-site prebuild, plugin
```

Or, while iterating on the engine:

```bash
cargo build --workspace
cargo nextest run --workspace
```

There is one build: `cargo build` produces the multi-mem, git-backed engine,
and the same binaries serve folder-only workspaces. No crate declares a
feature that changes what ships, and CI runs the same `./run-tests.sh`.
The docs-site reference pages (CLI, MCP tools, error index) are rendered
from the engine sources at every docs-site build and are never committed:
change a help string or a tool description and the next build renders it.

## Opening a pull request

- Keep the PR focused; one logical change per PR.
- Write the commit message and PR description for a future reader reconstructing
  *why* — not just *what*.
- Make sure `./run-tests.sh` is green.
- English only, for code, commits, and issues.

## Where things live

The repository root **is** the engine workspace (`Cargo.toml` + `crates/`). The
README's structure table maps the top-level layout; a quick tour:

- `crates/` — the engine crates: the schema layer, the in-memory store, the two
  storage backends (folder + git-branch), the `memstead` CLI, and the
  `memstead-mcp` server.
- `plugins/claude-code/` — the Claude Code plugin (skills + hooks).
- `docs/`, `docs-site/`, `examples/` — documentation and worked examples.

## Code of conduct

Participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md). Please
read it.
