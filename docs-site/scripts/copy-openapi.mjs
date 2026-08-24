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
// 3. Skills roster: render the plugin roster page at
//    `/skills/` from the SKILL.md frontmatter — the shipped skill
//    descriptions ARE the job map (adversarially reviewed as a plugin
//    gate), so the page is generated from them rather than hand-copied,
//    and cannot drift. The generator reads the live skill directories,
//    asserts the roster is exactly the expected set (an added or
//    removed skill fails the build), and derives each skill's invocation
//    posture from its frontmatter keys.

import { copyFileSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
// The prose lint's frontmatter reader — the source of truth for how a skill's
// `description:` is resolved (handles `>` block scalars and colons inside plain
// scalars that strict YAML rejects), so the rendered roster stays byte-identical
// to what the plugin ships.
import { extractDescription } from "../../scripts/check-skill-prose.mjs";

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
    skills: ["ingest", "sync", "tidy"],
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

// Fail the build if the live roster is not exactly the expected roster — an added
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

// The roster's size, as the word the prose uses. Derived rather than
// typed, because the count is exactly the kind of fact that rots: the
// generated page said "six" while the hand-written index said "eight",
// on a site whose pitch is that generated surfaces cannot drift. A
// number a human maintains is a number that will eventually be wrong.
const NUMBER_WORDS = [
  "zero", "one", "two", "three", "four", "five", "six", "seven",
  "eight", "nine", "ten", "eleven", "twelve",
];
const skillCount = actual.length;
const skillCountWord = NUMBER_WORDS[skillCount] ?? String(skillCount);

// No hand-written page may state a skill count at all. The index used
// to, and contradicted the generated page it linked to. Deleting the
// claim is the fix (the roster page is one click away and always
// right); this guard is what keeps it deleted.
const HANDWRITTEN_DOCS = `${here}/../src/content/docs`;
const skillsDest = `${HANDWRITTEN_DOCS}/skills.md`;
const COUNT_CLAIM =
  /\b(zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|\d+)[- ]skills?\b/i;
function scanForSkillCounts(dir) {
  const offenders = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = `${dir}/${entry.name}`;
    if (entry.isDirectory()) {
      offenders.push(...scanForSkillCounts(full));
      continue;
    }
    // skills.md is the generated roster page — it states the count
    // because it derives it, one line below.
    if (full === skillsDest) continue;
    if (!/\.mdx?$/.test(entry.name)) continue;
    const text = readFileSync(full, "utf8");
    for (const line of text.split("\n")) {
      const hit = line.match(COUNT_CLAIM);
      if (!hit) continue;
      // A claim that happens to be right is still hand-maintained, but
      // a wrong one is the actual defect — report both, name which.
      offenders.push({ file: full, line: line.trim(), claim: hit[1] });
    }
  }
  return offenders;
}

let skillsBody = `---
title: Skills
description: "The ${skillCountWord}-skill Memstead plugin roster in two families — onboarding & context and the mem lifecycle — with each skill's invocation posture and its shipped description."
---

> This page is generated from the plugin \`SKILL.md\` frontmatter at build time — the shipped skill descriptions are the source of truth, so the roster here cannot drift from the installed plugin.

The Claude Code plugin ships **${skillCountWord} skills in two families**. \`/setup\` and \`/interview\` are the human-driven front doors; the rest are both-invocable — usable from the \`/\` menu and auto-invocable by the model. Fidelity measurement is \`/sync --verify\`; the on-demand full stock-take is \`/sync --inventory\`. There is no command for everyday graph work: once a workspace exists and the session has started with the MCP server wired, you just talk to Claude and the \`memstead_*\` MCP tools stay live. A session that is already running picks the new skills up only after \`/reload-plugins\` or a restart. Restart the agent session afterwards: a session that is already running does not attach an MCP server added while it runs.

`;
for (const family of families) {
  skillsBody += `## ${family.title}\n\n${family.blurb}\n\n`;
  for (const name of family.skills) {
    const s = readSkill(name);
    skillsBody += `### \`/${s.name}\`\n\n_${s.posture}_\n\n${s.description}\n\n`;
  }
}
writeFileSync(skillsDest, skillsBody);
console.log(`copy-openapi: ${skillsDir}/*/SKILL.md -> ${skillsDest}`);

const countOffenders = scanForSkillCounts(HANDWRITTEN_DOCS);
if (countOffenders.length > 0) {
  const listed = countOffenders
    .map((o) => `  ${o.file}\n    claims "${o.claim}" skills: ${o.line}`)
    .join("\n");
  throw new Error(
    `hand-written skill count found (the live roster has ${skillCount}). The ` +
      `roster page derives its count; a page that states one of its own will ` +
      `eventually contradict it — as the index did, saying "eight" against a ` +
      `generated "six". Remove the number and link to /skills/ instead.\n${listed}`,
  );
}
console.log(`copy-openapi: no hand-written skill counts (roster has ${skillCount})`);
