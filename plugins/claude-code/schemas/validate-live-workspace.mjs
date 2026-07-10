// Version-generic workspace walker. Given a `--schemas-dir` (a
// `memstead-plugin/<version>/` directory), it:
//
//   1. metaschema-shape-checks every `*.schema.json` in the dir,
//   2. validates every fixture under `examples/` against the schema its
//      filename prefix names (`binding.*.json` → `binding.schema.json`),
//   3. (optional) walks a live `--workspace-dir`'s `.memstead/` layout and
//      validates each primitive file against its schema.
//
// It adapts to the version by what schemas are present: a v1 dir has a
// `binding` schema and no `ingest`, so `.memstead/projections/` validates
// against the binding schema and there is no `ingests/` leg; a v0 dir has
// `projection` + `ingest`. `--workspace-dir` is optional — omit it to run
// only the schema + example checks (the canonical-suite mode, which needs no
// live workspace). Node built-ins only — no npm.
//
// The runtime validator is version-agnostic; this script loads the one that
// ships alongside the target schemas (`<schemas-dir>/validator.mjs`).

import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, extname, basename } from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";

const { values: args } = parseArgs({
  options: {
    "schemas-dir": { type: "string" },
    "workspace-dir": { type: "string" },
  },
  strict: true,
});

const schemasDir = args["schemas-dir"];
const workspaceDir = args["workspace-dir"];
if (!schemasDir) {
  console.error("usage: validate-live-workspace.mjs --schemas-dir <dir> [--workspace-dir <dir>]");
  process.exit(2);
}

// ---------- Loaders ----------

function loadJson(p) {
  return JSON.parse(readFileSync(p, "utf8"));
}

// The validator that ships alongside the target schemas (version-agnostic
// code, but colocated per version so each version stays self-contained).
const { validate: runValidate } = await import(
  pathToFileURL(join(schemasDir, "validator.mjs")).href
);

// Discover the schemas: `<key>.schema.json` → key (e.g. "binding", "medium").
const schemas = {};
for (const ent of readdirSync(schemasDir)) {
  if (ent.endsWith(".schema.json")) {
    schemas[ent.slice(0, -".schema.json".length)] = loadJson(join(schemasDir, ent));
  }
}

// ---------- 2020-12 metaschema sanity check ----------
//
// Local structural check: each schema must be an object with `$schema` set
// to the 2020-12 URI, `$id` set, and any nested `properties`/`items`/`$defs`
// well-formed. Catches the realistic authoring mistakes (typos, missing
// braces) the published metaschema would catch, without fetching it.

function checkMetaschemaShape(name, schema) {
  const errs = [];
  if (typeof schema !== "object" || schema === null) {
    errs.push("schema must be an object");
    return errs;
  }
  if (schema.$schema !== "https://json-schema.org/draft/2020-12/schema") {
    errs.push(`$schema must be 'https://json-schema.org/draft/2020-12/schema', got ${JSON.stringify(schema.$schema)}`);
  }
  if (typeof schema.$id !== "string" || schema.$id.length === 0) {
    errs.push("$id must be a non-empty string");
  }
  function walk(node, path) {
    if (typeof node !== "object" || node === null || Array.isArray(node)) return;
    for (const [k, v] of Object.entries(node)) {
      if (k === "properties" || k === "$defs" || k === "patternProperties") {
        if (typeof v !== "object" || v === null || Array.isArray(v)) {
          errs.push(`${path}.${k} must be an object`);
          continue;
        }
        for (const [ck, cv] of Object.entries(v)) walk(cv, `${path}.${k}.${ck}`);
      } else if (k === "items" || k === "additionalProperties") {
        if (typeof v === "object") walk(v, `${path}.${k}`);
      } else if (k === "oneOf" || k === "anyOf" || k === "allOf") {
        if (!Array.isArray(v)) {
          errs.push(`${path}.${k} must be an array`);
          continue;
        }
        v.forEach((sub, i) => walk(sub, `${path}.${k}[${i}]`));
      } else if (typeof v === "object" && v !== null && !Array.isArray(v)) {
        walk(v, `${path}.${k}`);
      }
    }
  }
  walk(schema, name);
  return errs;
}

// ---------- Run the checks ----------

let totalFailures = 0;
function report(label, ok, details) {
  console.log(`[${ok ? "OK" : "FAIL"}] ${label}${details ? `\n      ${details}` : ""}`);
  if (!ok) totalFailures++;
}

// 1. Metaschema sanity per schema.
for (const [name, schema] of Object.entries(schemas)) {
  const errs = checkMetaschemaShape(name, schema);
  report(`metaschema-shape ${name}`, errs.length === 0, errs.join("; "));
}

// 2. Examples validate against the schema their filename prefix names
//    (`binding.from-init.json` → key `binding`).
const examplesDir = join(schemasDir, "examples");
if (existsSync(examplesDir)) {
  for (const file of readdirSync(examplesDir).sort()) {
    if (extname(file) !== ".json") continue;
    const key = basename(file).split(".")[0];
    const schema = schemas[key];
    if (!schema) {
      report(`example ${file}`, false, `no schema for prefix '${key}'`);
      continue;
    }
    const { valid, errors } = runValidate(schema, loadJson(join(examplesDir, file)));
    report(`example ${file}`, valid, errors.map(e => `${e.path}: ${e.message}`).join("\n      "));
  }
}

// 3. Optional live workspace: the `.memstead/` layout. Each primitive dir is
//    optional. `projections/` validates against the binding schema when the
//    version has one (v1), else the projection schema (v0). `ingests/` is
//    walked only where the version still has an ingest schema (v0).
function walkJson(dir) {
  const out = [];
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, ent.name);
    if (ent.isDirectory()) out.push(...walkJson(p));
    else if (ent.isFile() && extname(ent.name) === ".json") out.push(p);
  }
  return out;
}

if (workspaceDir) {
  const memsteadDir = join(workspaceDir, ".memstead");
  const layout = [
    ["mediums", "medium"],
    ["facets", "facet"],
    ["projections", schemas.binding ? "binding" : "projection"],
  ];
  if (schemas.ingest) layout.push(["ingests", "ingest"]);

  for (const [subdir, schemaKey] of layout) {
    const dir = join(memsteadDir, subdir);
    let exists = false;
    try {
      exists = statSync(dir).isDirectory();
    } catch {
      exists = false;
    }
    if (!exists) {
      report(`live-workspace ${subdir}`, true, `(.memstead/${subdir} absent)`);
      continue;
    }
    const files = walkJson(dir);
    if (files.length === 0) {
      report(`live-workspace ${subdir}`, true, `(no files in .memstead/${subdir})`);
      continue;
    }
    for (const f of files) {
      const { valid, errors } = runValidate(schemas[schemaKey], loadJson(f));
      const rel = f.slice(workspaceDir.length + 1);
      report(`live ${schemaKey} ${rel}`, valid, errors.map(e => `${e.path}: ${e.message}`).join("\n      "));
    }
  }
}

if (totalFailures > 0) {
  console.error(`\n${totalFailures} failure(s).`);
  process.exit(1);
} else {
  console.log("\nAll checks passed.");
}
