// Tests for the plugin-format version gate (versions.mjs) — the executable
// form of the README's Versioning table.

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";

import { SUPPORTED_FORMATS, resolveSchemaDir } from "./versions.mjs";

test("every supported format resolves to an existing schema directory", () => {
  for (const f of SUPPORTED_FORMATS) {
    assert.ok(existsSync(resolveSchemaDir(f)), `${f} → dir must exist`);
  }
});

test("absent format maps to legacy v0", () => {
  assert.match(resolveSchemaDir(null), /memstead-plugin\/v0$/);
  assert.match(resolveSchemaDir(undefined), /memstead-plugin\/v0$/);
});

test("an unsupported format is rejected with the supported list", () => {
  assert.throws(
    () => resolveSchemaDir("memstead-plugin/v999"),
    (e) => {
      assert.match(e.message, /unsupported plugin format/);
      // The refusal names the whole supported set, not just one version.
      assert.match(e.message, /memstead-plugin\/v0/);
      assert.match(e.message, /memstead-plugin\/v1/);
      return true;
    },
  );
});
