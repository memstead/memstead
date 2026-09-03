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

try:
    import yaml
except ModuleNotFoundError:  # pragma: no cover - environment guard
    sys.exit(
        "  ✗ verify_gate.py needs PyYAML (it parses the guide's printed workflow "
        "rather than pattern-matching it).\n"
        "    Install it: python3 -m pip install pyyaml\n"
        "    This is the only non-stdlib import in ci/; the failure above is a "
        "missing package, not a broken example."
    )
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURE = REPO / "ci" / "fixtures" / "verify-gate"
GUIDE = REPO / "docs-site" / "src" / "content" / "docs" / "guides" / "verify-in-ci.md"

BINDING = "docs/graph"
GATE_ARGS = ["projection", "verify", BINDING, "--fail-on-findings"]
# The command the copyable workflow must run, asserted INSIDE the workflow
# block rather than anywhere in the page. A bare page-wide substring is not
# an anti-drift check — the same words appear in the guide's `--json`
# example further down, so deleting the whole workflow once left the
# assertion passing. (An earlier version of this comment claimed the design
# also survives a single-line `run:`; it did not — the hand-rolled locator
# ran past its own step into the next one's script. That whole extractor is
# gone: `extract_job_steps` parses the YAML properly.)
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
    # The claim four grading rounds kept finding falsified elsewhere. Reworded
    # 2026-09-03 (C6): the `#verified` baseline now rides `--advance`, so the
    # blanket "not read-only" claim became wrong in the other direction — a
    # gate run leaves the mem's CONFIG byte-identical. What a consumer must
    # still implement against is the sidecar write, so that is what is pinned,
    # together with the sentence that keeps the config promise honest.
    "It is not a pure read",
    "The mem's config is a different matter: a bare verify leaves it untouched.",
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


def run_gate(
    memstead: Path,
    workspace: Path,
    json_mode: bool = True,
    binding: str | None = None,
) -> tuple[int, str]:
    """Run the gate. `json_mode=False` is verbatim what the guide prints.

    `binding` overrides the documented one, for the operational-failure
    polarity only — the drift checks must keep using GATE_ARGS verbatim
    so the anti-drift pin stays meaningful.
    """
    args = GATE_ARGS if binding is None else [
        binding if a == BINDING else a for a in GATE_ARGS
    ]
    argv = [str(memstead)] + (["--json"] if json_mode else []) + args
    proc = subprocess.run(argv, cwd=workspace, capture_output=True, text=True)
    return proc.returncode, proc.stdout


# Step keys a documented example may legally carry. Anything else is
# rejected outright rather than blacklisted: a grade disarmed the gating
# step with `if: ${{ 1 == 2 }}`, which no substring list of false-y
# expressions would have caught, and there is no end to that list. An
# example that needs a conditional is not an example a stranger can copy.
ALLOWED_STEP_KEYS = {"name", "uses", "with", "run"}

# Steps that set up the environment this harness has already set up: it
# stages a checkout-equivalent fixture and puts the binary on PATH. Named
# explicitly — and still required to be present by DOCUMENTED_JOB_LINES —
# so "skipped" never quietly becomes "missing from the printed job".
SETUP_STEP_NAMES = {"Install memstead"}

# The step whose absence a grade proved survivable — kept as a required
# name so deleting it fails loudly rather than silently shrinking the job.
REQUIRED_STEP_NAMES = ("Verify the mem against its source", VERDICT_STEP_NAME)


def extract_job_steps(guide_text: str) -> tuple[list[dict], str | None]:
    """Parse the guide's workflow with a real YAML parser.

    Four rounds of hand-rolled extraction were evaded four different ways —
    a decoy ```yaml block, a third step inserted between the two this
    harness knew by name, the steps swapped so the printed order differed
    from the executed one, and a `run:` locator that ran past its own step
    into the next one's script. The lesson is not "add a fifth check": it
    is that enumerating what you expect cannot gate what you did not.
    So: exactly one workflow block, every step in printed order, no key
    outside the allowed set. Returns (steps, error).
    """
    blocks = re.findall(r"```ya?ml\n(.*?)```", guide_text, re.DOTALL)
    workflows = []
    for block in blocks:
        try:
            doc = yaml.safe_load(block)
        except yaml.YAMLError as e:
            return [], f"a ```yaml block in the guide is not valid YAML: {e}"
        if isinstance(doc, dict) and "jobs" in doc:
            workflows.append(doc)
    if len(workflows) != 1:
        return [], (
            f"expected exactly one workflow block in the guide, found "
            f"{len(workflows)} — a second one lets a decoy satisfy this harness "
            f"while the job a reader copies is broken"
        )

    jobs = workflows[0]["jobs"]
    if len(jobs) != 1:
        return [], f"expected one job in the printed workflow, found {len(jobs)}"
    steps = next(iter(jobs.values())).get("steps") or []

    for i, step in enumerate(steps):
        extra = set(step) - ALLOWED_STEP_KEYS
        if extra:
            return [], (
                f"step {i + 1} ({step.get('name', '<unnamed>')!r}) carries "
                f"{sorted(extra)}, which can disarm it regardless of its script — "
                f"a copyable example carries none of these"
            )
    names = [s.get("name") for s in steps]
    for required in REQUIRED_STEP_NAMES:
        if required not in names:
            return [], f"the printed job no longer has a {required!r} step"
    if names.index(REQUIRED_STEP_NAMES[0]) > names.index(REQUIRED_STEP_NAMES[1]):
        return [], (
            "the printed job runs the verdict check before the report exists — "
            "in that order a copied job fails every build"
        )
    return steps, None


def run_job(steps: list[dict], workspace: Path, memstead: Path) -> int:
    """Run every `run:` step in printed order, as a copied workflow would.

    `memstead` goes on PATH rather than being substituted into the script,
    so the command text executed is byte-for-byte what the guide prints.
    `bash -e` matches GitHub's default step shell. Steps that only `uses:`
    an action are environment setup this harness already provides.
    """
    env = {**os.environ, "PATH": f"{memstead.parent}:{os.environ['PATH']}"}
    for step in steps:
        script = step.get("run")
        if not script or step.get("name") in SETUP_STEP_NAMES:
            continue
        proc = subprocess.run(
            ["bash", "-e", "-c", script],
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

    # (3) The polarity that gives the contract its point: an operational
    #     failure must NOT come back as the findings code. A CI job that
    #     cannot tell "the mem drifted" from "the engine could not run"
    #     will merge broken work the moment the engine breaks — and the
    #     failure mode is silent, because both look like a red build.
    #     This harness claimed to assert it long before it did; the gap
    #     was found by a grade reading the docstring against run().
    with tempfile.TemporaryDirectory() as tmp:
        workspace = stage_fixture(Path(tmp))
        code, out = run_gate(memstead, workspace, binding="docs/nonexistent")
        if code == EXIT_FINDINGS:
            failures += fail(
                "an unknown binding returned the findings code — an operational "
                f"failure is indistinguishable from drift:\n{out}"
            )
        elif code != EXIT_NOT_FOUND:
            failures += fail(
                f"unknown binding exited {code}, expected {EXIT_NOT_FOUND}\n{out}"
            )
        elif "PROJECTION_NOT_FOUND" not in out:
            failures += fail(f"unknown binding emitted no typed envelope:\n{out}")
        else:
            print("  ✓ unknown binding → exit 3 (operational), never 6")

    # (4) Run the printed job END TO END, as a copied workflow would.
    #     Step 1 produces the file step 2 reads; running them separately is
    #     how the previous harness passed a job that went green on drift.
    steps, step_err = extract_job_steps(guide_text)
    if step_err:
        failures += fail(step_err)
    else:
        # clean → the job passes. drifted → step 1 fails it. inconclusive →
        # step 1 exits 0 and the verdict step must fail it; that is its
        # entire reason for existing, and the case the exit code cannot
        # express.
        for kind, want_pass in (("clean", True), ("drifted", False), ("inconclusive", False)):
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
                rc = run_job(steps, ws, memstead)
                if (rc == 0) != want_pass:
                    failures += fail(
                        f"the printed job returned {rc} on a {kind} run — expected "
                        f"{'success' if want_pass else 'failure'}. A stranger copying "
                        f"this workflow would get the wrong answer."
                    )
                else:
                    verb = "passes" if want_pass else "fails"
                    print(f"  ✓ the printed job {verb} a {kind} run (every step, in order)")

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
