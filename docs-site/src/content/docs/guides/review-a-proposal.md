---
title: Review a proposal to a mem
description: "Read a fork's changes against the mem it was forked from, three ways, fill the disposition file, merge it under both identities, and read the record the merge keeps."
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

## 5. Merge the filled file

```sh
memstead proposal merge <fork> --dispositions brief.json --identity <you>
memstead proposal merge <fork> --dispositions brief.json --identity <you> --json
```

The merge re-renders the brief and checks the file against it before
anything lands: every entity the brief lists has a slot and no slot
names an entity it does not (`PROPOSAL_DISPOSITIONS_INCOMPLETE`), every
value is in the vocabulary, `reject` and `adopt_with_changes` carry a
reason and `adopt_with_changes` a body (`INVALID_INPUT`), no conflict
entity is adopted as is (`PROPOSAL_CONFLICT`), and the target tip, the
fork tip and the base the file recorded are still the branches' tips
(`PROPOSAL_STALE`, naming both shas: re-render and fill again). Then it
reads the proposer of every adopted entity off the fork commit that
last touched it; an entity whose last fork commit carries no identity
refuses `PROPOSAL_UNATTRIBUTED`, because a merge never invents an author.
Then every adopted body goes through the target's write gate, and a
deletion is judged against the target's referrers as the adopted bodies
leave them. Only after all of that does anything land, and a refusal at
any step lands nothing: the target, the fork, the sidecars and the
workspace state are byte-identical afterwards.

What lands, per disposition:

- `adopt`: the entity as the fork has it, created, updated or deleted in
  the target, with its anchor rows and self-links under the target's
  ids and the relations the fork dropped removed;
- `adopt_with_changes`: the fork's version first, then your final body
  in a second commit under your identity. The body's `sections` replace
  the landed ones, its `metadata` sets the fields it names (a field it
  omits keeps the landed value), its `relations`, when given, replace
  the landed ones; a `title` that differs refuses, since a title change
  is a rename;
- `reject`: nothing.

The merge commit sits on the target branch, parent-pinned to the tip
the file recorded, and carries the proposer's identity as the mutation's
identity with the merger's beside it: `Identity: <proposer>`,
`Merged-By: <you>`, `Proposal: <id>`, plus `Entities:` and `Created:`
(the ids the commit brought into the target). When adopted entities
were last touched by different proposers, there is one commit per
proposer identity, in slug order, each pinned to the one before, and
they land together or not at all: a failure after an earlier one moves
the branch back to the pinned tip. After the commits the merge records
a verification check under your identity for every entity it created
or updated (a landing that changed nothing, and a deletion, get none),
against the hash that landed (the second commit's for
`adopt_with_changes`), so `health --include checks` reads
them `confirmed_independent`; and it re-reads the whole target from its
backend and reports, under `validation`, the integrity and conformance
findings before and after the merge. `new_findings` is empty when the
store validates.

`--identity` is required: the merge records who merged beside who
proposed. The verb is CLI-only, a human's act on the owner's branch;
there is no MCP tool for it.

## 6. The record

The merge writes `.memstead/proposals.json` on the target branch, in the
merge commit, appending one entry per merged proposal: the proposal's id
(the fork's name and base sha), the proposer, the ancestor, the base, the
target tip at merge, who merged and when, and per entity the
disposition, the reason and the hash of the proposed version. Never a
body: a rejected proposal leaves its hash and its reason, nothing that
could be served.

```sh
memstead proposal list <target>            # one block per proposal
memstead proposal list <target> --json     # the record as written
memstead entity <target>--<slug> --provenance
```

`proposal list` renders the record; `entity --provenance` names, on an
entity a merge touched, the proposal, the merger and the disposition it
landed under beside the proposer's identity; the brief of a later fork
marks a re-proposal against it. The record travels: `export` seals it
into the archive as a recognised meta member, `proposal list` reads it
off an installed archive, and an engine older than the member installs
such an archive and ignores it. A fork made from a target that carries a
record does not inherit it: the fork commit drops it, since the record
belongs to the target branch.
