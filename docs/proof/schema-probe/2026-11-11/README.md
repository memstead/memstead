# Schema probe, second run: the due reading from the check ledger (2026-11-11)

The 2026-11-10 run (`../2026-11-10/`, described in `../README.md`) found one
reading missing in all three fields: a due date that follows from a recorded
check. Four rules were workarounds because the date sat in a field an agent had
to type by hand: stock rules 1 and 9, lab rule 2 and inspection rule 2.

This run takes copies of the three packages, rewrites them to the current
schema language with `memstead schema migrate --write`, and changes one line
in each of the four types: the `due` block reads `check_kind: <kind>` instead
of `date_field`. Nothing else changes; the field that used to hold the date is
still declared and is no longer read.

| Field | Rule | Type | Was | Now | Verdict |
|---|---|---|---|---|---|
| Managed stock | 1: the next due date of a yearly duty follows from the last recorded completion | `duty` | `date_field: last_completed` | `check_kind: x-completion` | carried |
| Managed stock | 9: a standing fact is confirmed again every 12 months, counted from the last recorded confirmation | `party` | `date_field: contact_confirmed_on` | `check_kind: x-confirmation` | carried |
| Laboratory QM | 2: a procedure is reviewed again every 24 months, counted from the last recorded review | `procedure` | `date_field: last_reviewed` | `check_kind: x-review` | carried |
| Inspection body | 2: an approval lapses 24 months after the inspection, the date coming from the recorded inspection | `item` | `date_field: last_modified` | `check_kind: x-inspection` | carried |

A rule counts as carried because the run shows both cases: before any check
of its kind the entity is listed due now with the standing `never_checked`
and no date, and each recorded check by another identity moves the date to
the check's act plus the declared offset. The hand-typed field (set to
2020-01-01 where the type requires it) plays no part.

## The run

`replay.sh <memstead binary> <empty scratch directory>` builds one folder
workspace per rule, writes the entities under `author-a`, records the checks
under `checker-c` and `checker-d` with `--role checker` and `--observed-at`,
and reads `memstead due --today 2027-06-01 --within 3y`. Output of the run
with the engine at the commit that shipped the reading:

```text
## stock rule 1 (x-completion, 365 days)
before any check: - `probe--boiler-service` — Boiler service — **due now**: no confirming `x-completion` check (never_checked) (status: active, mem: probe)
after a check dated 2026-01-10: - `probe--boiler-service` — Boiler service — **2027-01-10** **OVERDUE** (142 days past) (status: active, mem: probe)
after a later check dated 2026-06-01: - `probe--boiler-service` — Boiler service — **2027-06-01** (in 0 days) (status: active, mem: probe)

## stock rule 9 (x-confirmation, 365 days)
before any check: - `probe--gas-supplier` — Gas supplier — **due now**: no confirming `x-confirmation` check (never_checked) (status: current, mem: probe)
after a check dated 2026-01-10: - `probe--gas-supplier` — Gas supplier — **2027-01-10** **OVERDUE** (142 days past) (status: current, mem: probe)
after a later check dated 2026-06-01: - `probe--gas-supplier` — Gas supplier — **2027-06-01** (in 0 days) (status: current, mem: probe)

## lab rule 2 (x-review, 730 days)
effective on a review dated 2026-01-10: - `probe--titration` — Titration — **2028-01-10** (in 223 days) (status: effective, mem: probe)
after a later review dated 2026-06-01: - `probe--titration` — Titration — **2028-05-31** (in 365 days) (status: effective, mem: probe)

## inspection rule 2 (x-inspection, 730 days)
approved on an inspection dated 2026-01-10: - `probe--lift` — Lift — **2028-01-10** (in 223 days) (status: approved, mem: probe)
after a later inspection dated 2026-06-01: - `probe--lift` — Lift — **2028-05-31** (in 365 days) (status: approved, mem: probe)
```

## Files

| Path | What it is |
|---|---|
| `stockadmin/`, `labqm/`, `inspection-body/` | The 2026-11-10 packages, migrated, with `due` reading a check kind on the four rules. |
| `replay.sh` | The run above. |
