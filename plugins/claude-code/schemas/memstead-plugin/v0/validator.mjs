// Hand-rolled JSON Schema 2020-12 validator for the memstead-plugin/v0
// schemas. Node built-ins only — no npm dependencies (the plugin is
// self-contained by policy and ships without a node_modules tree).
//
// Covers the keywords actually used by the schemas under
// `plugins/claude-code/schemas/memstead-plugin/v0/`:
//   - type (including integer as number subtype)
//   - required, properties, additionalProperties
//   - items, minItems, uniqueItems
//   - enum, const
//   - pattern, minLength
//   - minimum
//   - oneOf
//   - $ref (to local $defs only — no remote resolution)
//
// This is a strict subset, not a fully conformant 2020-12 implementation.
// The trade-off is documented in the schemas/README.md: producers should
// run schema authoring through `validate-live-workspace.mjs` (which uses
// this same module + a metaschema shape check) before relying on the
// runtime validator.
//
// Public API:
//   validate(schema, instance) → { valid: boolean, errors: Array<{path, message}> }
//
// Errors carry a JSONPath-ish `path` string (`$`, `$.foo`, `$.foo[0]`)
// pointing at the offending field, and a one-line `message`.

export function validate(schema, instance) {
  const errors = [];
  validateNode(schema, schema, instance, "$", errors);
  return { valid: errors.length === 0, errors };
}

function typeOf(x) {
  if (x === null) return "null";
  if (Array.isArray(x)) return "array";
  if (Number.isInteger(x)) return "integer";
  return typeof x;
}

function deepEqual(a, b) {
  if (a === b) return true;
  if (typeOf(a) !== typeOf(b)) return false;
  if (Array.isArray(a)) {
    if (a.length !== b.length) return false;
    return a.every((v, i) => deepEqual(v, b[i]));
  }
  if (a && typeof a === "object") {
    const ak = Object.keys(a);
    const bk = Object.keys(b);
    if (ak.length !== bk.length) return false;
    return ak.every(k => deepEqual(a[k], b[k]));
  }
  return false;
}

function resolveRef(rootSchema, ref) {
  if (!ref.startsWith("#/")) {
    throw new Error(`only local $refs supported, got: ${ref}`);
  }
  const parts = ref.slice(2).split("/");
  let cur = rootSchema;
  for (const p of parts) {
    cur = cur[decodeURIComponent(p)];
    if (cur === undefined) {
      throw new Error(`unresolved $ref: ${ref}`);
    }
  }
  return cur;
}

function validateNode(rootSchema, schema, instance, path, errors) {
  if (schema === true) return;
  if (schema === false) {
    errors.push({ path, message: "schema is `false` — value not allowed" });
    return;
  }
  if (schema.$ref) {
    validateNode(rootSchema, resolveRef(rootSchema, schema.$ref), instance, path, errors);
    return;
  }

  if (schema.type !== undefined) {
    const types = Array.isArray(schema.type) ? schema.type : [schema.type];
    const t = typeOf(instance);
    // integer is a subtype of number per JSON Schema 2020-12
    const ok = types.some(want => want === t || (want === "number" && t === "integer"));
    if (!ok) {
      errors.push({ path, message: `expected type ${types.join("|")}, got ${t}` });
      return;
    }
  }

  if (schema.const !== undefined && !deepEqual(instance, schema.const)) {
    errors.push({
      path,
      message: `expected const ${JSON.stringify(schema.const)}, got ${JSON.stringify(instance)}`,
    });
  }

  if (schema.enum !== undefined) {
    if (!schema.enum.some(v => deepEqual(v, instance))) {
      errors.push({
        path,
        message: `expected one of ${JSON.stringify(schema.enum)}, got ${JSON.stringify(instance)}`,
      });
    }
  }

  const t = typeOf(instance);
  if (t === "string") {
    if (schema.minLength !== undefined && instance.length < schema.minLength) {
      errors.push({ path, message: `string shorter than minLength ${schema.minLength}` });
    }
    if (schema.pattern !== undefined && !new RegExp(schema.pattern).test(instance)) {
      errors.push({ path, message: `string does not match pattern ${schema.pattern}` });
    }
  }

  if (t === "number" || t === "integer") {
    if (schema.minimum !== undefined && instance < schema.minimum) {
      errors.push({ path, message: `value below minimum ${schema.minimum}` });
    }
  }

  if (t === "array") {
    if (schema.minItems !== undefined && instance.length < schema.minItems) {
      errors.push({ path, message: `array shorter than minItems ${schema.minItems}` });
    }
    if (schema.uniqueItems === true) {
      const seen = [];
      for (const v of instance) {
        if (seen.some(s => deepEqual(s, v))) {
          errors.push({ path, message: `duplicate item: ${JSON.stringify(v)}` });
          break;
        }
        seen.push(v);
      }
    }
    if (schema.items !== undefined) {
      instance.forEach((v, i) =>
        validateNode(rootSchema, schema.items, v, `${path}[${i}]`, errors)
      );
    }
  }

  if (t === "object") {
    if (schema.required !== undefined) {
      for (const k of schema.required) {
        if (!(k in instance)) {
          errors.push({ path, message: `missing required property: ${k}` });
        }
      }
    }
    const props = schema.properties || {};
    for (const [k, v] of Object.entries(instance)) {
      const p = `${path}.${k}`;
      if (k in props) {
        validateNode(rootSchema, props[k], v, p, errors);
      } else if (schema.additionalProperties === false) {
        errors.push({ path: p, message: `unexpected property: ${k}` });
      } else if (typeof schema.additionalProperties === "object") {
        validateNode(rootSchema, schema.additionalProperties, v, p, errors);
      }
      // additionalProperties: true (default) — allow.
    }
  }

  if (schema.oneOf !== undefined) {
    let matches = 0;
    const subErrors = [];
    for (const sub of schema.oneOf) {
      const e = [];
      validateNode(rootSchema, sub, instance, path, e);
      if (e.length === 0) matches++;
      else subErrors.push(e);
    }
    if (matches !== 1) {
      errors.push({
        path,
        message: `oneOf matched ${matches} schemas; expected exactly 1. Branch errors: ${JSON.stringify(subErrors)}`,
      });
    }
  }
}
