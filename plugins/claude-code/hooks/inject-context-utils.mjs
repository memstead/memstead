// Pure utility functions for inject-context hook — no side effects, testable.

/**
 * Render write-rules JSON export as compact text for write-time context.
 * Extracts constraints from the single write-rules entity node.
 */
export function renderWriteRules(json) {
  if (!json || !Array.isArray(json.nodes)) return '';
  for (const node of json.nodes) {
    if (!node.private && node.constraints) return node.constraints;
  }
  return '';
}
