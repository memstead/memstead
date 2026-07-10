// Tests for the memstead-plugin/v1 schemas via the shared validator.
//
// The keyword-level validator coverage lives in v0/validator.test.mjs (the
// validator is version-agnostic). This suite is the v1 CONSUMER: every v1
// example must validate against its schema, the init-emitted binding golden
// must validate against binding.schema.json (one half of the round-trip pin —
// the other half, that `projection init` actually emits this golden, is a
// Rust test in memstead-cli), and the v1 refusals must bite.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { validate } from "./validator.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));

function loadJson(rel) {
  return JSON.parse(readFileSync(join(__dirname, rel), "utf8"));
}

// ── Every v1 example validates against its schema ────────────────────

describe("memstead-plugin/v1 — examples validate", () => {
  const cases = [
    ["binding.schema.json", "examples/binding.minimal.json"],
    ["binding.schema.json", "examples/binding.full.json"],
    ["binding.schema.json", "examples/binding.from-init.json"],
    ["medium.schema.json", "examples/medium.minimal.json"],
    ["facet.schema.json", "examples/facet.minimal.json"],
    ["facet.schema.json", "examples/facet.full.json"],
    ["memstead-toml.schema.json", "examples/memstead-toml.minimal.json"],
    ["memstead-toml.schema.json", "examples/memstead-toml.full.json"],
  ];
  for (const [schemaFile, exampleFile] of cases) {
    it(`${exampleFile} validates against ${schemaFile}`, () => {
      const r = validate(loadJson(schemaFile), loadJson(exampleFile));
      assert.equal(r.valid, true, `errors: ${JSON.stringify(r.errors, null, 2)}`);
    });
  }
});

// ── Round-trip pin (JS half): init-emitted golden validates ──────────

describe("memstead-plugin/v1 — round-trip pin", () => {
  it("the `projection init` golden validates against binding.schema.json", () => {
    // If this fails, the schema drifted from what the engine emits. The Rust
    // half (memstead-cli) pins that init still emits exactly this golden, so
    // the two tests together keep schema and engine from drifting apart.
    const r = validate(loadJson("binding.schema.json"), loadJson("examples/binding.from-init.json"));
    assert.equal(r.valid, true, `errors: ${JSON.stringify(r.errors, null, 2)}`);
  });
});

// ── Refusals ─────────────────────────────────────────────────────────

describe("memstead-plugin/v1 — refusals", () => {
  it("rejects a `.memstead.toml` with an unsupported format constant", () => {
    const schema = loadJson("memstead-toml.schema.json");
    const r = validate(schema, { format: "memstead-plugin/v999", mems: ["m"] });
    assert.equal(r.valid, false);
    assert.ok(
      r.errors.some(e => /\$\.format/.test(e.path) && /expected const/.test(e.message)),
      `errors: ${JSON.stringify(r.errors)}`,
    );
  });

  it("rejects a retired `mode: refinement` build operation", () => {
    const schema = loadJson("binding.schema.json");
    const bad = {
      version: 1,
      destination_mem: "m",
      operations: { build: { mode: "refinement", trigger: "loop", batch_size: 20 } },
    };
    const r = validate(schema, bad);
    assert.equal(r.valid, false);
    assert.ok(
      r.errors.some(e => /expected one of/.test(e.message)),
      `errors: ${JSON.stringify(r.errors)}`,
    );
  });

  it("does NOT validate an old-layout (projection+ingest) fixture as a v1 binding", () => {
    // A legacy v0 projection carries no `version`/`operations`; a legacy
    // ingest carries `projection`/`mode`/`trigger` at the top level. Neither
    // is a v1 binding — both must be refused (drives migration, not silent
    // acceptance).
    const schema = loadJson("binding.schema.json");
    const legacyProjection = { destination_mem: "m", source_facets: ["f"] };
    const legacyIngest = { projection: "engine/src", mode: "discovery", trigger: "loop" };
    for (const [label, inst] of [["projection", legacyProjection], ["ingest", legacyIngest]]) {
      const r = validate(schema, inst);
      assert.equal(r.valid, false, `legacy ${label} must not validate as a v1 binding`);
    }
  });

  it("rejects a binding missing the required `version`", () => {
    const schema = loadJson("binding.schema.json");
    const r = validate(schema, {
      destination_mem: "m",
      operations: { build: { mode: "discovery", trigger: "loop", batch_size: 20 } },
    });
    assert.equal(r.valid, false);
    assert.ok(r.errors.some(e => /missing required property: version/.test(e.message)));
  });

  it("rejects an unknown top-level binding property", () => {
    const schema = loadJson("binding.schema.json");
    const r = validate(schema, {
      version: 1,
      destination_mem: "m",
      ingests: [],
      operations: { build: { mode: "discovery", trigger: "loop", batch_size: 20 } },
    });
    assert.equal(r.valid, false);
    assert.ok(r.errors.some(e => /unexpected property: ingests/.test(e.message)));
  });
});
