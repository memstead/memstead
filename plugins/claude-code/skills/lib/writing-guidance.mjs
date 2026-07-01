/**
 * writing-guidance.mjs — plugin-side resolver for schema-default +
 * per-vault writing-guidance prose.
 *
 * Two responsibilities:
 *
 * 1. `resolveWritingGuidance(schemaPayload, vaultConfig)` — merge the
 *    schema's `default_writing_guidance.{avoid, goal}` with the vault's
 *    `writeGuidance.{avoid_additions, goal_additions}` to produce one
 *    merged `{ avoid, goal, ...passthroughKeys }` object that downstream
 *    consumers (interview/SKILL.md framing, ingest's `renderGuidance`)
 *    use the same way they did before migration. Other writeGuidance
 *    keys (granularity, stack, language, phase_context, …) pass through
 *    verbatim.
 *
 * 2. `extractDefaultWritingGuidance(schemaYamlText)` — minimal regex-
 *    based YAML extractor for just the `default_writing_guidance:`
 *    top-level block + its `avoid: |` / `goal: |` block scalars. Used by
 *    consumers that work off raw schema YAML on disk (inject.mjs reads
 *    `<workspace>/<schemas_dir>/<schema_name>/schema.yaml` directly —
 *    no MCP round-trip in that path). The MCP-driven consumers
 *    (chat-agent prompts) get the schema payload from
 *    `memstead_overview` / `memstead_vault_create` and pass that object
 *    straight to `resolveWritingGuidance`.
 *
 * Legacy fallback (D8 deprecation path):
 *   If the vault still carries a literal `writeGuidance.avoid` (or
 *   `goal`) — pre-migration / external workspace — the resolver
 *   returns that verbatim and emits a one-time `console.warn` per
 *   process. Schema defaults are ignored on the legacy path so the
 *   operator's pre-migration prose still wins; once migrated, the
 *   deprecation warning stops firing.
 *
 * Zero runtime dependencies. Tested by writing-guidance.test.js next
 * to this file.
 */

const _legacyWarnSeen = new Set();

/**
 * Merge schema-side defaults with per-vault additions / legacy fallback.
 *
 * @param {object|null|undefined} schemaPayload — `memstead_overview`-style
 *   schema document (or the `extractDefaultWritingGuidance` result spread
 *   under `default_writing_guidance`). Pass `null` when the schema isn't
 *   resolvable; the legacy fallback path then shoulders the load.
 * @param {object|null|undefined} vaultConfig — full
 *   `<vault>/.memstead/config.json` object. The resolver consults
 *   `vaultConfig.writeGuidance` keys.
 * @returns {object} merged `{ avoid, goal, ...passthroughKeys }`. Keys
 *   absent everywhere are simply not on the returned object so callers
 *   that iterate `Object.entries` don't render empty bullets.
 */
export function resolveWritingGuidance(schemaPayload, vaultConfig) {
  const wg = (vaultConfig && typeof vaultConfig.writeGuidance === 'object')
    ? vaultConfig.writeGuidance
    : {};
  const def = (schemaPayload && typeof schemaPayload.default_writing_guidance === 'object')
    ? schemaPayload.default_writing_guidance
    : {};

  const out = {};

  // Pass-through keys (granularity, stack, language, phase_context, …).
  // Filter out the four reserved keys; everything else flows verbatim.
  for (const [k, v] of Object.entries(wg)) {
    if (k === 'avoid' || k === 'goal' || k === 'avoid_additions' || k === 'goal_additions') continue;
    out[k] = v;
  }

  out.avoid = mergeBlock('avoid', wg, def, vaultConfig);
  out.goal = mergeBlock('goal', wg, def, vaultConfig);

  // Drop empty strings so consumers don't render a header for nothing.
  if (!out.avoid) delete out.avoid;
  if (!out.goal) delete out.goal;

  return out;
}

function mergeBlock(field, wg, def, vaultConfig) {
  const additionsKey = `${field}_additions`;
  const additions = typeof wg[additionsKey] === 'string' ? wg[additionsKey] : '';
  const legacy = typeof wg[field] === 'string' ? wg[field] : '';
  const schemaDefault = typeof def[field] === 'string' ? def[field] : '';

  // Legacy path: vault still carries a pre-migration literal `avoid` /
  // `goal`. Preserve it verbatim, ignore the schema default, log once
  // per process per vault per field. The migration sweep removes these
  // legacy keys; this branch only fires for unmigrated workspaces or
  // external (post-publish) consumers that haven't caught up.
  if (legacy) {
    const vaultName = vaultConfig?.name ?? '<unknown>';
    const key = `${vaultName}::${field}`;
    if (!_legacyWarnSeen.has(key)) {
      _legacyWarnSeen.add(key);
      // eslint-disable-next-line no-console
      console.warn(
        `[memstead writing-guidance] vault '${vaultName}' carries legacy ` +
        `writeGuidance.${field}; schema's default_writing_guidance.${field} ` +
        `is ignored for this vault. Migrate by moving the prose into the ` +
        `schema YAML and removing the per-vault key (or rename to ` +
        `${field}_additions for vault-specific extra prose).`,
      );
    }
    // If the vault also carries `avoid_additions` alongside the legacy
    // `avoid`, prefer the legacy block per the plan's documented
    // precedence — a half-finished migration must not silently lose
    // either side.
    return legacy;
  }

  if (!schemaDefault && !additions) return '';
  if (!additions) return schemaDefault;
  if (!schemaDefault) return additions;
  return `${schemaDefault.trimEnd()}\n\n${additions}`;
}

/**
 * Minimal YAML extractor for a schema manifest's
 * `default_writing_guidance:` top-level block. Handles the only shape
 * the engine emits via `emit_json_schemas`: an optional outer block
 * with optional `avoid:` and `goal:` block scalars (`|` / `>`).
 *
 * Returns `{ avoid?, goal? }` — keys absent when the schema doesn't
 * declare them, both keys absent when the whole block is missing.
 *
 * Not a general YAML parser: any other shape (folded scalars with
 * indicators, anchors, references) returns the empty object. The
 * primary consumer is inject.mjs which reads schema YAML off disk; if
 * a future schema author uses an exotic YAML feature the extractor
 * doesn't recognise, the resolver falls through to vault-side
 * additions and the operator should report it.
 */
export function extractDefaultWritingGuidance(yamlText) {
  if (typeof yamlText !== 'string') return {};

  // Find the `default_writing_guidance:` line at column 0 (top-level).
  const lines = yamlText.split('\n');
  let i = 0;
  while (i < lines.length) {
    if (/^default_writing_guidance:\s*$/.test(lines[i])) break;
    i += 1;
  }
  if (i === lines.length) return {};

  // Scan inner block — children are indented by ≥ 2 spaces. Stop on the
  // first line that's a top-level key (column 0, non-empty, not a
  // comment) or end of file.
  i += 1;
  const out = {};
  while (i < lines.length) {
    const line = lines[i];
    if (line === '' || /^\s*#/.test(line)) {
      i += 1;
      continue;
    }
    // Top-level key reached → block ended.
    if (/^[A-Za-z_]/.test(line)) break;

    const m = line.match(/^\s+(avoid|goal):\s*([|>][-+]?)?\s*(.*)$/);
    if (!m) {
      i += 1;
      continue;
    }
    const field = m[1];
    const indicator = m[2] || '';
    const inline = m[3] || '';

    if (indicator) {
      // Block scalar. Body lines are everything more-indented than the
      // key line, until we hit a less-indented line.
      const keyIndent = line.match(/^(\s+)/)[1].length;
      const bodyLines = [];
      i += 1;
      let bodyIndent = null;
      while (i < lines.length) {
        const next = lines[i];
        if (next === '') {
          bodyLines.push('');
          i += 1;
          continue;
        }
        const nextIndent = next.match(/^(\s*)/)[1].length;
        if (nextIndent <= keyIndent) break;
        if (bodyIndent === null) bodyIndent = nextIndent;
        bodyLines.push(next.slice(bodyIndent));
        i += 1;
      }
      // Trim trailing empty lines (matches `|`'s default chomping).
      while (bodyLines.length > 0 && bodyLines[bodyLines.length - 1] === '') {
        bodyLines.pop();
      }
      out[field] = bodyLines.join('\n');
      continue;
    }
    // Inline scalar (rare in the wild — schema authors use `|`).
    out[field] = inline.replace(/^["']|["']$/g, '');
    i += 1;
  }
  return out;
}

/**
 * Render a resolved-writing-guidance object to Markdown for prompt
 * embedding. Adds **consistent framing** around the `avoid` block —
 * schema YAML carries pure bullets / pure goal prose, this renderer
 * supplies the meta-framing once for every schema.
 *
 * Why centralized: schema authors should not write "trust the schema"-
 * style preambles inside their `default_writing_guidance.avoid` —
 * that's commentary about the surrounding delivery context, not
 * authoring guidance. Putting it here means every schema gets
 * identical framing automatically; if the framing wording needs to
 * evolve (e.g. when Session 5 adds a `--strict` enforcement layer),
 * one edit covers every shipping schema.
 *
 * Output shape: lines, no trailing newline. Caller wraps in their
 * preferred section header (`## Write Guidance`, `## Authoring`, …).
 */
export function renderResolvedGuidance(merged) {
  if (!merged || typeof merged !== 'object') return '';
  const sections = [];

  if (typeof merged.avoid === 'string' && merged.avoid.trim()) {
    sections.push(
      '**Authoring norms** (the schema enforces type / edge / `required_outgoing` rules structurally; these need agent attention because they live outside that mechanism):',
      '',
      merged.avoid.trim(),
    );
  }

  if (typeof merged.goal === 'string' && merged.goal.trim()) {
    if (sections.length) sections.push('');
    sections.push('**Goal:**', '', merged.goal.trim());
  }

  // Pass-through keys (granularity, stack, language, phase_context, …).
  // Filter to genuine pass-through — `avoid` / `goal` already rendered;
  // the `_additions` keys were consumed by the resolver upstream so they
  // shouldn't appear here, but defensive-skip them anyway.
  for (const [k, v] of Object.entries(merged)) {
    if (k === 'avoid' || k === 'goal') continue;
    if (k === 'avoid_additions' || k === 'goal_additions') continue;
    if (k.startsWith('_')) continue;
    if (sections.length) sections.push('');
    sections.push(`**${k}:** ${[].concat(v).join(' ')}`);
  }

  return sections.join('\n');
}

// For tests: reset the once-per-process legacy warning cache.
export function _resetLegacyWarnCacheForTests() {
  _legacyWarnSeen.clear();
}
