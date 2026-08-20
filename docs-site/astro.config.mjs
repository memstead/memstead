import { execFileSync } from "node:child_process";
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import remarkGfm from "remark-gfm";

// The generation stamp every page footer carries. The deploy injects
// `PUBLIC_GENERATION_SHA` / `PUBLIC_GENERATION_DATE`; a build without
// them derives the same facts from git rather than printing a
// placeholder.
//
// The placeholder is what this replaces: every published page read
// `Generated from dev on unbuilt`, because the fallbacks were words
// that render like data. On a site whose pitch is "generated
// deterministically from the live source on every push", the one line
// that says WHICH source was the one line that never did — and a
// reader trying to tell whether the reference pages describe the
// release they installed had nothing to go on.
//
// When neither the environment nor git can answer, the build fails.
// An unattributed page is worse than no page: it looks authoritative
// and cannot be checked.
function generationStamp() {
  const fromGit = (args) => {
    try {
      return execFileSync("git", args, { encoding: "utf8" }).trim() || null;
    } catch {
      return null;
    }
  };
  const sha = process.env.PUBLIC_GENERATION_SHA || fromGit(["rev-parse", "--short", "HEAD"]);
  // Normalised to the calendar date. The deploy passes a full ISO
  // datetime with offset (`%cI`) and the git fallback a bare date
  // (`%cs`), so without this the published footer and a local build
  // would state the same fact in two shapes — and the footer's whole
  // job is to be a fact a reader can compare.
  const rawDate =
    process.env.PUBLIC_GENERATION_DATE || fromGit(["log", "-1", "--format=%cs"]);
  const date = rawDate ? rawDate.slice(0, 10) : rawDate;
  if (!sha || !date) {
    throw new Error(
      "docs-site: cannot determine the generation stamp. Set " +
        "PUBLIC_GENERATION_SHA and PUBLIC_GENERATION_DATE, or build inside a " +
        "git checkout. Refusing to publish pages that cannot name the " +
        "revision they were generated from.",
    );
  }
  return { sha, date };
}

const generation = generationStamp();

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
        "Guides plus auto-generated reference for the Memstead engine's binding format, MCP, CLI, WASM, and Registry HTTP surfaces.",
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
            { label: "Grow a mem from a source", link: "/guides/grow-a-mem-from-a-source/" },
            { label: "Author a schema", link: "/guides/author-a-schema/" },
            { label: "Publish a mem", link: "/guides/publish-a-mem/" },
            { label: "Agent recipes", link: "/guides/agent-recipes/" },
            { label: "Back up a mem-repo", link: "/guides/back-up-a-mem-repo/" },
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
            { label: "Skills", link: "/skills/" },
            { label: "Binding format", link: "/reference/binding/" },
            { label: "MCP tools", link: "/reference/mcp/" },
            { label: "CLI", link: "/reference/cli/cli/" },
            { label: "WASM surface", link: "/reference/wasm/" },
            { label: "Registry HTTP", link: "/reference/registry/" },
            { label: "Surface Parity Matrix", link: "/reference/parity/" },
            { label: "Error Code Index", link: "/reference/errors/" },
          ],
        },
      ],
    }),
  ],
  // The stamp reaches the Footer as compile-time constants rather than
  // as `import.meta.env`, so an unset variable is a build failure above
  // (where it can say why) instead of a silent fallback in a component.
  vite: {
    define: {
      __GENERATION_SHA__: JSON.stringify(generation.sha),
      __GENERATION_DATE__: JSON.stringify(generation.date),
    },
  },
});
