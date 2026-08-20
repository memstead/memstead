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
| `0` | The run completed and found nothing | Pass |
| `6` | The run completed and **recorded findings** | Fail — the mem and its source disagree |
| anything else | The measurement itself failed | Fail — but this is a broken job, not a drifted mem |

Code `6` is deliberately outside the ordinary error range. Codes 1–5 all
mean *the command failed*; 6 means *the command succeeded and you should
care about the answer*. No operational path ever returns 6, so a job can
tell "the mem drifted from its source" from "the engine could not boot"
without parsing any output.

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
        run: memstead projection verify docs/graph --fail-on-findings
```

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
  as "passing".

`report.findings_by_class` uses a closed vocabulary: `drifted`, `wrong`,
`uncovered`, `unresolvable-anchor`, `queued-for-adjudication`.
`report.coverage.denominator` is a tagged union — either
`{"Enumerated": {"count": N}}` or `{"NonEnumerable": {"reason": "…"}}`.

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

**A medium with no change signal cannot show drift.** If a facet's
capability row reports `change_signal: false`, drift on it is
unobservable — not absent. The verdict degrades to `inconclusive` and
names the facet.

**Web sources are not enumerable.** Coverage over a web medium is
reported against anchors only, so an *uncovered* artifact cannot be
detected: there is no denominator to be uncovered against. Freshness is
similarly limited to what the medium exposes.

**Graph-medium verify is currently inert.** A binding whose source is
another mem enumerates nothing and leaves its anchors permanently
unobserved — coverage reads `0/0` despite the capability row saying
`enumerable: true`. The rollup refuses to call that clean and returns
`inconclusive`, but the underlying measurement is not there yet. Do not
gate a graph-source binding on this today.

**The mtime baseline does not survive a fresh checkout.** A binding
using mtime change-detection compares file modification times against a
recorded baseline. A CI clone gives every file a fresh mtime, so such a
binding flags everything on its first run in a new checkout. Use
git change-detection for anything you intend to gate.

## Related

- [The fidelity contract](/concepts/fidelity-contract/) — what verify
  measures and why.
- [Grow a mem from a source](/guides/grow-a-mem-from-a-source/) —
  creating the binding this guide gates.
- [CLI reference](/reference/cli/cli/) — the full exit-code table.
