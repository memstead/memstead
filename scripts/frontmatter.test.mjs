// The non-entity frontmatter helper keeps the core's contract: CRLF and LF
// documents split, a byte-order mark is skipped, the fence counts only on
// the first line, an unclosed block is body, and the command line prints
// the same slices a caller gets from the library.

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { parseFrontmatter, frontmatterField, renderFrontmatter } from "./frontmatter.mjs";

const HELPER = fileURLToPath(new URL("./frontmatter.mjs", import.meta.url));

test("splits an LF document at its first-line fence", () => {
  const doc = "---\nname: sync\ndescription: keep it true\n---\n# Body\n\nProse.\n";
  assert.deepEqual(parseFrontmatter(doc), {
    meta: "name: sync\ndescription: keep it true",
    body: "# Body\n\nProse.\n",
  });
  assert.equal(frontmatterField("name: sync\ndescription: keep it true", "description"), "keep it true");
  assert.equal(frontmatterField("name: sync", "missing"), null);
});

test("splits a CRLF document and a byte-order-marked one identically", () => {
  const doc = "---\r\nname: sync\r\ndescription: d\r\n---\r\nBody\r\n";
  assert.deepEqual(parseFrontmatter(doc), { meta: "name: sync\r\ndescription: d", body: "Body\r\n" });
  assert.deepEqual(parseFrontmatter("﻿" + doc), parseFrontmatter(doc));
  assert.equal(frontmatterField("name: sync\r\ndescription: d", "description"), "d");
});

test("a fence anywhere but the first line is body, and an unclosed block is body", () => {
  assert.equal(parseFrontmatter("# Intro\n\n---\nname: x\n---\n"), null);
  assert.equal(parseFrontmatter("---\nname: x\nno close\n"), null);
  assert.equal(parseFrontmatter(""), null);
  // An empty block closes on the very next line.
  assert.deepEqual(parseFrontmatter("---\n---\nbody"), { meta: "", body: "body" });
  // A fence at the end of the text closes the block with no body.
  assert.deepEqual(parseFrontmatter("---\nname: x\n---"), { meta: "name: x", body: "" });
});

test("the command line prints the same slices", () => {
  const dir = mkdtempSync(join(tmpdir(), "fm-"));
  const file = join(dir, "SKILL.md");
  writeFileSync(file, "---\nname: sync\ndescription: keep it true\n---\n# Body\n");
  const run = (...args) => spawnSync(process.execPath, [HELPER, ...args], { encoding: "utf8" });
  assert.equal(run("--field", "description", file).stdout, "keep it true\n");
  assert.equal(run("--meta", file).stdout, "name: sync\ndescription: keep it true\n");
  assert.equal(run("--body", file).stdout, "# Body\n");
  const plain = join(dir, "plain.md");
  writeFileSync(plain, "# No frontmatter\n");
  assert.equal(run("--field", "description", plain).stdout, "");
  assert.equal(run("--body", plain).stdout, "# No frontmatter\n");
  assert.equal(run("--nonsense").status, 2);
});

test("renderFrontmatter round-trips through parseFrontmatter", () => {
  const meta = "title: Glossary\ndescription: words";
  const doc = renderFrontmatter(meta, "\n# Body\n");
  assert.deepEqual(parseFrontmatter(doc), { meta, body: "\n# Body\n" });
  assert.equal(renderFrontmatter(meta + "\n", ""), renderFrontmatter(meta, ""));
});
