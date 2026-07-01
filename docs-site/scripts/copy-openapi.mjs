// Copy the generated OpenAPI document from the reference content
// collection into the Astro `public/` directory so it's served from
// the site root at `/openapi.json`. The source lives next to the
// rendered Markdown so the registry reference page can link to it as
// a sibling artefact; the deploy-time copy exposes it at the canonical
// publication path AC 5 names.
//
// xtask writes the source on every regenerate; this script keeps the
// served copy in sync without re-running xtask just to refresh
// `public/`.

import { copyFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = `${here}/../src/content/docs/reference/openapi.json`;
const dest = `${here}/../public/openapi.json`;

mkdirSync(dirname(dest), { recursive: true });
copyFileSync(src, dest);
console.log(`copy-openapi: ${src} -> ${dest}`);
