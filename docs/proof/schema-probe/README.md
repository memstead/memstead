# Schema probe: what today's schema language carries for inspection-type work (2026-11-10)

Three schemas, each written from the authoring guide alone for one field of
work, each run against ten rules given in advance. The question:

> For work in which someone checks a thing against a criterion and the result
> holds for a while, which rules can a schema make the engine refuse or report
> today, and which need an agent to keep something up by hand?

It is **one run, not a benchmark.** The schemas are in this folder and validate
against the engine; the verdicts below are what three sessions observed when
they ran each case and its opposite in a scratch workspace on engine 0.25.0.

## What it supports, and what it does not

- **It supports:** the statement that one reading is missing in all three
  fields: a due date taken from the newest recorded check of a kind. In each
  field a new check was recorded and the due brief did not move until an agent
  typed the date into a metadata field by hand.
- **It supports:** the statement that a third of the rules are carried by a
  declaration alone, with no agent keeping anything up.
- **It does not support:** any claim about how well these fields are served in
  practice. No holding in any of them exists. The rules were written by the
  author of the design question, not collected from practitioners.
- **It is the paper evidence** the decision
  [Admit a reading on paper evidence and keep the real-holding bar for a form that refuses a write](../../../engineering/admit-a-reading-on-paper-evidence-and-keep-the-real-holding-bar-for-a-form-that-refuses-a-write.md)
  asks for, for that one reading and for nothing else.

## The result

| Field | Carried | Workaround | Not carried |
|---|---:|---:|---:|
| Managed stock of installations (`stockadmin`) | 5 | 5 | 0 |
| Laboratory quality management (`labqm`) | 3 | 6 | 1 |
| Inspection body (`inspection-body`) | 2 | 6 | 2 |
| Of 30 | 10 | 17 | 3 |

- **Carried:** the engine refuses or reports it from a declaration alone.
- **Workaround:** it works only while an agent keeps something up by hand, or
  only for part of the rule.
- **Not carried:** nothing in the language does it.

### What is missing, by kind

| Missing in the schema language | Rules | Fields |
|---|---|---|
| A due date read from the newest check of a kind | stock 1, 9; lab 2; inspection 2 | all three |
| A gate that reads a related entity's field value or date, where today it reads only that entity's check standing | stock 7; lab 3; inspection 4, 10 | all three |
| A way to require that an entity carries an anchor | stock 2; lab 4; inspection 5 | all three |
| A comparison of numbers or of two dates | lab 6, 10; inspection 8 | two |
| Propagation that does not rest on a status or an edge someone sets by hand, or that filters by date | lab 7; inspection 6, 7 | two |
| Freshness or a derivation baseline scoped to a named field | lab 5; inspection 9 | two |
| Recurrence of a fixed calendar date | stock 8 | one |
| A count of distinct identities across check records | lab 8 | one |

### Managed stock of installations

| # | Rule | Verdict | Carried by, or what is missing |
|---|---|---|---|
| 1 | The next due date of a yearly duty follows from the last recorded completion | workaround | `due` with `offset_days: 365` works, but reads a field an agent must copy the completion date into |
| 2 | A completion counts only with its service report as evidence, confirmed by someone other than the commissioner | workaround | `requires_when` demands the section's text and a self-check gate demands another identity; nothing requires an anchor, and independence is measured against the entry's creator |
| 3 | Which duties an installation has depends on its kind | carried | `required_outgoing` with `when_field` / `when_value`, `severity: block` |
| 4 | A terminated contractor flags every duty that depends on it | carried | `status_propagation` |
| 5 | A case has a response deadline and must state what is lost if it is missed | carried | `due` with `lead_section`, and a required section |
| 6 | A case cannot be closed without outcome and reference number | carried | `requires_when`, `severity: block` |
| 7 | A project is approved only when every linked permit is granted and each grant was confirmed by someone else | workaround | `transition_requires_checks` reads the permits' check standing, never their status: a checked permit still at `applied` passes |
| 8 | A fixed calendar deadline that recurs every year | workaround | `due` on a date field; nothing produces the next year's occurrence |
| 9 | A standing fact must be confirmed again every 12 months, counted from the last recorded confirmation | workaround | as rule 1: the date is a field typed by hand, a recorded check does not move it |
| 10 | A duty is not due while a blocking case is open | carried | `due.unless_open_via` |

### Laboratory quality management

| # | Rule | Verdict | Carried by, or what is missing |
|---|---|---|---|
| 1 | A procedure becomes effective only after a review by someone other than its author | carried | `transition_requires_self_check` |
| 2 | A procedure is reviewed again every 24 months, counted from the last recorded review | workaround | the date is a field typed by hand |
| 3 | Equipment is in use only while its calibration is valid for 12 months | workaround | the gate measures the calibration's check standing, not its age: a checked calibration 22 months old passes; the due reading sits on the calibration, not on the equipment |
| 4 | A calibration carries its certificate as evidence and is flagged when the file changes | workaround | a file anchor and `verify-anchors` report the change; nothing requires the anchor |
| 5 | A new version of a procedure flags the trainings on the old one | workaround | `derivation: true` flags them, but on any edit of the procedure, not only on a new version |
| 6 | A deviation needs a root cause to close, and an effectiveness check no earlier than 90 days after closure | workaround | `requires_when` carries the root cause; no constraint compares two dates |
| 7 | Equipment found out of tolerance flags every result since its last good calibration | workaround | `status_propagation` flags every result of that equipment, with no filter by date |
| 8 | A critical procedure needs two approvals by two different people | workaround | two self-check kinds on a separate type; one person can give both |
| 9 | An inventory number is unique | carried | `unique` |
| 10 | A measured value outside the equipment's tolerance cannot be recorded as passed | not carried | no numeric comparison exists |

### Inspection body

| # | Rule | Verdict | Carried by, or what is missing |
|---|---|---|---|
| 1 | An object is approved only after an inspection recorded by someone other than the entry's author | carried | `transition_requires_self_check` |
| 2 | An approval lapses 24 months after the inspection, the date coming from the recorded inspection | workaround | `due` on `last_modified` plus 730 days; the date moves with any edit and never comes from the check |
| 3 | An object of class `b` needs one more check than one of class `a` | carried | conditional `required_outgoing` plus `transition_requires_checks` |
| 4 | An open major defect blocks approval; a minor one does not but is overdue after 30 days | workaround | the gate reads the defect's check standing, not its status: a confirming check on a defect still open opens the gate; the 30 days are carried by `due` on `created_date` |
| 5 | The inspection cites its report, and a changed report flags the approval | workaround | a file anchor reports the change; nothing requires the anchor, and drift does not touch the gate |
| 6 | A new edition of the catalogue flags approvals given under the old one | workaround | `status_propagation`, once someone sets the old edition to `superseded` by hand |
| 7 | A withdrawn inspector flags every approval resting on that inspector's inspections | workaround | `status_propagation` along edges written by hand; nothing ties a check's identity to an entity |
| 8 | A measured value must lie inside a range declared for the kind of object | not carried | `value_pattern` bounds one field by a regular expression, not against a bound declared elsewhere, and not conditional on the result |
| 9 | Changing the technical description after approval invalidates it; changing a remark does not | not carried | freshness covers the whole entity, and the check that licensed the approval is carried across every later edit while the status holds, so neither edit invalidates it |
| 10 | Re-approval needs every defect closed and each closure confirmed by someone else | workaround | the gate requires an independent check on each major defect and does not read that the defect is closed |

## Method

- **Rules first.** Ten rules per field were fixed before any schema was written,
  and the sessions were told not to add to them or to bend one to fit.
- **A run decides.** A rule counts as carried only where the session ran the
  failing case and saw the refusal or the report line, and ran the opposite
  case and saw it pass or stay silent.
- **Workaround is not carried.** Where a rule holds only while an agent
  remembers to write a date, a status or an edge, the verdict says what must be
  remembered.
- **Three sessions, each alone.** One session per field, none seeing another,
  each working from the authoring guide and the built-in schemas. Model: Claude
  Opus 5.5. Engine: 0.25.0. At least two identities per workspace, with roles
  declared on every write and check.
- **An independent design review preceded it.** Four readers, two of them
  models of other vendors, had tested the design question against the code and
  left the one reading named above standing.

## Honest bounds

- **One run, one model, one engine version.** Not a distribution.
- **The rules are the design author's.** They cover what the design question
  cared about. A practitioner would write other rules.
- **The schemas are probes.** Their write rules are stubs and their sections are
  the minimum the rules needed. They are not templates to copy.
- **Two verdicts show engine behaviour, not only the language.** Inspection
  rule 9 and the independence half of stock rule 2 turn on how a check stands
  today (a licensing check carried across later edits, independence compared
  with the first author). Those are open questions about the engine in their
  own right.
- **Only the missing reading is evidence for a change.** The other gaps are
  recorded here as observed. Most of them would need a form that refuses a
  write, and that keeps its own, higher bar.

## Rerun kit

Each package validates as written:

```sh
memstead schema validate docs/proof/schema-probe/2026-11-10/stockadmin
memstead schema validate docs/proof/schema-probe/2026-11-10/labqm
memstead schema validate docs/proof/schema-probe/2026-11-10/inspection-body
```

To replay a rule, install the package into a fresh folder workspace, create
the few entities the rule names under one identity, and record the check under
another with `--role checker`. Reports that depend on the date take a fixed day
through `memstead due --today <date>`.

2026-11-11: the three packages speak the schema language of 0.25.0
(`direction: incoming` / `outgoing`, `rel_type` / `rel_types` in constraint
entries). They validate on 0.25.0, and on later engines after
`memstead schema migrate <package> --write`, which rewrites those spellings to
`in` / `out` and `relationships`. The packages stay as they are, as the dated
evidence of the 2026-11-10 run.

## Files

| Path | What it is |
|---|---|
| `2026-11-10/stockadmin/` | The schema for the managed stock: nine types. |
| `2026-11-10/labqm/` | The schema for laboratory quality management: seven types. |
| `2026-11-10/inspection-body/` | The schema for the inspection body: seven types. |
| `README.md` | This note. |
