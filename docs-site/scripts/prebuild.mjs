// Prebuild of the site's generated content (runs as `prebuild`; every
// output is gitignored — nothing here is a committed copy).
//
// 1. Reference: render the CLI, MCP, error-index, parity, binding and
//    WASM reference pages from the engine sources of THIS checkout by
//    running `cargo run -p xtask -- generate-docs` into the content
//    tree. The pages describe the commit being built, by construction:
//    there is no committed copy to regenerate, so nothing can drift and
//    no gate has to compare. A deploy image that builds the docs stage
//    without a Rust toolchain renders the reference in its own Rust
//    stage and hands the directory over via MEMSTEAD_DOCS_REFERENCE_DIR;
//    the handover is verified, never trusted, and an empty or missing
//    directory fails the build here rather than serving a site without
//    its reference.
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

import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
// The prose lint's frontmatter reader — the source of truth for how a skill's
// `description:` is resolved (handles `>` block scalars and colons inside plain
// scalars that strict YAML rejects), so the rendered roster stays byte-identical
// to what the plugin ships.
import { extractDescription } from "../../scripts/check-skill-prose.mjs";

const here = dirname(fileURLToPath(import.meta.url));

// --- 1. Reference ---
const workspaceRoot = resolve(here, "../..");
const referenceDest = resolve(here, "../src/content/docs/reference");
// Start from nothing: a page the generator no longer renders must not
// survive from an earlier build of this working tree.
rmSync(referenceDest, { recursive: true, force: true });
const prebuilt = process.env.MEMSTEAD_DOCS_REFERENCE_DIR;
if (prebuilt) {
  if (!existsSync(prebuilt)) {
    throw new Error(
      `prebuild: MEMSTEAD_DOCS_REFERENCE_DIR names ${prebuilt}, which does not exist. The stage ` +
        `that renders the reference did not run or did not hand it over; refusing to build a site ` +
        `without its reference.`,
    );
  }
  cpSync(prebuilt, referenceDest, { recursive: true });
  console.log(`prebuild: reference taken from ${prebuilt} (MEMSTEAD_DOCS_REFERENCE_DIR)`);
} else {
  const cargo = spawnSync(
    "cargo",
    ["run", "-q", "-p", "xtask", "--", "generate-docs", "--output", referenceDest],
    { cwd: workspaceRoot, stdio: "inherit" },
  );
  if (cargo.error) {
    throw new Error(
      `prebuild: cannot run cargo (${cargo.error.message}). The reference pages are ` +
        `rendered from the engine sources at build time, so the docs-site build needs a ` +
        `Rust toolchain — or a pre-rendered tree named by MEMSTEAD_DOCS_REFERENCE_DIR.`,
    );
  }
  if (cargo.status !== 0) {
    throw new Error(`prebuild: generate-docs failed (exit ${cargo.status}); the reference cannot be rendered from this tree`);
  }
  console.log(`prebuild: reference rendered from ${workspaceRoot} -> ${referenceDest}`);
}
// Whichever path filled it: the tree must carry pages. A generator that
// exited green but wrote nothing, or a handover directory that was never
// filled, would otherwise build a site whose sidebar points at 404s.
let referencePages;
try {
  referencePages = readdirSync(referenceDest, { recursive: true }).filter((f) => /\.md$/.test(String(f)));
} catch (e) {
  throw new Error(`prebuild: the reference tree ${referenceDest} is unreadable after generation: ${e.message}`);
}
if (referencePages.length === 0) {
  throw new Error(`prebuild: the reference tree ${referenceDest} holds no pages after generation; refusing to build a site without its reference`);
}
console.log(`prebuild: ${referencePages.length} reference page(s) in place`);

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
console.log(`prebuild: ${glossarySrc} -> ${glossaryDest}`);

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
    skills: ["ingest", "sync", "remodel", "tidy"],
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
    `skills roster drift: live skills [${actual.join(", ")}] != page roster [${expected.join(", ")}] — update the families map in scripts/prebuild.mjs`,
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
console.log(`prebuild: ${skillsDir}/*/SKILL.md -> ${skillsDest}`);

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
console.log(`prebuild: no hand-written skill counts (roster has ${skillCount})`);
