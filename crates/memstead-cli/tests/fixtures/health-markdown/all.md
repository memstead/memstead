# Graph health

**Verdict coverage:** examined=dangling_links,missing_required_outgoing,constraints,signals,integrity,config,mounts,anchors; advisory=orphans,stubs,most_connected,missing_fields,stale,tags,labelling,conformance,friction,open_questions,vital_signs,stale_derivations,checks,ledger; not_examined=projection

- Entities: 6
- Orphans: 2
- Stubs: 1
- Stale: 0
- Missing fields: 6
- Communities: 3

## Orphans
- notes--lonely — Lonely
- side--aside — Aside

## Stubs
- notes--ghost

## Most connected
(ranked by typed dependency degree; total keeps mention edges)
- notes--alpha — Alpha (typed 2, total 2, in 1, out 1)
- notes--beta — Beta (typed 2, total 2, in 0, out 2)
- notes--hub — Hub (typed 2, total 2, in 2, out 0)
- notes--gamma — Gamma (typed 1, total 1, in 0, out 1)
- notes--lonely — Lonely (typed 0, total 0, in 0, out 0)
- side--aside — Aside (typed 0, total 0, in 0, out 0)

## Missing fields
- notes--alpha — Alpha (issues: details (MISSING))
- notes--beta — Beta (issues: details (MISSING))
- notes--gamma — Gamma (issues: details (MISSING))
- notes--hub — Hub (issues: details (MISSING))
- notes--lonely — Lonely (issues: details (MISSING))
- side--aside — Aside (issues: details (MISSING))

## Stale entities

## Missing required outgoing

## Conformance findings (0)
- none

## Consistency findings (1)
- [UNRESOLVED_STUB] notes--ghost (axis consistency)

## Constraint violations (0)
- none

## Dangling links

## Tags

## Untagged
- Total: 6
  - spec: 4
  - concept: 2

## Ledger vs files (0 folder mem(s))
- no folder mems: the check does not apply to git-branch storage, whose change set is a real two-tree diff

## Anchors (2 mems)
- `notes`: resolves 0, drifted 0, recheck 0, unresolvable (artifact gone) 0, unobserved (not measured) 0, dangling (entity gone) 0 — over 0 counted row(s): 0 adjudicated, 0 not (recheck 0, unobserved 0)
- `side`: resolves 0, drifted 0, recheck 0, unresolvable (artifact gone) 0, unobserved (not measured) 0, dangling (entity gone) 0 — over 0 counted row(s): 0 adjudicated, 0 not (recheck 0, unobserved 0)

## Vital signs (2 mems)
- `notes`: last-resort type `concept` over 2 community(ies); no bound source; 0 contested unowned file(s); 2 zero-outgoing entity(ies) in 2 community(ies); 0 empty declared section(s)
- `side`: last-resort type `concept` over 1 community(ies); no bound source; 0 contested unowned file(s); 1 zero-outgoing entity(ies) in 1 community(ies); 0 empty declared section(s)

## Open questions (item cap 20 per kind)
- `notes`: 1 open
  - stubs: 1
- `side`: 0 open

## Checks (2 mems)
- `notes`: never_checked 6, checked_ok 0, check_failed 0, check_stale 0; conformance: never_checked 6, checked_ok 0, check_failed 0, check_stale 0; independence: self_checked 0, confirmed_independent 0, unconfirmable 0
- `side`: never_checked 1, checked_ok 0, check_failed 0, check_stale 0; conformance: never_checked 1, checked_ok 0, check_failed 0, check_stale 0; independence: self_checked 0, confirmed_independent 0, unconfirmable 0

## Signals (notice 0, warn 0)

## Labelling (0 mems)

## Stale derivations (0 findings)

## Friction (0 refusals recorded, 0 in the last 24h)

