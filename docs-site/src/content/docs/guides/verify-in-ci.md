---
title: Gate a pull request on mem fidelity
description: "Run `memstead projection verify` in CI so a pull request fails when the mem drifts from the source it describes — exit codes, the versioned JSON report, and what the gate cannot see."
sidebar:
  order: 7
---

A mem describes a source. The source moves. Without a gate, the two
drift apart quietly and you find out months later, from an agent that
confidently tells you something that stopped being true.

`memstead projection verify` measures that drift. This guide turns the
measurement into a pull-request gate.

## The three outcomes

The gate mode is opt-in. Add `--fail-on-findings` and verify reports
one of three outcomes:

| Exit | Meaning | What CI should do |
|------|---------|-------------------|
| `0` | The run completed and recorded no findings | **Read the verdict — see below** |
| `6` | The run completed and **recorded findings** | Fail — the mem and its source disagree |
| anything else | The measurement itself failed | Fail — but this is a broken job, not a drifted mem |

**Exit 0 is necessary, not sufficient.** It says the run recorded no
findings; it does not say the run could see anything. A pass whose verdict
is `inconclusive` — a facet with no readable change signal, or an empty
enumerated scope — also records no findings and also exits 0. The exit code has no representation for that third answer,
so a job that branches on the code alone goes green on exactly the runs
this guide tells you not to gate on. Read `rollup.verdict`; the job below
does.

One more case the table cannot show: a run that records findings **and**
then fails to write its bookkeeping (an unwritable `#verified` baseline,
say) exits `1`, not `6` — the measurement's answer is on stdout, but the
run could not finish recording it, and that is an operational failure.
Rare, and the report is still there to read.

Code `6` is deliberately outside the ordinary error range. Codes 1–5 all
mean *the run could not complete*; 6 means *it completed and you should
care about the answer*. A run that fails returns its own code, so a job
can tell "the mem and its source disagree" from "the engine could not
boot" without parsing any output.

The line is **did the measurement complete**, not *was everything well*.
An artifact the pass could not read is a finding — it was observed, and
being unable to adjudicate it is the measurement's answer. An input the
pass could not read at all is not: an unreadable anchors sidecar refuses
with `ANCHORS_SIDECAR_UNREADABLE` (exit 5) rather than reporting every
artifact uncovered, because "no anchors parsed" and "no anchors exist"
are different facts and only one of them is the mem's fault.

Without `--fail-on-findings`, verify exits 0 whether it found anything
or not — unchanged, so adding the flag breaks no existing script.

The gate fires on **any** finding class, not only `drifted`. On a mem
still being backfilled onto a new binding, that includes `uncovered`
artifacts — real work, but onboarding rather than drift. The rollup says
so (`verdict: inconclusive`, with the backfill framing); the exit code
does not distinguish. Finish the backfill before turning the gate on.

## The job

```yaml
name: mem-fidelity

on: [pull_request]

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0        # verify compares against git history

      - name: Install memstead
        run: |
          curl -fsSL https://memstead.io/install.sh | sh
          echo "$HOME/.memstead/bin" >> "$GITHUB_PATH"

      - name: Verify the mem against its source
        run: |
          set -o pipefail
          memstead projection verify docs/graph --fail-on-findings --json \
            | tee report.json
        # Exit 6 fails the step here. Exit 0 continues to the check below.

      - name: Require a conclusive verdict
        run: |
          verdict=$(jq -s -r '.[0].rollup.verdict' report.json)
          echo "verdict: $verdict"
          if [ "$verdict" != "clean" ]; then
            echo "::error::verify could not support a clean verdict"
            jq -s -r '.[0].rollup.blind_spots[]' report.json
            exit 1
          fi
```

The second step is what makes the gate trustworthy: it fails on
`inconclusive` as well as on drift, so a run that could not see its source
does not pass for lack of anything to report. Drop it only if you have
read the caps below and accept a green build on an unmeasurable binding.

Replace `docs/graph` with your binding id (`memstead projection brief`
lists them). `fetch-depth: 0` matters: a git-change-detection binding
compares the source against recorded history, and a shallow clone hides
it.

That job is not decorative. This repository runs the same command
against a committed fixture on every push, in both polarities — clean
and drifted — so an example that stopped working turns our own CI red
before it reaches you.

## Reading the failure

A red build carries its own explanation. The full report is rendered
**before** the exit code fires, so stdout holds the verdict and the
ranked next actions:

```
# Fidelity report — `docs/graph`

**Verdict: DRIFTED** — 1 finding(s) recorded over the current key (drifted: 1).

**Do next:**

1. 1 anchored artifact(s) moved since the entity was written — re-read the
   source and update the entity, then re-verify to advance the baseline
```

## Machine-readable output

Add the global `--json` flag for a structured payload:

```bash
memstead projection verify docs/graph --fail-on-findings --json
```

The payload is **external contract**. It opens with a version marker —
assert it before parsing, so a future shape change fails loudly instead
of misparsing:

```json
{
  "format": "memstead-verify/v1",
  "rollup": {
    "verdict": "drifted",
    "findings_total": 1,
    "because": "1 finding(s) recorded over the current key (drifted: 1)",
    "blind_spots": [],
    "actions": ["1 anchored artifact(s) moved since the entity was written — …"]
  },
  "report": { "findings_by_class": { "drifted": 1 }, "coverage": { … } }
}
```

`rollup.verdict` is one of three values:

- **`clean`** — the pass was substantive on every axis and found nothing.
- **`drifted`** — findings were recorded.
- **`inconclusive`** — the pass completed but **cannot support a green
  claim**. `blind_spots` names why. Treat this as "not yet gated", not
  as "passing". The triggers are not only the capability row: a facet that
  is not `enumerable`, one with no `change_signal`, one whose *resolved*
  `signal` is `none` (a binding declaring `change_detection: "none"`, or a
  git binding in a checkout with no `.git` — `change_signal` stays `true`
  in both), an empty enumerated scope, or a pass that adjudicated no
  anchor. Branch on `verdict`, never on the capability fields directly.

`report.findings_by_class` uses a closed vocabulary: `drifted`, `wrong`,
`uncovered`, `unresolvable-anchor`, `queued-for-adjudication`.
`report.coverage.denominator` is an internally-tagged union on `kind` —
either `{"kind": "enumerated", "count": N}` or
`{"kind": "non-enumerable", "reason": "…"}`. Branch on `kind`; a
`non-enumerable` denominator means an uncovered artifact is undetectable,
not that there are none.

Two per-facet arrays carry what the measurement *could* do, which is how
you tell a real green from a lucky one:

- **`report.capabilities[]`** — one row per source facet: `facet`,
  `medium_type`, `enumerable` (is `S(D)` computable), `change_signal`
  (can drift be observed at all), `base_version_retrievable`,
  `anchor_namespace` (`path` / `path+commit` / `entity` / `url`), and
  the resolved `signal` (`git` / `mtime` / `graph` / `none`). A `false`
  in `enumerable` or `change_signal` is what forces `inconclusive`.
- **`report.freshness[]`** — per facet: `signal`, the recorded `synced`
  and `verified` baselines (`null` when never recorded), and
  `change_detectable`. A facet with `change_detectable: false` is
  structurally unable to render a green freshness verdict.

Both are contract: they carry the same version marker and change only
with it.

### Two documents on the gate's failure path

On exit 6 stdout carries **two** JSON documents: the report envelope,
then the typed error envelope. That is the report-before-exit guarantee
paying out, but it means a plain `json.loads(stdout)` fails and the
usual `… --json | jq -r .code` recipe sees the first document too.

Read them as a stream:

```bash
# the typed code, from the last document on stdout
memstead projection verify docs/graph --fail-on-findings --json \
  | jq -s -r '.[-1].code'

# the verdict, from the first
memstead projection verify docs/graph --fail-on-findings --json \
  | jq -s -r '.[0].rollup.verdict'
```

On exit 0 there is exactly one document (the report), and on an
operational failure exactly one (the error envelope) — the two-document
case is specific to the findings exit.

## What this gate cannot see

A gate is only worth what its measurement covers. These caps are real
today, and the report names each one it hits rather than quietly
rendering green.

**Verify writes.** It is not a read-only command. A completed run
records a findings store, backfills observed content hashes into the
mem's anchors sidecar, and records a `#verified` baseline — on a
mem-repo workspace, that is a commit. On CI's ephemeral checkout this is
harmless and nothing needs pushing back. Do not run it in a working tree
you need to stay pristine.

**The anchor figures answer for this binding only.** On a mem carrying
several bindings, each binding's report counts its own anchors and names the
ones it excluded, with the reason. So two reports on one mem are two different
measurements, and neither is the mem's total. If you gate on the anchor figures,
gate per binding.

**A clean anchor axis means every counted row was adjudicated.** If any row
could not be (unobserved, a span never checked, an entity end nobody
reconciled), the verdict is inconclusive and the blind spot is named, so a gate
reading the verdict rather than the exit code sees it. Excluded rows are not
blind spots: an anchor outside this binding's scope is a correct answer, not a
gap.

**An anchor whose entity vanished is reported, not counted.** If something
wrote the mem from outside the engine and removed an entity, the sidecar row
naming it is reported as dangling and excluded from every anchor figure. It is
not repaired: the row is the evidence. If the mem could not be reconciled at all
(not mounted, quarantined, lazily unloaded, or carrying a file that failed to
parse) the report says so instead of showing a clean anchor axis, so a gate
reading the anchor figures should treat that statement as a blind spot.

**A medium with no change signal cannot show drift.** If a facet's
capability row reports `change_signal: false`, drift on it is
unobservable — not absent. The verdict degrades to `inconclusive` and
names the facet.

**Web sources are not enumerable.** Coverage over a web medium is
reported against anchors only, so an *uncovered* artifact cannot be
detected: there is no denominator to be uncovered against. Freshness is
similarly limited to what the medium exposes.

**The exit code cannot express `inconclusive`.** The contract has three
codes and the verdict has three values, but they are not the same three: a
run that completed and recorded nothing exits `0` whether it saw everything
or nothing. That is why the job above reads the verdict rather than trusting
the code. A CI-visible signal for "could not measure" would need a change to
the exit-code contract itself, which is not a change this guide can make on
its own.

**The mtime baseline does not survive a fresh checkout.** A binding using
mtime change-detection compares file modification times against a recorded
baseline, and a CI clone gives every file a fresh mtime, so the baseline is
meaningless there. That bounds the *changed-source slice* — what `sync`
acts on. `verify` adjudicates anchors by content hash, so a bumped-mtime
checkout still verifies clean rather than flagging everything: the cap is
real for the loop, milder for the gate. Use git change-detection for
anything you intend to gate.

## Related

- [The fidelity contract](../../concepts/fidelity-contract/) — what verify
  measures and why.
- [Grow a mem from a source](../grow-a-mem-from-a-source/) —
  creating the binding this guide gates.
- [CLI reference](../../reference/cli/cli/) — the full exit-code table.
