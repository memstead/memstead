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
//
// 3. Skills roster: render the eight-skill plugin roster page at
//    `/skills/` from the SKILL.md frontmatter — the shipped skill
//    descriptions ARE the job map (adversarially reviewed as a plugin
//    gate), so the page is generated from them rather than hand-copied,
//    and cannot drift. The generator reads the live skill directories,
//    asserts the roster is exactly the expected eight (an added or
//    removed skill fails the build), and derives each skill's invocation
//    posture from its frontmatter keys.

import { copyFileSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
// The plugin's own frontmatter reader — the source of truth for how a skill's
// `description:` is resolved (handles `>` block scalars and colons inside plain
// scalars that strict YAML rejects), so the rendered roster stays byte-identical
// to what the plugin ships.
import { extractDescription } from "../../plugins/claude-code/scripts/check-skill-prose.mjs";

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

// --- 3. Skills roster ---
const skillsDir = `${here}/../../plugins/claude-code/skills`;
// The two families (agent-surfaces.md). Membership and ordering are editorial;
// every drift-sensitive fact — the roster set, each description, each invocation
// posture — is read from the live SKILL.md frontmatter below.
const families = [
  {
    title: "Onboarding & context",
    blurb: "Getting a workspace started and its knowledge in.",
    skills: ["setup", "interview", "learn"],
  },
  {
    title: "The mem lifecycle",
    blurb: "Building a mem from sources, then keeping it true.",
    skills: ["ingest", "sync", "verify", "tidy", "commit"],
  },
];

function readSkill(name) {
  const raw = readFileSync(`${skillsDir}/${name}/SKILL.md`, "utf8");
  const m = raw.match(/^---\n([\s\S]*?)\n---\n/);
  if (!m) throw new Error(`skills roster: ${name}/SKILL.md has no frontmatter`);
  const frontmatter = m[1];
  const description = extractDescription(frontmatter).trim();
  if (!description) throw new Error(`skills roster: ${name}/SKILL.md has no description`);
  // Invocation posture from the two inverse frontmatter keys (plugin CLAUDE.md).
  let posture = "Both-invocable";
  if (/^disable-model-invocation:\s*true\s*$/m.test(frontmatter)) {
    posture = "Human-only (front door)";
  } else if (/^user-invocable:\s*false\s*$/m.test(frontmatter)) {
    posture = "Model-only";
  }
  return { name, description, posture };
}

// Fail the build if the live roster is not exactly the expected eight — an added
// or removed skill must be reflected here, so the page can never claim a stale set.
const expected = families.flatMap((f) => f.skills).sort();
const actual = readdirSync(skillsDir, { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => d.name)
  .sort();
if (JSON.stringify(expected) !== JSON.stringify(actual)) {
  throw new Error(
    `skills roster drift: live skills [${actual.join(", ")}] != page roster [${expected.join(", ")}] — update the families map in scripts/copy-openapi.mjs`,
  );
}

let skillsBody = `---
title: Skills
description: "The eight-skill Memstead plugin roster in two families — onboarding & context and the mem lifecycle — with each skill's invocation posture and its shipped description."
---

> This page is generated from the plugin \`SKILL.md\` frontmatter at build time — the shipped skill descriptions are the source of truth, so the roster here cannot drift from the installed plugin.

The Claude Code plugin ships **eight skills in two families**. \`/setup\` and \`/interview\` are the human-driven front doors; the rest are both-invocable — usable from the \`/\` menu and auto-invocable by the model. There is no command for everyday graph work: once a workspace exists, you just talk to Claude and the \`memstead_*\` MCP tools stay live.

`;
for (const family of families) {
  skillsBody += `## ${family.title}\n\n${family.blurb}\n\n`;
  for (const name of family.skills) {
    const s = readSkill(name);
    skillsBody += `### \`/${s.name}\`\n\n_${s.posture}_\n\n${s.description}\n\n`;
  }
}
const skillsDest = `${here}/../src/content/docs/skills.md`;
writeFileSync(skillsDest, skillsBody);
console.log(`copy-openapi: ${skillsDir}/*/SKILL.md -> ${skillsDest}`);
