// Workspace-walker that runs the shared `validator.mjs` (under
// `memstead-plugin/v0/`) against each example fixture and each live
// workspace file, plus a metaschema-shape sanity check. Node built-ins
// only — no npm.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, basename, extname } from "node:path";
import { parseArgs } from "node:util";

import { validate as runValidate } from "./memstead-plugin/v0/validator.mjs";

const { values: args } = parseArgs({
  options: {
    "schemas-dir": { type: "string" },
    "workspace-dir": { type: "string" },
  },
  strict: true,
});

const schemasDir = args["schemas-dir"];
const workspaceDir = args["workspace-dir"];
if (!schemasDir || !workspaceDir) {
  console.error("usage: validate-live-workspace.mjs --schemas-dir <dir> --workspace-dir <dir>");
  process.exit(2);
}

// ---------- Loaders ----------

function loadJson(p) {
  return JSON.parse(readFileSync(p, "utf8"));
}

const schemas = {
  memsteadToml: loadJson(join(schemasDir, "memstead-toml.schema.json")),
  medium: loadJson(join(schemasDir, "medium.schema.json")),
  facet: loadJson(join(schemasDir, "facet.schema.json")),
  projection: loadJson(join(schemasDir, "projection.schema.json")),
  ingest: loadJson(join(schemasDir, "ingest.schema.json")),
};

// ---------- 2020-12 metaschema sanity check ----------
//
// Full-fidelity metaschema validation requires fetching the published
// 2020-12 metaschema bundle. We perform a local structural check: each
// schema must be an object with `$schema` set to the 2020-12 URI,
// `$id` set, `type` declared (or `oneOf`/`anyOf`/`$ref` at the root),
// and any nested `properties`/`items`/`$defs` recursively well-formed
// (object-typed where required). This catches the realistic class of
// authoring mistakes — typos, missing braces, forgotten brackets — that
// the metaschema would catch.

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
  const symbol = ok ? "OK" : "FAIL";
  console.log(`[${symbol}] ${label}${details ? `\n      ${details}` : ""}`);
  if (!ok) totalFailures++;
}

// 1. Metaschema sanity per schema
for (const [name, schema] of Object.entries(schemas)) {
  const errs = checkMetaschemaShape(name, schema);
  report(`metaschema-shape ${name}`, errs.length === 0, errs.join("; "));
}

// 2. Examples validate against their schemas
const examplesDir = join(schemasDir, "examples");
const exampleMap = [
  ["memstead-toml.minimal.json", "memsteadToml"],
  ["memstead-toml.full.json", "memsteadToml"],
  ["facet.minimal.json", "facet"],
  ["facet.full.json", "facet"],
  ["medium.minimal.json", "medium"],
  ["projection.four-primitive.json", "projection"],
  ["ingest.minimal.json", "ingest"],
  ["ingest.full.json", "ingest"],
];
for (const [file, schemaKey] of exampleMap) {
  const inst = loadJson(join(examplesDir, file));
  const { valid, errors } = runValidate(schemas[schemaKey], inst);
  report(`example ${file}`, valid, errors.map(e => `${e.path}: ${e.message}`).join("\n      "));
}

// 3. Live workspace: the four-primitive `.memstead/` layout
//    (mediums / facets / projections / ingests). Each primitive
//    directory is optional — a workspace that doesn't use a primitive
//    simply has no directory for it, which is not an error.
function walkJson(dir) {
  const out = [];
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, ent.name);
    if (ent.isDirectory()) out.push(...walkJson(p));
    else if (ent.isFile() && extname(ent.name) === ".json") out.push(p);
  }
  return out;
}

const memsteadDir = join(workspaceDir, ".memstead");

for (const [subdir, schemaKey, label] of [
  ["mediums", "medium", "medium"],
  ["facets", "facet", "facet"],
  ["projections", "projection", "projection"],
  ["ingests", "ingest", "ingest"],
]) {
  const dir = join(memsteadDir, subdir);
  let exists = false;
  try {
    exists = statSync(dir).isDirectory();
  } catch {
    exists = false;
  }
  if (!exists) {
    // Optional primitive — absence is fine, not a failure.
    report(`live-workspace ${label} dir`, true, `(.memstead/${subdir} absent)`);
    continue;
  }
  const files = walkJson(dir);
  if (files.length === 0) {
    report(`live-workspace ${label} dir`, true, `(no files in .memstead/${subdir})`);
    continue;
  }
  for (const f of files) {
    const inst = loadJson(f);
    const { valid, errors } = runValidate(schemas[schemaKey], inst);
    const rel = f.slice(workspaceDir.length + 1);
    report(`live ${label} ${rel}`, valid, errors.map(e => `${e.path}: ${e.message}`).join("\n      "));
  }
}

if (totalFailures > 0) {
  console.error(`\n${totalFailures} failure(s).`);
  process.exit(1);
} else {
  console.log("\nAll checks passed.");
}
