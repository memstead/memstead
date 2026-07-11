// Pure, side-effect-free helpers for the check-realization hook — testable
// without process.exit, stdin, or a subprocess.
//
// The hook is now anchor-based (E3a): it asks the engine which entities
// anchored the file an agent just edited (via `memstead anchors --artifact`)
// and surfaces a fail-open notice. These helpers shape that engine reply into
// the notice. The old regex reader (`schema.drift.realizationPatterns.*`) is
// retired — the plugin never loads schema-derived scan patterns anymore.

/**
 * Entity ids that anchored the edited artifact with a *span*- or *file*-grain
 * anchor — the grains that name a single file the edit could invalidate.
 * `tree` / `url` / `entity` grains are deliberately excluded: a tree anchor
 * covers many files and would be noisy on any edit beneath it, and url/entity
 * anchors don't reference a filesystem path at all.
 *
 * Deduplicated, in first-seen order. Tolerant of any malformed reply — a
 * missing/!array `anchors` yields `[]` (the hook then stays silent).
 *
 * @param {unknown} result - Parsed `memstead anchors --artifact --json` reply,
 *   shaped `{ count, anchors: [{ grain, entity_id, ... }], composition }`.
 * @returns {string[]}
 */
export function pickReferencedEntityIds(result) {
  const anchors = result && Array.isArray(result.anchors) ? result.anchors : [];
  const seen = new Set();
  const ids = [];
  for (const a of anchors) {
    if (!a || (a.grain !== 'span' && a.grain !== 'file')) continue;
    const id = a.entity_id;
    if (typeof id === 'string' && id.length > 0 && !seen.has(id)) {
      seen.add(id);
      ids.push(id);
    }
  }
  return ids;
}

/**
 * The single-line notice the hook writes when an edited file is anchored.
 * References only living surfaces — `memstead_entity` — never a now-retired
 * skill command.
 *
 * @param {string} relPath - Edited path, workspace-relative (the form anchors store).
 * @param {string[]} ids - Non-empty referencing entity ids.
 * @returns {string}
 */
export function formatRealizationNotice(relPath, ids) {
  return (
    `REALIZATION EDIT: \`${relPath}\` is anchored by: ${ids.join(', ')}. ` +
    `Review them with memstead_entity to check the entities still describe the file.\n`
  );
}
