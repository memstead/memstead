// The supported plugin-format versions and the mapping from a
// `.memstead.toml` `format` value to its schema directory — the documented
// loader gate (see schemas/README.md "Versioning") as executable code.
//
// An unknown `format` is rejected with the supported set named, never
// silently accepted or silently dropped to a default. Absent `format` maps to
// legacy v0 (the README's "treat as legacy v0" row). Node built-ins only.

import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

/** Supported `format` values, in version order. */
export const SUPPORTED_FORMATS = ["memstead-plugin/v0", "memstead-plugin/v1"];

/**
 * Resolve a `.memstead.toml` `format` value to its `memstead-plugin/<version>/`
 * schema directory. Absent (`null`/`undefined`) → legacy v0. Throws with the
 * supported set named for any other value.
 */
export function resolveSchemaDir(format) {
  const value = format == null ? "memstead-plugin/v0" : format;
  if (!SUPPORTED_FORMATS.includes(value)) {
    throw new Error(
      `unsupported plugin format ${JSON.stringify(value)} — supported: ${SUPPORTED_FORMATS.join(", ")}`,
    );
  }
  const version = value.slice("memstead-plugin/".length); // "v0" | "v1"
  const dir = join(HERE, "memstead-plugin", version);
  if (!existsSync(dir)) {
    throw new Error(`schema directory missing for ${value}: ${dir}`);
  }
  return dir;
}
