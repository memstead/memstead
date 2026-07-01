# Contributing to Memstead

Thanks for your interest in Memstead. Contributions are welcome — bug reports,
documentation fixes, and code alike.

Memstead is a schema-agnostic graph engine: each vault keeps a typed model of a
chosen subject as Markdown + git, readable by both humans and LLMs, with MCP as
the AI-agent access layer. The engine is the open core; a couple of commercial
layers (a hosted registry, a native app) build on top of it and are not part of
this repository. See [LICENSING.md](LICENSING.md) for the boundary.

## Posture (accept-with-guardrails)

We accept external contributions, with a few guardrails that keep the project
maintainable:

- **Discuss large or structural changes first.** Open an issue before starting
  significant work (new subsystems, dependency additions, schema or MCP
  surface changes) so we can agree on the approach before you invest time. Small
  fixes — typos, docs, obvious bugs — can go straight to a PR.
- **Every change ships with a test plan.** New behaviour needs a test; a bug fix
  needs a test that fails before and passes after. See *Testing* below.
- **No CLA and no DCO.** There is nothing extra to sign. By opening a pull
  request you agree that your contribution is licensed under the same terms as
  the file it modifies (see [LICENSING.md](LICENSING.md) — the engine is
  `MIT OR Apache-2.0`; the Claude Code plugin under `plugins/` is MIT).

## Getting set up

You need a stable Rust toolchain (edition 2024) and — for the plugin tests —
Node.js. There is no published release yet, so the install path is
build-from-source:

```bash
./build-engine.sh          # builds the workspace, installs the `memstead` CLI,
                           # builds the release `memstead-mcp` binary
```

See [docs/build.md](docs/build.md) for the details (build flavours, output
paths, troubleshooting).

## Testing

Run the full suite before opening a PR:

```bash
./run-tests.sh             # engine (both build flavours) + plugin
```

Or, while iterating on the engine, one flavour at a time:

```bash
cargo nextest run --workspace --features vault-repo      # full (git-backed)
cargo nextest run --workspace --no-default-features      # lean (folder-only)
```

The engine builds in two flavours from one set of crates — the default
`vault-repo` build and a lean `--no-default-features` build — and CI runs both.
If your change touches a generated reference doc, regenerate it rather than
editing it by hand (CI fails on drift); the generator is `xtask` — see
[docs/build.md](docs/build.md).

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
