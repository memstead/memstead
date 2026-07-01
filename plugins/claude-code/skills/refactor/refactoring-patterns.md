# Refactoring Patterns

Common structural issues and how to resolve them.

## Split: Entity too large

**Signal:** More than 10 `###` headings in `specifies`, or specifies covers multiple distinct concepts.

**Resolution:**
1. Identify distinct concepts within the entity
2. Create new entities for each concept with `memstead_create`
3. Connect them to the original via `PART_OF` relationships with `memstead_relate`
4. Move relevant content from the parent to each child via `memstead_update`
5. Parent keeps only a high-level summary

**Example:** An entity covering "API Server" with headings for routing, middleware, error handling, and authentication → split into 4 PART_OF entities.

## Merge: Entity too small

**Signal:** `specifies` is < 2 sentences, entity has only one relationship, and a neighboring entity covers the same concept at a broader level.

**Resolution:**
1. Move content into the broader entity via `memstead_update`
2. Redirect relationships from the small entity to the broader one via `memstead_relate`
3. Delete the small entity via `memstead_delete`

## Orphan: Missing connections

**Signal:** Entity has zero relationships but its `specifies` mentions concepts that exist as other entities.

**Resolution:**
1. Read the orphan's `specifies` content
2. Search for related entities via `memstead_search`
3. Add appropriate relationships: `PART_OF`, `USES`, `DEPENDS_ON` via `memstead_relate`; for soft references, write a `[[wiki-link]]` in the body — the alias-synthesis pass auto-emits `REFERENCES`.

## Stub: Unresolved reference

**Signal:** A `[[wiki-link]]` target doesn't exist as an entity.

**Resolution options:**
- **Create the entity** if the concept deserves its own entity
- **Fix the link** if it's a typo or the target was renamed
- **Remove the link** if the reference is no longer relevant

## Relationship type mismatch

| Current | Should be | When |
|---------|-----------|------|
| USES | PART_OF | Target is a sub-component, not an external dependency |
| REFERENCES (synthesised from `[[link]]`) | DEPENDS_ON | Target is required for function, not just mentioned — author DEPENDS_ON explicitly; leave the body wiki-link as the REFERENCES marker |
| PART_OF | USES | Target is shared across multiple parents |
| DEPENDS_ON | USES | Dependency is optional/soft, not a hard requirement |
