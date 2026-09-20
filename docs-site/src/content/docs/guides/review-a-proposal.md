---
title: Review a proposal to a mem
description: "Read a fork's changes against the mem it was forked from, three ways, and fill the disposition file the merge consumes."
sidebar:
  order: 8
---

A proposal against a mem is a fork of it (see [fork a mem at a recorded
ancestor](/guides/back-up-a-mem-repo/#5-fork-a-mem-at-a-recorded-ancestor)):
the proposer works on their own branch for as long as they like, and the
owner reviews the result against a common ancestor instead of guessing
which of two unrelated mems changed. The review is one read, the brief.

## 1. Render the brief

```sh
memstead proposal brief <fork>                       # markdown for a human
memstead proposal brief <fork> --json                # the same as JSON
memstead proposal brief <fork> --out brief.json      # the fillable disposition file
```

The fork's changes are read against its base: the fork commit
(`forkedFrom.base`), or the ancestor (`forkedFrom.sha`) for a fork made
before fork commits existed. The target side is the source mem's branch
tip at render time. The brief prints all four shas (ancestor, base, fork
tip, target tip) so the merge can refuse when the target moved after the
review. A mem with no `forkedFrom` refuses `INVALID_INPUT`; a fork whose
source mem is not mounted refuses `UNKNOWN_MEM`. The brief writes
nothing: not the fork, not the target, not the workspace state.

Entities match by slug across the two mems: the fork's `<fork>--x` is
the target's `<target>--x`. Bodies compare with mem-qualified self-links
normalised, so a link the fork commit retargeted is never reported as a
change. A rename is a recorded rename (the engine's rename note on the
fork's branch) or a byte-identical move; the brief never guesses one from
content similarity, so a deletion beside an unrelated addition reads as
both.

## 2. Read one block per entity

Every entity the fork added, modified, deleted or renamed gets a block; a
target entity the fork did not touch is absent. Each block carries:

- the sections and metadata fields that differ against the base (the
  engine-stamped dates never count);
- the target's referrers: the entities, in the target or in other mems,
  holding an edge to it, so you see what a deletion or a change ripples
  into;
- the anchor rows the fork's entity rests on, read from the fork's
  sidecar under the fork's ids: artifact, grain, class, the quoted span,
  and the last recorded state. No observation is fetched; a url row
  shows the state its last observation recorded, or `unobserved`;
- the mechanics precheck: would this body pass the target's write gate
  as it stands. The brief rehearses the create or update the merge would
  run, through the gate's own dry run (required sections and fields, enum
  values, constraints, relationship shapes, cross-mem grants), and reports
  `clean`, `failed` with the gate's typed code (an `INVALID_ENUM_VALUE`
  reads exactly as it would on a write), or `not_run` where no write
  applies (a deletion is judged against the referrers; a conflict is the
  owner's to resolve first). It never judges content;
- a conflict mark where the target modified or deleted the same entity
  since the ancestor, or created an entity under the slug the fork
  added. A conflict shows the three versions (base, fork, target) and the
  sections that differ on each side against the base;
- a re-proposal mark where the target's proposal record already holds a
  rejection of the same content hash (the same body under any slug) or
  the same id, with the recorded reason. The record is written by the
  merge on the target's branch; a target that was never merged into has
  none, and none means no rejections.

## 3. Fill the disposition file

The JSON form carries everything the markdown shows plus `dispositions`,
a map keyed by slug:

```json
"dispositions": {
  "beta": { "disposition": "", "reason": "", "accepts": ["adopt", "adopt_with_changes", "reject"] },
  "epsilon": { "disposition": "", "reason": "", "accepts": ["adopt_with_changes", "reject"] }
}
```

The vocabulary is closed: `adopt` takes the entity as the fork has it,
`adopt_with_changes` lands the fork's version and then your final body,
`reject` lands nothing. `reject` and `adopt_with_changes` need a
`reason`. `adopt_with_changes` carries your final body as `body`, in the
shape a create takes (`title`, `sections`, `metadata`); the merge
validates it through the write gate like any other write. A conflict
entity accepts no `adopt`: the brief cannot merge two meanings, so you
supply the merged body or reject. Fill every slot; the merge refuses a
file with a slot left empty, a value outside the vocabulary, or a
target tip that moved since the render.

## 4. Over MCP

The owner's agent renders the same brief with `memstead_proposal_brief`
(one parameter, `fork`): the markdown rides the text channel, the JSON
with the disposition skeleton rides `structured_content`, so the agent
can pre-fill the dispositions for the owner to confirm. The tool is
read-only and refuses with the same codes the CLI uses. The merge itself
is a human's act on the owner's branch and stays on the CLI.
