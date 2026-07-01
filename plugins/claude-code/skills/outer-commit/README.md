# Outer-Commit — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- Commit pending entity changes to the outer repo — manual fallback for
  when the Stop hook is disabled or couldn't complete (e.g. wrong
  branch, engine crash).
- Delegates to the shared `produceOuterCommit` pipeline in
  `hooks/auto-commit-utils.mjs` — no duplicated git-commit recipe in the
  skill. Subject, body, and trailer shape come from the hook's code.

## Safety

- only commit the writable-mem worktrees — never code, never config
- never amend, never force push, never rewrite history

## Commit message format

Same as the Stop hook. Subject `memstead: session changes (N entities, M
mems)`; body carries `Agent notes:` and `External edits captured:`
subsections; trailer block includes `Mems:` and one `Memstead-cursor:`
entry per writable mem. The `Session:` trailer is present on hook
commits and omitted on skill commits — that is the only difference
between the two paths.
