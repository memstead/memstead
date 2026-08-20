---
type: memo
created_date: 2026-08-20T06:24:16Z
last_modified: 2026-08-20T06:24:16Z
status: active
tags: bindings, anchors, projections, gotcha
---

# A binding record's folder is its identity, and destination_mem can disagree

## Claim
The **location** of a binding record — `.memstead/projections/<mem>/<stem>.json` — determines the binding id (`<mem>/<stem>`) and which mem's anchors resolve against it. The record's `destination_mem` field is separate data and can name a different mem entirely. Neither one is derived from the other, and nothing reconciles them.

## Context
Found while writing a remedy for a brief whose destination mem did not exist. The obvious fix — "set `destination_mem` to the mem you have" — produces a workspace that looks repaired and is not: the Destination block resolves, and every anchored write the same brief mandates still refuses `INVALID_ANCHOR`, because anchor resolution follows the folder the record sits in.

Measured both halves separately in a filesystem-mem workspace:

- **Edit the field only** — brief shows `**fs** — schema: default@1.3.0`; `memstead create --anchor '{"artifact":"main.rs",…,"source":"src-tree"}'` refuses `INVALID_ANCHOR: anchor artifact "main.rs" resolves under no candidate path`.
- **Move the record only** — the identical create succeeds (`# Created fs--main-fn`), while the brief still describes the absent mem, because `destination_mem` was left alone.

So a record misfiled under the wrong mem is broken in a way that reads as two unrelated symptoms depending on which half you touch.

## Substance

The practical consequence for anything that repairs a binding: **re-declare rather than edit**. `rm` the misfiled record and re-run `memstead projection init --mem <right-mem> --source <pointer> --medium-type <type> --name <stem>` — `--name` is not optional when the pointer is `.` (the `quickstart --repo .` layout), which derives no stem and refuses `PROJECTION_INVALID_NAME`.

This is why the absent-destination note in the build brief names a re-declaration for the filesystem-mem shape rather than a field edit, and says so explicitly: "Editing `destination_mem` alone is not enough — the record's folder decides which mem's anchors resolve."

## Alternatives

Making the engine reconcile the two — deriving `destination_mem` from the folder, or refusing a record whose field disagrees — was not attempted here. It would be a real design change with migration consequences for any workspace where the divergence is currently harmless, and nothing in the first-session path required it. Recorded as an observation, not a proposal.

## Outcome

The brief's remedy and the public walkthrough both now describe re-declaration. A regression test follows the printed remedy end to end — extracts its commands, runs them, then performs the anchored write — rather than matching its wording, which is what caught the missing `--name` before it shipped.
