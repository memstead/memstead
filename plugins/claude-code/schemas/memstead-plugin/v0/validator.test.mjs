// Tests for the hand-rolled validator.mjs.
//
// Coverage: every keyword the validator implements, plus a round-trip
// test against each of the four real plugin schemas using the example
// fixtures shipped under `examples/`.
//
// Node built-in test runner — no npm. Run via `node --test` from the
// project root.

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

// ── Atomic keyword tests ─────────────────────────────────────────────

describe("validator — type", () => {
  it("accepts matching primitive types", () => {
    assert.equal(validate({ type: "string" }, "hi").valid, true);
    assert.equal(validate({ type: "number" }, 1.5).valid, true);
    assert.equal(validate({ type: "integer" }, 3).valid, true);
    assert.equal(validate({ type: "boolean" }, true).valid, true);
    assert.equal(validate({ type: "array" }, [1, 2]).valid, true);
    assert.equal(validate({ type: "object" }, { a: 1 }).valid, true);
    assert.equal(validate({ type: "null" }, null).valid, true);
  });

  it("treats integer as a subtype of number", () => {
    assert.equal(validate({ type: "number" }, 7).valid, true);
  });

  it("rejects mismatched types and reports the path", () => {
    const r = validate({ type: "string" }, 42);
    assert.equal(r.valid, false);
    assert.equal(r.errors[0].path, "$");
    assert.match(r.errors[0].message, /expected type string, got integer/);
  });
});

describe("validator — required + properties", () => {
  it("flags missing required keys", () => {
    const schema = {
      type: "object",
      required: ["a", "b"],
      properties: { a: { type: "string" }, b: { type: "string" } },
    };
    const r = validate(schema, { a: "x" });
    assert.equal(r.valid, false);
    assert.ok(r.errors.some(e => /missing required property: b/.test(e.message)));
  });

  it("recurses into property schemas with a JSONPath-shaped path", () => {
    const schema = {
      type: "object",
      properties: { inner: { type: "string" } },
    };
    const r = validate(schema, { inner: 5 });
    assert.equal(r.valid, false);
    assert.equal(r.errors[0].path, "$.inner");
  });
});

describe("validator — additionalProperties", () => {
  it("rejects extras when set to false", () => {
    const schema = {
      type: "object",
      properties: { known: { type: "string" } },
      additionalProperties: false,
    };
    const r = validate(schema, { known: "x", extra: 1 });
    assert.equal(r.valid, false);
    assert.ok(r.errors.some(e => /unexpected property: extra/.test(e.message)));
  });

  it("allows extras when set to true (default behaviour)", () => {
    const schema = {
      type: "object",
      properties: { known: { type: "string" } },
      additionalProperties: true,
    };
    assert.equal(validate(schema, { known: "x", extra: 1 }).valid, true);
  });

  it("validates extras against an inline subschema when given", () => {
    const schema = {
      type: "object",
      additionalProperties: { type: "string" },
    };
    assert.equal(validate(schema, { a: "x" }).valid, true);
    const r = validate(schema, { a: 1 });
    assert.equal(r.valid, false);
    assert.equal(r.errors[0].path, "$.a");
  });
});

describe("validator — items, minItems, uniqueItems", () => {
  it("checks each item against the items schema", () => {
    const schema = { type: "array", items: { type: "integer" } };
    const r = validate(schema, [1, "two", 3]);
    assert.equal(r.valid, false);
    assert.equal(r.errors[0].path, "$[1]");
  });

  it("flags arrays shorter than minItems", () => {
    const r = validate({ type: "array", minItems: 2 }, [1]);
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /minItems 2/);
  });

  it("flags duplicate items when uniqueItems is true", () => {
    const r = validate({ type: "array", uniqueItems: true }, [1, 2, 1]);
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /duplicate item/);
  });
});

describe("validator — enum + const", () => {
  it("enforces enum membership", () => {
    const schema = { type: "string", enum: ["a", "b"] };
    assert.equal(validate(schema, "a").valid, true);
    const r = validate(schema, "c");
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /expected one of/);
  });

  it("enforces const", () => {
    const schema = { type: "string", const: "memstead-plugin/v0" };
    assert.equal(validate(schema, "memstead-plugin/v0").valid, true);
    const r = validate(schema, "memstead-plugin/v1");
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /expected const/);
  });
});

describe("validator — string keywords", () => {
  it("checks minLength", () => {
    const r = validate({ type: "string", minLength: 3 }, "ab");
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /minLength 3/);
  });

  it("checks pattern", () => {
    const schema = { type: "string", pattern: "^[a-z]+/[a-z]+$" };
    assert.equal(validate(schema, "engine/scope").valid, true);
    const r = validate(schema, "engine.scope");
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /does not match pattern/);
  });
});

describe("validator — minimum", () => {
  it("flags numbers below minimum", () => {
    const r = validate({ type: "integer", minimum: 1 }, 0);
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /below minimum 1/);
  });
});

describe("validator — oneOf", () => {
  it("requires exactly one branch to match", () => {
    const schema = {
      type: "object",
      oneOf: [
        { required: ["a"] },
        { required: ["b"] },
      ],
    };
    assert.equal(validate(schema, { a: 1 }).valid, true);
    assert.equal(validate(schema, { b: 1 }).valid, true);
    const r = validate(schema, { a: 1, b: 1 });
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /matched 2 schemas/);
    const r2 = validate(schema, { c: 1 });
    assert.equal(r2.valid, false);
    assert.match(r2.errors[0].message, /matched 0 schemas/);
  });
});

describe("validator — $ref to local $defs", () => {
  it("resolves $ref entries", () => {
    const schema = {
      type: "object",
      properties: { x: { $ref: "#/$defs/named" } },
      $defs: { named: { type: "string", minLength: 1 } },
    };
    assert.equal(validate(schema, { x: "ok" }).valid, true);
    const r = validate(schema, { x: "" });
    assert.equal(r.valid, false);
    assert.match(r.errors[0].message, /minLength 1/);
  });
});

// ── Round-trip tests against the real plugin schemas ─────────────────

describe("validator — real plugin schemas", () => {
  const cases = [
    ["memstead-toml.schema.json", "examples/memstead-toml.minimal.json"],
    ["memstead-toml.schema.json", "examples/memstead-toml.full.json"],
    ["projection.schema.json", "examples/projection.four-primitive.json"],
    ["ingest.schema.json", "examples/ingest.minimal.json"],
    ["ingest.schema.json", "examples/ingest.full.json"],
    ["medium.schema.json", "examples/medium.minimal.json"],
    ["facet.schema.json", "examples/facet.minimal.json"],
    ["facet.schema.json", "examples/facet.full.json"],
  ];

  for (const [schemaFile, exampleFile] of cases) {
    it(`${exampleFile} validates against ${schemaFile}`, () => {
      const schema = loadJson(schemaFile);
      const inst = loadJson(exampleFile);
      const r = validate(schema, inst);
      assert.equal(
        r.valid,
        true,
        `expected valid; errors: ${JSON.stringify(r.errors, null, 2)}`
      );
    });
  }

  it("flags a malformed facet scope entry with a path-pointed error", () => {
    const schema = loadJson("facet.schema.json");
    const bad = {
      name: "src",
      medium: "src",
      scope: [{ path: "x/**/*.rs" /* missing required `mode` */ }],
    };
    const r = validate(schema, bad);
    assert.equal(r.valid, false);
    // The error must point at the offending entry (the missing `mode` key
    // surfaces inside the tree-entry subschema referenced via $ref).
    assert.ok(
      r.errors.some(e => /missing required property: mode/.test(e.message)),
      `errors: ${JSON.stringify(r.errors)}`
    );
  });

  it("rejects an memstead-toml with the wrong format constant", () => {
    const schema = loadJson("memstead-toml.schema.json");
    const bad = {
      format: "memstead-plugin/v999",
      mems: ["some/mem"],
    };
    const r = validate(schema, bad);
    assert.equal(r.valid, false);
    assert.ok(
      r.errors.some(e => /\$\.format/.test(e.path) && /expected const/.test(e.message)),
      `errors: ${JSON.stringify(r.errors)}`
    );
  });
});
