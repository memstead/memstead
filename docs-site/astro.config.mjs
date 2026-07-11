import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import remarkGfm from "remark-gfm";

export default defineConfig({
  // The CLI/MCP/etc. reference pages are machine-generated from clap
  // help prose, which routinely contains stray single tildes (`~/config`
  // paths, `~10` approximations). micromark's GFM strikethrough defaults
  // to `singleTilde: true`, so two such tildes wrap everything between
  // them in one `<del>` — striking through whole sections. Re-register
  // remark-gfm with `singleTilde: false` so only `~~double~~` strikes.
  markdown: {
    remarkPlugins: [[remarkGfm, { singleTilde: false }]],
  },
  // GitHub Pages publishes from `<org>.github.io/memstead/` by default;
  // DOCS_SITE / DOCS_BASE override both for other hosts (e.g. the
  // memstead.com image builds this site with DOCS_SITE=https://memstead.com
  // DOCS_BASE=/dev and serves it under /dev) without changing the docs
  // build itself.
  site: process.env.DOCS_SITE ?? "https://memstead.github.io",
  base: process.env.DOCS_BASE ?? "/memstead",
  integrations: [
    starlight({
      title: "Memstead Docs",
      description:
        "Guides plus auto-generated reference for the Memstead engine's MCP, CLI, UniFFI, WASM, and Registry HTTP surfaces.",
      components: {
        Footer: "./src/components/Footer.astro",
      },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/memstead/memstead",
        },
      ],
      sidebar: [
        {
          label: "Overview",
          link: "/",
        },
        {
          label: "Guides",
          items: [
            { label: "Getting started", link: "/guides/getting-started/" },
            { label: "Author a schema", link: "/guides/author-a-schema/" },
            { label: "Publish a mem", link: "/guides/publish-a-mem/" },
            { label: "Declare an ingest", link: "/guides/declare-an-ingest/" },
            { label: "Agent recipes", link: "/guides/agent-recipes/" },
          ],
        },
        {
          label: "Concepts",
          items: [
            // Built from ../GLOSSARY.md at prebuild (scripts/copy-openapi.mjs).
            { label: "Glossary", link: "/glossary/" },
            { label: "The fidelity contract", link: "/concepts/fidelity-contract/" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "MCP tools", link: "/reference/mcp/" },
            { label: "CLI", link: "/reference/cli/cli/" },
            { label: "UniFFI surface", link: "/reference/uniffi/" },
            { label: "WASM surface", link: "/reference/wasm/" },
            { label: "Registry HTTP", link: "/reference/registry/" },
            { label: "Surface Parity Matrix", link: "/reference/parity/" },
            { label: "Error Code Index", link: "/reference/errors/" },
          ],
        },
      ],
    }),
  ],
});
