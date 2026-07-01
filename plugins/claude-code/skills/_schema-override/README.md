# Schema Skill Overrides

Schema-specific skill overrides. When a schema needs different behavior for a skill, place a `SKILL.md` in the matching schema directory here. The adapter loads overrides before falling back to the base skill in `skills/`.

## How it works

Set the `MEMSTEAD_SCHEMA` environment variable to the active schema name (e.g. `default`). The adapter auto-resolves the override directory:

```
_schema-override/<MEMSTEAD_SCHEMA>/<skill>/SKILL.md   (override — used if exists)
skills/<skill>/SKILL.md                           (base — fallback)
```

Callers can also pass `opts.schemaSkillsDir` explicitly to bypass env var detection.

## Structure

```
_schema-override/
  default/           # Overrides for the built-in `default` schema
    audit/SKILL.md   # Example: spec-oriented audit checks
  recipe/            # Overrides for a custom `recipe` schema
    audit/SKILL.md   # Example: recipe-specific audit checks
```

## When to use

Only when a skill genuinely needs different behavior per schema. Most skills (maintain, graph, commit, rollback) are schema-agnostic and should stay as base skills in `skills/`.

Good candidates for overrides: `/audit` (different quality criteria), `/ingest` (different source handling).
