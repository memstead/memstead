# Documentation

Memstead's docs are organized by [Diátaxis](https://diataxis.fr) — four modes,
each answering a different need. Start with the mode that matches what you're
trying to do.

## 🚀 Tutorial — *learning by doing*

New to Memstead? Start here.

- **[Quickstart](../README.md#quickstart)** — install, create your first vault,
  and run a `create → search` loop end to end.

## 🔧 How-to guides — *accomplishing a task*

You know what you want; these get you there.

- **[Build & test from source](build.md)** — the build flavours, the test
  suite, output paths, troubleshooting.
- **Connect an AI agent** — the [Claude Code plugin](../plugins/claude-code/README.md):
  run `/setup`, then drive the graph with the `memstead_*` MCP tools.
- **Author a schema** — learn by example from the
  [worked schemas](../examples/README.md) (`agent-program`; the paired
  `reimpl-source`/`reimpl-target`), then point a vault at yours.
- **[Publish & install a vault](publish.md)** — share a vault through a
  registry and install one into a workspace.

## 📖 Reference — *looking something up*

Complete, generated, and kept honest by CI (regenerated from source; a drift
check fails the build if the committed copy lags).

- **[CLI / MCP / UniFFI / WASM reference + parity matrix](../docs-site/src/content/docs/reference/)**
  — every command, tool, and binding, plus the cross-surface parity matrix and
  the error-code index.

## 💡 Explanation — *understanding why*

The ideas and rationale behind Memstead.

- **[Vision](../VISION.md)** — what Memstead is for and the design rationale.
- **[Glossary](../GLOSSARY.md)** — precise definitions (vault, schema,
  workspace, mount, storage backend, …).
- **[Prior art](../PRIOR_ART.md)** — how Memstead relates to adjacent tools.

---

Contributing to the docs (or the code)? See [CONTRIBUTING.md](../CONTRIBUTING.md).
Each mode is kept distinct on purpose — a how-to that detours into theory, or a
tutorial that turns into reference, serves neither reader; keep new pages in one
mode.
