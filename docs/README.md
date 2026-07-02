# Documentation

Memstead's docs are organized by [Diátaxis](https://diataxis.fr) — four modes,
each answering a different need. Start with the mode that matches what you're
trying to do.

## 🚀 Tutorial — *learning by doing*

New to Memstead? Start here.

- **[Getting started](../docs-site/src/content/docs/guides/getting-started.md)**
  — install, `memstead quickstart`, first entities, and connecting an AI
  agent, end to end. (The [README quickstart](../README.md#quickstart) is
  the condensed version.)

## 🔧 How-to guides — *accomplishing a task*

You know what you want; these get you there.

- **[Build & test from source](build.md)** — the build flavours, the test
  suite, output paths, troubleshooting.
- **Connect an AI agent** — `memstead quickstart` writes the MCP wiring;
  for Claude Code the [plugin](../plugins/claude-code/README.md)'s `/setup`
  skill is the paved path. Then drive the graph with the `memstead_*` MCP
  tools — the [agent recipes](../docs-site/src/content/docs/guides/agent-recipes.md)
  show worked tool-call sequences with real payloads.
- **[Author a schema](../docs-site/src/content/docs/guides/author-a-schema.md)**
  — scaffold with `memstead schema new`, validate, install, and pin a mem
  to it; read the [worked schemas](../examples/README.md) (`agent-program`;
  the paired `reimpl-source`/`reimpl-target`) for full examples.
- **[Publish a mem](../docs-site/src/content/docs/guides/publish-a-mem.md)**
  — share a mem through the registry and install one into a workspace
  ([command cheat-sheet](publish.md)).

## 📖 Reference — *looking something up*

Complete, generated, and kept honest by CI (regenerated from source; a drift
check fails the build if the committed copy lags).

- **[CLI / MCP / UniFFI / WASM reference + parity matrix](../docs-site/src/content/docs/reference/)**
  — every command, tool, and binding, plus the cross-surface parity matrix and
  the error-code index.

## 💡 Explanation — *understanding why*

The ideas and rationale behind Memstead.

- **[Vision](../VISION.md)** — what Memstead is for and the design rationale.
- **[Glossary](../GLOSSARY.md)** — precise definitions (mem, schema,
  workspace, mount, storage backend, …).
- **[Prior art](../PRIOR_ART.md)** — how Memstead relates to adjacent tools.

---

Contributing to the docs (or the code)? See [CONTRIBUTING.md](../CONTRIBUTING.md).
Each mode is kept distinct on purpose — a how-to that detours into theory, or a
tutorial that turns into reference, serves neither reader; keep new pages in one
mode.
