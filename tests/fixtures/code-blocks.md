---
type: spec
created_date: 2026-03-01
last_modified: 2026-04-12
level: M0
---
# Code Block Entity

## Identity

Entity with code blocks that contain patterns that should NOT be detected.

## Purpose

Testing code block masking in the parser.

## Specifies

Here is a code block with wiki-link-like content:

```rust
// This should NOT be parsed as a wiki-link
let link = "[[fake-link]]";
// This should NOT be parsed as a section
// ## Not A Section
```

And another one:

```markdown
## Also Not A Section
- **FAKE**: [[not-a-relationship]]
```

Real content after code blocks with [[real-link]].
