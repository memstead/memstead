#!/usr/bin/env python3
"""Exercise the documented verify-in-CI gate, in both polarities.

The guide at `docs-site/.../guides/verify-in-ci.md` prints a GitHub
Actions job a stranger is meant to copy. A printed example nobody runs
rots silently, so this harness runs the same command against the
committed fixture workspace (`ci/fixtures/verify-gate/`) and asserts all
three outcomes of the exit-code contract:

    clean source            -> 0
    drifted source          -> 6   (the dedicated findings code)
    unknown binding         -> 3   (an operational failure, NOT 6)

The third is the point of the whole contract: a CI job must be able to
tell "the mem drifted from its source" from "the engine could not run".
Asserting 0 and 6 alone would leave that untested.

Anti-drift: the command string this harness executes is also asserted to
appear verbatim in the guide. Rewording the example, or changing what we
run, fails the check — so the printed workflow and the exercised one
cannot diverge without turning CI red.

Fixture seam: a git repo cannot be committed inside a git repo, so the
fixture's `src/` is not itself a checkout. This harness copies the
fixture to a temp directory and `git init`s it there. A real user needs
no such step (their source tree IS the checkout). The copy also keeps
the working tree clean, since verify writes a findings store, backfills
anchor hashes, and records a `#verified` baseline.

Invocation::

    python3 ci/verify_gate.py --memstead path/to/target/debug/memstead
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURE = REPO / "ci" / "fixtures" / "verify-gate"
GUIDE = REPO / "docs-site" / "src" / "content" / "docs" / "guides" / "verify-in-ci.md"

BINDING = "docs/graph"
GATE_ARGS = ["projection", "verify", BINDING, "--fail-on-findings"]
# The command the copyable workflow must run, asserted INSIDE the ```yaml
# block rather than anywhere in the page. A bare page-wide substring is not
# an anti-drift check — the same words appear in the guide's `--json`
# example further down, so deleting the whole workflow once left the
# assertion passing. Scoping to the block survives the job being reshaped
# (single-line `run:` vs. a `run: |` script) while still failing if the job
# is removed or the command changed.
DOCUMENTED_STEP = "memstead projection verify docs/graph --fail-on-findings"
# The rest of the printed job. Pinning only the `run:` line left three of the
# workflow's four steps free to rot: a grade broke the checkout action, the
# install URL and `fetch-depth` and this harness stayed green. Each entry is a
# line the copied job needs to actually work.
DOCUMENTED_JOB_LINES = [
    # Without a trigger the workflow never runs at all.
    "on: [pull_request]",
    # The install step is a `curl … | sh`; it needs a POSIX runner.
    "runs-on: ubuntu-latest",
    "uses: actions/checkout@v4",
    # The guide's own cap prose says a shallow clone hides drift, so this is
    # load-bearing, not decoration.
    "fetch-depth: 0",
    "https://memstead.io/install.sh",
    'echo "$HOME/.memstead/bin" >> "$GITHUB_PATH"',
    # Step 1 pipes through `tee`; without pipefail the step's status is
    # tee's, so exit 6 is swallowed and the gate silently stops failing.
    "set -o pipefail",
    # Step 2 — the step the guide calls "what makes the gate trustworthy",
    # and the half that was neither pinned nor run until a grade deleted it
    # and this harness stayed green.
    "Require a conclusive verdict",
]

# The heading of the guide's second step, used to locate its script below.
VERDICT_STEP_NAME = "Require a conclusive verdict"
# The command that step runs, as a stranger would type it: human mode, no
# global --json. The harness runs BOTH modes, because the mode the guide
# prints must be the mode something gates.
DOCUMENTED_COMMAND = "memstead projection verify docs/graph --fail-on-findings"

# The guide's CONTRACT prose, as opposed to its copyable job. A grade broke
# five of these — the exit-table row, the version marker, the finding-class
# vocabulary, the jq recipe, and the "not a read-only command" sentence — and
# this harness stayed green, because it pinned only the YAML. Criterion 6's
# complement had no machine gate at all. These are the claims a consumer
# actually implements against, so they are worth more than the workflow.
DOCUMENTED_CONTRACT = [
    # The dedicated findings code, in the outcomes table.
    "| `6` |",
    # The version marker consumers assert before parsing.
    '"format": "memstead-verify/v1"',
    # The closed finding-class vocabulary, every member.
    "`drifted`, `wrong`,",
    "`uncovered`, `unresolvable-anchor`, `queued-for-adjudication`",
    # The recipe that works on the two-document gate path. A plain
    # `jq -r .code` does not, and the guide used to print it.
    "jq -s -r '.[-1].code'",
    # The claim four grading rounds kept finding falsified elsewhere.
    "It is not a read-only command",
]

# The generated CLI reference renders from the clap epilog, and it is the
# page this guide's own "Related" section sends a CI author to for the
# exit-code table. It recommended `jq -r .code`, which the guide documents
# as misreading the gate path — so the two pages disagreed about the one
# command code 6 exists for. Pinned here rather than only in the guide.
CLI_REFERENCE = (
    REPO / "docs-site" / "src" / "content" / "docs" / "reference" / "cli" / "cli.md"
)
CLI_REFERENCE_CONTRACT = ["jq -s -r '.[-1].code'"]

# Every exit code the binary can actually return. The guide's prose must
# not mention one outside this set: a grade rewrote every prose mention of
# "exit 6" to "exit 7", left the pinned table cell intact, and this harness
# stayed green — the guide then told a stranger to branch on a code the
# binary never returns. Pinning six strings gates six strings; the prose
# BETWEEN them still drifts. So this checks the class of claim rather than
# adding a seventh pin.
REAL_EXIT_CODES = {"0", "1", "2", "3", "4", "5", "6"}

EXIT_CLEAN = 0
EXIT_FINDINGS = 6
EXIT_NOT_FOUND = 3


def fail(message: str) -> int:
    print(f"  ✗ {message}")
    return 1


def git(cwd: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-c", "user.email=ci@memstead.test", "-c", "user.name=ci", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
    )


def stage_fixture(dest: Path) -> Path:
    """Copy the committed fixture and make its source tree a git checkout."""
    workspace = dest / "verify-gate"
    shutil.copytree(FIXTURE, workspace)
    src = workspace / "src"
    git(src, "init", "-q", ".")
    git(src, "add", "-A")
    git(src, "commit", "-qm", "fixture base")
    return workspace


def run_gate(memstead: Path, workspace: Path, json_mode: bool = True) -> tuple[int, str]:
    """Run the gate. `json_mode=False` is verbatim what the guide prints."""
    argv = [str(memstead)] + (["--json"] if json_mode else []) + GATE_ARGS
    proc = subprocess.run(argv, cwd=workspace, capture_output=True, text=True)
    return proc.returncode, proc.stdout


# The two steps of the guide's job, by their `name:` headings. The job is
# executed as a COMPOSED whole below — step 1 producing the file step 2
# reads — because running them in isolation is how the last version of this
# harness passed a job that went green on real drift: `|| true` on step 1
# plus a narrowed condition in step 2 are each survivable alone and
# ungating together. A gate that cannot see the seam cannot gate the seam.
STEP_NAMES = ("Verify the mem against its source", VERDICT_STEP_NAME)

# GitHub-Actions attributes that neuter a step regardless of its script.
# None of them appear in a working job, and each silently disarms the gate.
FORBIDDEN_JOB_ATTRS = ("continue-on-error", "if: false", "if: ${{ false")


def extract_step_scripts(guide_text: str) -> dict[str, str] | None:
    """Lift each named step's shell script out of the guide's YAML block.

    Returns a name→script map, or None when the block or a step is missing.
    """
    for block in re.findall(r"```yaml\n(.*?)```", guide_text, re.DOTALL):
        if not all(name in block for name in STEP_NAMES):
            continue
        scripts: dict[str, str] = {}
        for name in STEP_NAMES:
            after = block.split(f"name: {name}", 1)[1]
            lines = after.splitlines()
            try:
                start = next(
                    i for i, ln in enumerate(lines) if ln.strip() in ("run: |", "run:")
                )
            except StopIteration:
                return None
            head = lines[start].strip()
            if head != "run: |":
                # A single-line `run: cmd` step.
                scripts[name] = head[len("run:") :].strip()
                continue
            body, indent = [], None
            for ln in lines[start + 1 :]:
                if not ln.strip():
                    body.append("")
                    continue
                lead = len(ln) - len(ln.lstrip())
                if indent is None:
                    indent = lead
                elif lead < indent:
                    break
                body.append(ln[indent:])
            scripts[name] = "\n".join(body).strip()
        return scripts
    return None


def run_job(scripts: dict[str, str], workspace: Path, memstead: Path) -> int:
    """Run the printed job end to end, as a copied workflow would.

    `memstead` is put on PATH rather than substituted into the script, so the
    command text executed is byte-for-byte what the guide prints. Steps run in
    order and the first non-zero status ends the job — GitHub's own semantics.
    """
    env = {**os.environ, "PATH": f"{memstead.parent}:{os.environ['PATH']}"}
    for name in STEP_NAMES:
        proc = subprocess.run(
            ["bash", "-c", scripts[name]],
            cwd=workspace,
            capture_output=True,
            text=True,
            env=env,
        )
        if proc.returncode != 0:
            return proc.returncode
    return 0


def run(memstead: Path) -> int:
    failures = 0

    # (0) The example and the exercise must be the same command.
    if not GUIDE.exists():
        return fail(f"the guide this harness gates does not exist: {GUIDE}")
    # The two constants must describe the same command, or the anti-drift
    # check below asserts a string this harness never runs. Cheap, and it
    # closes the last way the guide and the exercise could disagree.
    built = "memstead " + " ".join(GATE_ARGS)
    if built != DOCUMENTED_COMMAND:
        failures += fail(
            f"the harness runs {built!r} but asserts {DOCUMENTED_COMMAND!r} into the "
            f"guide — the anti-drift check would be pinning the wrong command"
        )

    guide_text = GUIDE.read_text(encoding="utf-8")
    yaml_blocks = re.findall(r"```yaml\n(.*?)```", guide_text, re.DOTALL)
    if not yaml_blocks:
        failures += fail("the guide no longer contains a copyable YAML workflow block")
    elif not any(DOCUMENTED_STEP in block for block in yaml_blocks):
        failures += fail(
            f"no YAML block in the guide runs the command this harness exercises "
            f"({DOCUMENTED_STEP!r}) — the example and the exercise have drifted, "
            f"or the job was removed outright"
        )
    else:
        print("  ✓ the guide's workflow runs the command this harness runs")

    job = "\n".join(yaml_blocks) if yaml_blocks else ""
    missing = [line for line in DOCUMENTED_JOB_LINES if line not in job]
    if missing:
        failures += fail(
            f"the guide's copyable workflow lost {len(missing)} load-bearing "
            f"line(s) — a reader copying it would get a job that does not work: "
            f"{missing}"
        )
    else:
        print("  ✓ the printed workflow still carries every step it needs")

    if CLI_REFERENCE.exists():
        cli_text = CLI_REFERENCE.read_text(encoding="utf-8")
        missing_cli = [c for c in CLI_REFERENCE_CONTRACT if c not in cli_text]
        if missing_cli:
            failures += fail(
                f"the generated CLI reference lost the stream-aware recipe "
                f"{missing_cli} — it is where the guide sends a CI author for the "
                f"exit-code table, and the plain `jq -r .code` misreads exit 6"
            )
        else:
            print("  ✓ the CLI reference carries the recipe that works on exit 6")

    missing_contract = [c for c in DOCUMENTED_CONTRACT if c not in guide_text]
    if missing_contract:
        failures += fail(
            f"the guide lost {len(missing_contract)} contract claim(s) this harness "
            f"holds it to — a consumer implementing the documented contract would "
            f"now be implementing something else: {missing_contract}"
        )
    else:
        print("  ✓ the documented contract still says what the binary does")

    # `exit 7`, `exits 9`, `exit code 42` — any number the binary cannot
    # produce. Deliberately narrow: only the phrasings that instruct a
    # reader to branch on a code, so prose about (say) 40 minutes is not
    # swept up.
    # Three shapes, because a grade dodged the first with a hyphen and a
    # table row: prose ("exit 7", "exit-code 7", "returns code 9"), and the
    # outcomes table's own leading cell (`| `7` |`). The table row is the
    # sharpest miss — it is where a reader looks first, and it needs no
    # `exit` token at all.
    patterns = [
        r"exits?[\s-]+(?:code[\s-]+)?`?(\d+)`?",
        r"(?:returns?|yields?)\s+(?:exit[\s-]*)?code\s+`?(\d+)`?",
        r"^\|\s*`(\d+)`\s*\|",
    ]
    bogus = sorted(
        {
            m.group(1)
            for pat in patterns
            for m in re.finditer(pat, guide_text, re.MULTILINE)
            if m.group(1) not in REAL_EXIT_CODES
        }
    )
    if bogus:
        failures += fail(
            f"the guide tells a reader to branch on exit code(s) the binary never "
            f"returns: {bogus} — the real set is {sorted(REAL_EXIT_CODES)}"
        )
    else:
        print("  ✓ every exit code the guide names is one the binary returns")

    with tempfile.TemporaryDirectory() as tmp:
        workspace = stage_fixture(Path(tmp))

        # (1) Clean source: the gate passes, and says so in the payload.
        code, out = run_gate(memstead, workspace)
        if code != EXIT_CLEAN:
            failures += fail(f"clean fixture exited {code}, expected {EXIT_CLEAN}\n{out}")
        else:
            try:
                env = json.loads(out)
            except json.JSONDecodeError:
                failures += fail(f"clean run emitted no JSON envelope:\n{out}")
            else:
                if env.get("format") != "memstead-verify/v1":
                    failures += fail(f"missing/!= version marker: {env.get('format')!r}")
                elif env.get("rollup", {}).get("verdict") != "clean":
                    failures += fail(f"clean run verdict: {env.get('rollup')}")
                else:
                    print("  ✓ clean fixture → exit 0, verdict clean, envelope versioned")

        # (2) Drift the anchored source. The gate must fail with the
        #     dedicated code, and the report must still reach stdout —
        #     a red build with no report is a red build nobody can act on.
        alpha = workspace / "src" / "alpha.md"
        alpha.write_text("Alpha now says something else entirely.\n", encoding="utf-8")
        git(workspace / "src", "add", "-A")
        git(workspace / "src", "commit", "-qm", "drift alpha")

        code, out = run_gate(memstead, workspace)
        if code != EXIT_FINDINGS:
            failures += fail(f"drifted fixture exited {code}, expected {EXIT_FINDINGS}\n{out}")
        elif "memstead-verify/v1" not in out:
            failures += fail(f"the report did not reach stdout before the gate failed:\n{out}")
        elif "PROJECTION_VERIFY_FINDINGS" not in out:
            failures += fail(f"the typed error envelope did not reach stdout:\n{out}")
        else:
            print("  ✓ drifted fixture → exit 6, report and typed error both on stdout")

        # (2b) The SAME command in the mode the guide actually prints. The
        #      job a stranger copies has no `--json`, so gating only the
        #      JSON path would leave the copied one exercised by nothing.
        code, out = run_gate(memstead, workspace, json_mode=False)
        if code != EXIT_FINDINGS:
            failures += fail(
                f"human-mode gate (the mode the guide prints) exited {code}, "
                f"expected {EXIT_FINDINGS}\n{out}"
            )
        elif "Verdict: DRIFTED" not in out:
            failures += fail(f"human-mode gate rendered no verdict before failing:\n{out}")
        elif "Do next:" not in out:
            failures += fail(f"human-mode gate rendered no actions:\n{out}")
        else:
            print("  ✓ human-mode gate (as printed in the guide) → exit 6, verdict rendered")

        # (2c) The guide tells readers to recover the typed code from the
        #      LAST stdout document, because the gate path emits two. Prove
        #      the shape that advice assumes.
        code, out = run_gate(memstead, workspace)
        decoder = json.JSONDecoder()
        parsed, idx = [], 0
        try:
            while idx < len(out):
                while idx < len(out) and out[idx] in " \t\r\n":
                    idx += 1
                if idx >= len(out):
                    break
                obj, idx = decoder.raw_decode(out, idx)
                parsed.append(obj)
        except json.JSONDecodeError as exc:
            failures += fail(f"gate stdout is not a JSON document stream: {exc}\n{out}")
            parsed = []
        if parsed:
            if len(parsed) != 2:
                failures += fail(
                    f"gate stdout carried {len(parsed)} JSON document(s), expected 2 "
                    f"(report then typed error) — the documented jq recipe assumes 2"
                )
            elif parsed[0].get("format") != "memstead-verify/v1":
                failures += fail(f"first document is not the report envelope: {parsed[0]!r}")
            elif parsed[-1].get("code") != "PROJECTION_VERIFY_FINDINGS":
                failures += fail(f"last document is not the typed error: {parsed[-1]!r}")
            else:
                print("  ✓ gate stdout is report-then-error, as the guide's jq recipe assumes")

    # (4) Run the printed job END TO END, as a copied workflow would.
    #     Step 1 produces the file step 2 reads; running them separately is
    #     how the previous harness passed a job that went green on drift.
    scripts = extract_step_scripts(guide_text)
    if scripts is None:
        failures += fail(
            "could not find both named steps and their scripts in the guide's "
            "workflow — the job a stranger copies is not there to run"
        )
    else:
        for attr in FORBIDDEN_JOB_ATTRS:
            if any(attr in b for b in yaml_blocks):
                failures += fail(
                    f"the printed job carries {attr!r}, which disarms a step "
                    f"whatever its script says — the gate would not gate"
                )

        # clean → the job passes. drifted → step 1 fails it. inconclusive →
        # step 1 exits 0 and step 2 must fail it; that is step 2's whole job.
        polarities = [("clean", 0), ("drifted", 6), ("inconclusive", 1)]
        for kind, want in polarities:
            with tempfile.TemporaryDirectory() as tmp2:
                ws = stage_fixture(Path(tmp2))
                if kind == "drifted":
                    (ws / "src" / "alpha.md").write_text(
                        "Alpha now says something else entirely.\n", encoding="utf-8"
                    )
                    git(ws / "src", "add", "-A")
                    git(ws / "src", "commit", "-qm", "drift alpha")
                elif kind == "inconclusive":
                    binding = ws / ".memstead" / "projections" / "docs" / "graph.json"
                    binding.write_text(
                        binding.read_text(encoding="utf-8").replace(
                            '"change_detection":"git"', '"change_detection":"none"'
                        ),
                        encoding="utf-8",
                    )
                rc = run_job(scripts, ws, memstead)
                ok = (rc == 0) if want == 0 else (rc != 0)
                if not ok:
                    failures += fail(
                        f"the printed job returned {rc} on a {kind} run — expected "
                        f"{'success' if want == 0 else 'failure'}. A stranger copying "
                        f"this workflow would get the wrong answer."
                    )
                else:
                    verb = "passes" if want == 0 else "fails"
                    print(f"  ✓ the printed job {verb} a {kind} run (end to end)")

    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--memstead", type=Path, required=True)
    args = parser.parse_args()
    if not args.memstead.is_file():
        return fail(f"no memstead binary at {args.memstead}")
    # Resolved, because every run below sets cwd to the staged workspace —
    # a relative binary path would be looked up there and vanish.
    return run(args.memstead.resolve())


if __name__ == "__main__":
    sys.exit(main())
