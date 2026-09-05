#!/usr/bin/env node
// The one frontmatter reader for non-entity documents: a skill's SKILL.md,
// a docs page. Entity frontmatter is the engine's (memstead-base's core
// split, reached by Rust callers directly and by scripts through the CLI's
// JSON output); this helper carries the same contract for the YAML block a
// SKILL.md opens with, so no script in either tree does its own delimiter
// arithmetic:
//
//   * a byte-order mark is skipped;
//   * the opening fence is `---` on the FIRST line, LF or CRLF; a fence
//     anywhere later is not frontmatter and the whole text is body;
//   * the block closes at the first line that is exactly `---`; an
//     unclosed block is not frontmatter either;
//   * the body starts after the closing fence's line break.
//
// Library: `parseFrontmatter(text)` → `{ meta, body }` or `null`;
// `frontmatterField(meta, key)` → the raw value of the first `key:` line
// (block scalars are the caller's concern; see `extractDescription` in
// check-skill-prose.mjs). Command line, for shell callers:
//
//   node scripts/frontmatter.mjs --meta  FILE    # the block, no fences
//   node scripts/frontmatter.mjs --body  FILE    # everything after it
//   node scripts/frontmatter.mjs --field KEY FILE
//
// A file without frontmatter prints nothing for --meta and --field and the
// whole file for --body; exit 0 either way, 2 on misuse.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const FENCE = /^---(\r?\n)/;

export function parseFrontmatter(text) {
  const src = text.startsWith("﻿") ? text.slice(1) : text;
  const open = src.match(FENCE);
  if (!open) return null;
  const rest = src.slice(open[0].length);
  // The closing fence is a whole line: `---` at a line start followed by a
  // line break or the end of the text.
  const close = rest.match(/(^|\n)---(\r?\n|$)/);
  if (!close) return null;
  const at = close.index + (close[1] ? 1 : 0);
  let meta = rest.slice(0, close.index === 0 ? 0 : at - 1);
  if (meta.endsWith("\r")) meta = meta.slice(0, -1);
  const body = rest.slice(at + 3 + close[2].length);
  return { meta, body };
}

/// Render a frontmatter block over `body`: the fence, the meta lines as
/// given (the caller owns their YAML), the fence, then the body. The one
/// writer beside the one reader, so a generated page never spells the
/// fence itself.
export function renderFrontmatter(meta, body) {
  const block = meta.endsWith("\n") ? meta : meta + "\n";
  return `---\n${block}---\n${body}`;
}

export function frontmatterField(meta, key) {
  for (const line of meta.split(/\r?\n/)) {
    const m = line.match(/^([A-Za-z0-9_-]+):\s*(.*)$/);
    if (m && m[1] === key) return m[2];
  }
  return null;
}

function usage() {
  process.stderr.write(
    "usage: frontmatter.mjs --meta FILE | --body FILE | --field KEY FILE\n",
  );
  return 2;
}

export function main(argv) {
  const [mode, ...rest] = argv;
  let key = null;
  let file = null;
  if (mode === "--field") [key, file] = rest;
  else if (mode === "--meta" || mode === "--body") [file] = rest;
  else return usage();
  if (!file || (mode === "--field" && !key)) return usage();
  const text = readFileSync(file, "utf8");
  const parsed = parseFrontmatter(text);
  if (mode === "--body") {
    process.stdout.write(parsed ? parsed.body : text);
    return 0;
  }
  if (!parsed) return 0;
  if (mode === "--meta") {
    process.stdout.write(parsed.meta + "\n");
    return 0;
  }
  const value = frontmatterField(parsed.meta, key);
  if (value !== null) process.stdout.write(value + "\n");
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(main(process.argv.slice(2)));
}
