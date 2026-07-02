// Prebuild sync of generated/normative artefacts into the site
// (runs as `prebuild`; both outputs are gitignored).
//
// 1. OpenAPI: copy the generated OpenAPI document from the reference
//    content collection into the Astro `public/` directory so it's
//    served from the site root at `/openapi.json`. The source lives
//    next to the rendered Markdown so the registry reference page can
//    link to it as a sibling artefact; the deploy-time copy exposes it
//    at the canonical publication path. xtask writes the source on
//    every regenerate; this script keeps the served copy in sync
//    without re-running xtask just to refresh `public/`.
//
// 2. Glossary: render the repo-root GLOSSARY.md as a docs page at
//    `/glossary/`. GLOSSARY.md is normative and stays the single
//    source of truth at the repo root; the site carries a build-time
//    copy rather than a committed duplicate so the two can never
//    drift. The transform injects Starlight frontmatter, drops the
//    duplicate H1, and points the repo-relative VISION.md links at
//    GitHub.

import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

// --- 1. OpenAPI ---
const openapiSrc = `${here}/../src/content/docs/reference/openapi.json`;
const openapiDest = `${here}/../public/openapi.json`;
mkdirSync(dirname(openapiDest), { recursive: true });
copyFileSync(openapiSrc, openapiDest);
console.log(`copy-openapi: ${openapiSrc} -> ${openapiDest}`);

// --- 2. Glossary ---
const glossarySrc = `${here}/../../GLOSSARY.md`;
const glossaryDest = `${here}/../src/content/docs/glossary.md`;
const body = readFileSync(glossarySrc, "utf8")
  .replace(/^# Glossary\n/, "")
  .replaceAll("](VISION.md", "](https://github.com/memstead/memstead/blob/main/VISION.md");
const frontmatter = `---
title: Glossary
description: "Normative definitions of Memstead's technical vocabulary — mem, schema, workspace, mount, entity, storage backend, and the rest."
---

> This page is built from [GLOSSARY.md](https://github.com/memstead/memstead/blob/main/GLOSSARY.md) at the repository root — the normative source. Definitions here override any older wording elsewhere.

`;
writeFileSync(glossaryDest, frontmatter + body);
console.log(`copy-openapi: ${glossarySrc} -> ${glossaryDest}`);
