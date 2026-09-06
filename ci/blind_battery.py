#!/usr/bin/env python3
"""The blind battery: a mem-only reader against a source-only reader.

Eleven fixed questions about Memstead. For each, two fresh agents answer:
one may read only an installed engine mem through the memstead CLI, the
other may read only the Rust source under ``crates/``. The answers are
extracted, their citations stripped, and placed in a random A/B order. A
third fresh agent grades each pair, verifying claims against the source
and by running the release binary, and returns a JSON verdict. The result
maps back to mem or source through the recorded order.

The questions are the instrument; they stay fixed so runs compare. The
first run was 2026-09-04 (source 7, mem 4, engine mem 0.2.2 unsynced).

Phases, each a subcommand over one run directory::

    prepare   writes the questions, the A/B order, the reader prompts and
              a scratch workspace with the mem installed
    run       drives the readers or the graders through ``claude -p``
              (``--phase readers`` / ``--phase graders``); a session that
              spawns its own subagents skips this and drops the files in
    pair      strips citations from the answers and writes the pairs plus
              the grader prompts
    tally     reads the verdicts, maps A/B back to mem/source, writes
              result.json and result.md
    record    copies the run into a committable record, with the machine's
              paths replaced by placeholders (<run>, <memstead>, <source>,
              <mem>) and the reader workspace and grader scratch left out

Run directory layout: ``index.json`` (questions, order, models, paths),
``prompts/<key>-mem.md`` and ``prompts/<key>-src.md`` (reader prompts),
``answers/<key>-mem.md`` and ``answers/<key>-src.md`` (verbatim answers),
``pairs/<key>.md``, ``prompts/grader-<key>.md``, ``verdicts/<key>.json``,
``result.json``, ``result.md``.

``--self-test`` exercises the stripping, the order mapping and the tally
on inline fixtures and needs no agent, no binary and no network.

Invocation::

    python3 ci/blind_battery.py prepare --dir RUN --memstead BIN \\
        --mem PATH.mem --source . [--seed 20260904]
    python3 ci/blind_battery.py run --dir RUN --phase readers
    python3 ci/blind_battery.py pair --dir RUN
    python3 ci/blind_battery.py run --dir RUN --phase graders
    python3 ci/blind_battery.py tally --dir RUN
    python3 ci/blind_battery.py record --dir RUN --out docs/proof/blind-battery/<date>
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import random
import re
import subprocess
import sys
from pathlib import Path

# The eleven questions, verbatim from the 2026-09-04 run. Keys are the
# first run's keys so the two runs line up. The two `h` entries repeat a
# question with both readers on the small model.
QUESTIONS: list[tuple[str, str, bool]] = [
    (
        "Q0",
        "Two AI agents are editing the same entity in a Memstead mem at the same "
        "time. Can one of them silently overwrite the other's change? What exactly "
        "prevents that, and what does the losing agent see?",
        False,
    ),
    (
        "Q1",
        "I have never seen Memstead. Explain how it works end to end: what happens "
        "from the moment an AI agent asks to write a fact until that fact is a file "
        "in git, where the checks sit along the way, and what the agent reads back "
        "afterwards. One page, for a developer.",
        False,
    ),
    (
        "Q2",
        "If I install a mem that someone else published, what can it do to my AI "
        "agent, and what can it not do? Can its content act as instructions to my "
        "agent, can it change my own data, and what does the engine do to contain "
        "it?",
        False,
    ),
    (
        "Q3",
        "A knowledge graph about a codebase goes stale as the code moves on. What "
        "can go wrong with a Memstead mem over time, how does Memstead notice, and "
        "what does it do about it, automatically or on request?",
        False,
    ),
    (
        "N1",
        "Give me the exact shell commands, in order, to: (1) create a fresh Memstead "
        "workspace in an empty folder with the built-in default schema, (2) create "
        "an entity of type `concept` titled \"Optimistic locking\" with the sections "
        "the default schema requires, (3) create a second entity of type `principle` "
        "titled \"Validate at the boundary\" and relate the concept to it with a "
        "relationship type the default schema allows, and (4) export the mem as a "
        "`.mem` archive file. The commands should run as written.",
        False,
    ),
    (
        "N2",
        "My coding agent called memstead_update on an entity and got an error with "
        "code HASH_MISMATCH. What happened, is my data safe, and what is the correct "
        "sequence of calls to recover and land the edit? Are there shortcuts, and "
        "when are they a bad idea?",
        False,
    ),
    (
        "N3",
        "My searches over a mem feel too literal. How do I turn on semantic (vector) "
        "search in Memstead and choose the embedding model? If that is not how it "
        "works, tell me how search actually ranks results and what I should do "
        "instead to find things by meaning.",
        False,
    ),
    (
        "N4",
        "I want to keep a knowledge graph about my product inside the product's "
        "existing git repository, so it is versioned and reviewed together with the "
        "code. Which workspace shape should I choose, what do I give up compared "
        "with the other shape, and can I switch later?",
        False,
    ),
    (
        "N5",
        "What are the hard limits on a published Memstead mem archive (size, "
        "entries, entity count, file types, anything else), and why were they set "
        "that way? How big a mem is the system actually designed for?",
        False,
    ),
    ("Q0h", "", True),
    ("Q2h", "", True),
]

SIDES = ("mem", "src")
SIDE_LABEL = {"mem": "mem", "src": "source"}

WORD_CAP = "Answer in 200 to 350 words"


def question_text(key: str) -> str:
    """The text of a question; the small-model repeats resolve to their base."""
    base = key[:-1] if key.endswith("h") else key
    for k, text, _small in QUESTIONS:
        if k == base:
            return text
    raise KeyError(key)


# ---------------------------------------------------------------- stripping

# Parentheticals that name a citation: an entity id, a crate path, a Rust
# file, a function, struct or module name, or a source phrase.
CITE_PAREN_RE = re.compile(
    r"\s*\((?:(?:[^()]|\(\))*?)(?:"
    r"[a-z0-9-]+--[a-z0-9-]+"  # an entity id
    r"|crates/|\.rs\b|\bfn\s+\w+|\bstruct\s+\w+|\bimpl\s+\w+|\bmod\s+\w+|::\w+"
    r"|\bthe (?:code|source|mem|entity)\b|\bper the\b|\bsee\s+`"
    r")(?:[^()]|\(\))*\)"
)
# A parenthetical left holding only separators once its citations are gone.
EMPTY_PAREN_RE = re.compile(r"\s*\((?:\s|[,;:`]|\band\b)*\)")
# Inline code spans that are file paths or Rust items, and bare path tokens.
CODE_PATH_RE = re.compile(
    r"`(?:[\w./-]*(?:crates/|\.rs\b)[\w./:-]*|"
    r"[\w:]+::[\w:]+|fn\s+\w+|struct\s+\w+)`"
)
BARE_PATH_RE = re.compile(r"(?<![\w`])[\w.-]+(?:/[\w.-]+)*\.rs\b(?::\d+(?:-\d+)?)?")
def entity_id_re(mem_name: str) -> re.Pattern[str]:
    """Ids of the mem under test are citations; ids of any other mem (a
    scratch mem in a command sequence, say) are content and stay."""
    return re.compile(r"(?<![\w`-])" + re.escape(mem_name) + r"--[a-z0-9]+(?:-[a-z0-9]+)*(?![\w-])")

# Phrases that tell the grader which side wrote the answer.
LEAK_PHRASES: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\bper the (?:installed )?mem\b", re.I), "per the material"),
    (re.compile(r"\bfrom the (?:installed )?mem alone\b", re.I), "from the material alone"),
    (
        re.compile(
            r"\b(the|this) (?:installed )?mem (does|says|states|establishes|documents|"
            r"tells|leaves|names|records|lists|describes|carries|gives|notes|"
            r"mentions|covers|has|contains|shows|reports|explains|is silent)\b",
            re.I,
        ),
        r"\1 material \2",
    ),
    (re.compile(r"\bwhat the (?:installed )?mem does\b", re.I), "what the material does"),
    (re.compile(r"\bthe (?:installed )?mem'?s? entities\b", re.I), "the material"),
    (re.compile(r"\bthe (?:code|source|sources|material) I (?:read|examined|reviewed)\b", re.I), "the material"),
    (re.compile(r"\bthe (?:Rust )?(?:source code|source tree|codebase)\b", re.I), "the material"),
    (re.compile(r"\bin the (?:Rust )?source\b", re.I), "in the material"),
    (re.compile(r"\bfrom the (?:Rust )?source\b", re.I), "from the material"),
    (re.compile(r"\bthe Rust source\b", re.I), "the material"),
]
SPACE_BEFORE_PUNCT_RE = re.compile(r"\s+([,.;:)])")
DOUBLE_SPACE_RE = re.compile(r"[ \t]{2,}")


def strip_citations(text: str, mem_name: str = "engine") -> str:
    """Remove the traces that would tell a grader which reader wrote this."""
    out = CITE_PAREN_RE.sub("", text)
    out = CODE_PATH_RE.sub("", out)
    out = BARE_PATH_RE.sub("", out)
    out = entity_id_re(mem_name).sub("", out)
    out = EMPTY_PAREN_RE.sub("", out)
    for pattern, replacement in LEAK_PHRASES:
        out = pattern.sub(replacement, out)
    out = SPACE_BEFORE_PUNCT_RE.sub(r"\1", out)
    out = DOUBLE_SPACE_RE.sub(" ", out)
    return out.strip()


# ---------------------------------------------------------------- prompts

READER_COMMON = (
    "You are answering one question about Memstead for a developer who will "
    "act on your answer. Be concrete and honest: state what you could verify, "
    "hedge what you could not, and never invent a mechanism. {cap}. Do not "
    "mention where you looked, do not describe your method, and do not refer "
    "to your source of information at all; a reader must not be able to tell "
    "what you read. Return the answer text and nothing else, no preamble."
)


def reader_prompt(key: str, side: str, index: dict) -> str:
    text = question_text(key)
    cap = "Answer as a sequence of shell commands with one line of explanation each, under 350 words" if key == "N1" else WORD_CAP
    common = READER_COMMON.format(cap=cap)
    if side == "mem":
        ws = index["mem_workspace"]
        binary = index["memstead"]
        scope = (
            "Your only source of information is the mem named `engine` installed "
            f"in the workspace at `{ws}`. Read it through the memstead CLI and "
            f"nothing else: `{binary} --workspace {ws} --quiet overview`, "
            f"`{binary} --workspace {ws} --quiet search \"<terms>\" --mem engine`, "
            f"`{binary} --workspace {ws} --quiet entity <id>`, "
            f"`{binary} --workspace {ws} --quiet relations <id>`. "
            "Run no other command, read no file on disk, do not open the "
            "engine's source or documentation, and do not use the web. Cite the "
            "entity ids you relied on in parentheses after the sentences they "
            "support; the citations are stripped before grading."
        )
    else:
        src = index["source"]
        scope = (
            "Your only source of information is the Rust source of the engine "
            f"under `{src}/crates` (every `.rs` file, tests included as code). "
            "Read no markdown, no documentation, no changelog, run no binary, "
            "open no mem, and do not use the web. Cite the files and functions "
            "you relied on in parentheses after the sentences they support; the "
            "citations are stripped before grading."
        )
    return f"{common}\n\n{scope}\n\nQuestion:\n\n{text}\n"


GRADER_PROMPT = """You are grading two anonymous answers, A and B, to one question about Memstead. You do not know who wrote them and must not guess; ignore any stray hint of where an answer came from and judge content only.

Verify every checkable claim in both answers against the engine's Rust source under `{source}/crates` and, where a claim is about behaviour, by running the release binary `{memstead}` in a scratch workspace of your own under `{scratch}` (create it with `{memstead} init --name probe --schema default@1.3.0 {scratch}/probe` or `{memstead} quickstart {scratch}/probe`; never touch any other workspace or repository).{n1_rule}

Return ONLY a JSON object with exactly these keys:
- "winner": "A", "B" or "tie"
- "margin": 1, 2 or 3 (1 close, 3 decisive; 0 for a tie)
- "scores": {{"A": {{"correctness": 0-5, "completeness": 0-5, "usefulness": 0-5, "honesty": 0-5}}, "B": {{same}}}}
- "errors_A": a list of strings, each one false or unverifiable claim in A with what the source or the binary shows instead; empty if none
- "errors_B": the same for B
- "rationale": under 120 words

Correctness is truth against the source and the binary; completeness is whether the question's parts are all answered; usefulness is whether a developer could act on it; honesty is whether the answer hedges what it could not establish and invents nothing.

Question:

{question}

# Answer A

{answer_a}

# Answer B

{answer_b}
"""

N1_RULE = (
    " For this question, grade by execution: run each answer's command "
    "sequence literally, in order, in its own fresh empty directory under "
    f"the scratch path, with `PATH` carrying the release binary, and score "
    "correctness by what runs as written."
)


def grader_prompt(key: str, index: dict, pair_a: str, pair_b: str) -> str:
    n1_rule = N1_RULE if key == "N1" else ""
    return GRADER_PROMPT.format(
        source=index["source"],
        memstead=index["memstead"],
        scratch=str(Path(index["dir"]) / f"grader-{key}"),
        n1_rule=n1_rule,
        question=question_text(key),
        answer_a=pair_a,
        answer_b=pair_b,
    )


# ---------------------------------------------------------------- phases


def load_index(run_dir: Path) -> dict:
    index = json.loads((run_dir / "index.json").read_text(encoding="utf-8"))
    index["dir"] = str(run_dir)
    return index


def save_index(run_dir: Path, index: dict) -> None:
    index = {k: v for k, v in index.items() if k != "dir"}
    (run_dir / "index.json").write_text(json.dumps(index, indent=1) + "\n", encoding="utf-8")


def draw_order(keys: list[str], seed: int) -> dict[str, dict[str, str]]:
    rng = random.Random(seed)
    order = {}
    for key in keys:
        first = rng.choice(SIDES)
        second = "src" if first == "mem" else "mem"
        order[key] = {"A": first, "B": second}
    return order


def run_cli(argv: list[str], cwd: Path | None = None) -> str:
    proc = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"command failed ({proc.returncode}): {' '.join(argv)}\n{proc.stderr.strip()}")
    return proc.stdout


def cmd_prepare(args: argparse.Namespace) -> int:
    run_dir = Path(args.dir).resolve()
    if run_dir.exists() and any(run_dir.iterdir()):
        raise SystemExit(f"run directory is not empty: {run_dir}")
    run_dir.mkdir(parents=True, exist_ok=True)
    for sub in ("prompts", "answers", "pairs", "verdicts"):
        (run_dir / sub).mkdir()
    memstead = str(Path(args.memstead).resolve())
    source = str(Path(args.source).resolve())
    ws = run_dir / "mem-ws"
    run_cli([memstead, "init", "--name", "battery", "--schema", args.schema, str(ws)])
    install_target = args.mem if "/" in args.mem and not Path(args.mem).exists() else str(Path(args.mem).resolve())
    run_cli([memstead, "--workspace", str(ws), "--quiet", "install", install_target])
    keys = [k for k, _t, _s in QUESTIONS]
    index = {
        "created": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "seed": args.seed,
        "memstead": memstead,
        "memstead_version": run_cli([memstead, "--version"]).strip(),
        "source": source,
        "mem": install_target,
        "mem_name": args.mem_name,
        "mem_workspace": str(ws),
        "models": {"large": args.model, "small": args.small_model},
        "questions": {k: {"text": question_text(k), "small_model": small} for k, _t, small in QUESTIONS},
        "order": draw_order(keys, args.seed),
        "runs": {},
    }
    index["dir"] = str(run_dir)
    for key in keys:
        for side in SIDES:
            (run_dir / "prompts" / f"{key}-{side}.md").write_text(reader_prompt(key, side, index), encoding="utf-8")
    save_index(run_dir, index)
    print(f"prepared {run_dir}: {len(keys)} questions, {len(keys) * 2} reader prompts, mem installed at {ws}")
    return 0


def cmd_pair(args: argparse.Namespace) -> int:
    run_dir = Path(args.dir).resolve()
    index = load_index(run_dir)
    written = 0
    missing = []
    for key in index["questions"]:
        answers = {}
        for side in SIDES:
            path = run_dir / "answers" / f"{key}-{side}.md"
            if not path.exists():
                missing.append(path.name)
                break
            answers[side] = strip_citations(path.read_text(encoding="utf-8"), index.get("mem_name", "engine"))
        if len(answers) < 2:
            continue
        order = index["order"][key]
        pair_a, pair_b = answers[order["A"]], answers[order["B"]]
        body = f"# Question\n\n{question_text(key)}\n\n# Answer A\n\n{pair_a}\n\n# Answer B\n\n{pair_b}\n"
        (run_dir / "pairs" / f"{key}.md").write_text(body, encoding="utf-8")
        (run_dir / "prompts" / f"grader-{key}.md").write_text(grader_prompt(key, index, pair_a, pair_b), encoding="utf-8")
        written += 1
    print(f"paired {written} question(s)" + (f"; missing answers: {', '.join(missing)}" if missing else ""))
    return 0 if not missing else 1


def side_of(order: dict[str, str], letter: str) -> str:
    return order[letter]


def tally(index: dict, verdicts: dict[str, dict]) -> dict:
    rows = []
    wins = {"mem": 0, "src": 0, "tie": 0}
    margins = {"mem": 0, "src": 0}
    score_sum = {s: {"correctness": 0, "completeness": 0, "usefulness": 0, "honesty": 0} for s in SIDES}
    errors = {"mem": [], "src": []}
    by_model = {"large": {"mem": 0, "src": 0, "tie": 0}, "small": {"mem": 0, "src": 0, "tie": 0}}
    for key, q in index["questions"].items():
        v = verdicts.get(key)
        if v is None:
            rows.append({"question": key, "graded": False})
            continue
        order = index["order"][key]
        winner_letter = v["winner"]
        winner = "tie" if winner_letter == "tie" else side_of(order, winner_letter)
        margin = int(v.get("margin", 0)) if winner != "tie" else 0
        wins[winner] += 1
        model_bucket = "small" if q["small_model"] else "large"
        by_model[model_bucket][winner] += 1
        if winner != "tie":
            margins[winner] += margin
        scores = {}
        for letter in ("A", "B"):
            side = side_of(order, letter)
            scores[side] = v["scores"][letter]
            for axis, val in v["scores"][letter].items():
                score_sum[side][axis] += int(val)
            for err in v.get(f"errors_{letter}", []):
                errors[side].append({"question": key, "error": err})
        rows.append(
            {
                "question": key,
                "graded": True,
                "winner": winner,
                "margin": margin,
                "order": order,
                "scores": scores,
                "rationale": v.get("rationale", ""),
            }
        )
    graded = sum(1 for r in rows if r["graded"])
    return {
        "graded": graded,
        "wins": wins,
        "margins": margins,
        "scores_total": score_sum,
        "by_model": by_model,
        "errors": errors,
        "rows": rows,
    }


def render_result(index: dict, result: dict) -> str:
    lines = [
        f"# Blind battery result ({index['created'][:10]})",
        "",
        f"Mem: `{index['mem']}`; engine `{index.get('memstead_version', '?')}`; source `{index['source']}`; "
        f"models {index['models']['large']} (large) and {index['models']['small']} (small); seed {index['seed']}.",
        "",
        f"Graded {result['graded']} of {len(index['questions'])} pairs. "
        f"Wins: mem {result['wins']['mem']}, source {result['wins']['src']}, tie {result['wins']['tie']}. "
        f"Margin sum: mem {result['margins']['mem']}, source {result['margins']['src']}. "
        f"Large model: mem {result['by_model']['large']['mem']}, source {result['by_model']['large']['src']}, "
        f"tie {result['by_model']['large']['tie']}; small model: mem {result['by_model']['small']['mem']}, "
        f"source {result['by_model']['small']['src']}, tie {result['by_model']['small']['tie']}.",
        "",
        "| Question | Winner | Margin | Mem C/Co/U/H | Source C/Co/U/H |",
        "|---|---|---:|---|---|",
    ]
    for r in result["rows"]:
        if not r["graded"]:
            lines.append(f"| {r['question']} | not graded | | | |")
            continue

        def fmt(s: dict) -> str:
            return "/".join(str(s[a]) for a in ("correctness", "completeness", "usefulness", "honesty"))

        lines.append(
            f"| {r['question']} | {SIDE_LABEL.get(r['winner'], r['winner'])} | {r['margin']} | "
            f"{fmt(r['scores']['mem'])} | {fmt(r['scores']['src'])} |"
        )
    lines += ["", "Score totals (correctness, completeness, usefulness, honesty):", ""]
    for side in SIDES:
        s = result["scores_total"][side]
        lines.append(
            f"- {SIDE_LABEL[side]}: {s['correctness']}, {s['completeness']}, {s['usefulness']}, {s['honesty']}"
        )
    for side in SIDES:
        lines += ["", f"## Errors the graders found in the {SIDE_LABEL[side]} answers", ""]
        errs = result["errors"][side]
        if not errs:
            lines.append("- none")
        for e in errs:
            lines.append(f"- {e['question']}: {e['error']}")
    lines += ["", "## Rationales", ""]
    for r in result["rows"]:
        if r["graded"]:
            lines.append(f"- {r['question']} (A = {SIDE_LABEL[r['order']['A']]}, B = {SIDE_LABEL[r['order']['B']]}): {r['rationale']}")
    return "\n".join(lines) + "\n"


def cmd_tally(args: argparse.Namespace) -> int:
    run_dir = Path(args.dir).resolve()
    index = load_index(run_dir)
    verdicts = {}
    for key in index["questions"]:
        path = run_dir / "verdicts" / f"{key}.json"
        if path.exists():
            verdicts[key] = json.loads(path.read_text(encoding="utf-8"))
    result = tally(index, verdicts)
    (run_dir / "result.json").write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8")
    (run_dir / "result.md").write_text(render_result(index, result), encoding="utf-8")
    print(
        f"tallied {result['graded']} pair(s): mem {result['wins']['mem']}, source {result['wins']['src']}, "
        f"tie {result['wins']['tie']} -> {run_dir / 'result.md'}"
    )
    return 0 if result["graded"] == len(index["questions"]) else 1


# ---------------------------------------------------------------- driver

READER_TOOLS = {
    "mem": lambda index: [f"Bash({index['memstead']} *)", f"Bash({index['memstead']}:*)"],
    "src": lambda index: ["Read", "Grep", "Glob"],
}
GRADER_TOOLS = ["Read", "Grep", "Glob", "Bash", "Write"]


def claude_call(prompt: str, model: str, tools: list[str], cwd: Path, add_dirs: list[str], executable: str) -> tuple[str, dict]:
    """One ``claude -p`` call; returns the result text and the usage record."""
    argv = [
        executable,
        "-p",
        prompt,
        "--model",
        model,
        "--output-format",
        "json",
        "--permission-mode",
        "dontAsk",
        "--strict-mcp-config",
        "--allowedTools",
        ",".join(tools),
    ]
    for d in add_dirs:
        argv += ["--add-dir", d]
    proc = subprocess.run(argv, cwd=cwd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"claude exited {proc.returncode}: {proc.stderr.strip()[:2000]}")
    payload = json.loads(proc.stdout)
    usage = {
        "model": model,
        "turns": payload.get("num_turns"),
        "duration_ms": payload.get("duration_ms"),
        "cost_usd": payload.get("total_cost_usd"),
    }
    return str(payload.get("result", "")), usage


def extract_json(text: str) -> dict:
    start = text.find("{")
    end = text.rfind("}")
    if start < 0 or end < 0:
        raise ValueError("no JSON object in the grader's reply")
    return json.loads(text[start : end + 1])


def cmd_run(args: argparse.Namespace) -> int:
    run_dir = Path(args.dir).resolve()
    index = load_index(run_dir)
    only = set(args.only.split(",")) if args.only else None
    keys = [k for k in index["questions"] if not only or k in only]
    for key in keys:
        q = index["questions"][key]
        model = index["models"]["small"] if q["small_model"] else index["models"]["large"]
        if args.phase == "readers":
            for side in SIDES:
                out = run_dir / "answers" / f"{key}-{side}.md"
                if out.exists() and not args.redo:
                    continue
                prompt = (run_dir / "prompts" / f"{key}-{side}.md").read_text(encoding="utf-8")
                cwd = Path(index["mem_workspace"]) if side == "mem" else Path(index["source"]) / "crates"
                add_dirs = [str(cwd)]
                text, usage = claude_call(prompt, model, READER_TOOLS[side](index), cwd, add_dirs, args.claude)
                out.write_text(text.strip() + "\n", encoding="utf-8")
                index["runs"][f"{key}-{side}"] = usage
                save_index(run_dir, index)
                print(f"answered {key}-{side} ({usage['turns']} turns)")
        else:
            out = run_dir / "verdicts" / f"{key}.json"
            if out.exists() and not args.redo:
                continue
            prompt_path = run_dir / "prompts" / f"grader-{key}.md"
            if not prompt_path.exists():
                print(f"no grader prompt for {key}; run `pair` first")
                continue
            scratch = run_dir / f"grader-{key}"
            scratch.mkdir(exist_ok=True)
            grader_model = index["models"]["large"]
            text, usage = claude_call(
                prompt_path.read_text(encoding="utf-8"),
                grader_model,
                GRADER_TOOLS,
                scratch,
                [str(scratch), str(Path(index["source"]) / "crates")],
                args.claude,
            )
            verdict = extract_json(text)
            out.write_text(json.dumps(verdict, indent=1) + "\n", encoding="utf-8")
            index["runs"][f"{key}-grader"] = usage
            save_index(run_dir, index)
            print(f"graded {key}: winner {verdict.get('winner')} margin {verdict.get('margin')}")
    return 0


# ---------------------------------------------------------------- record

RECORD_DIRS = ("prompts", "answers", "pairs", "verdicts")
RECORD_FILES = ("index.json", "result.json", "result.md")


def path_placeholders(index: dict) -> list[tuple[str, str]]:
    """The machine paths a run carries and the placeholders that replace
    them in the record, longest path first so a prefix never wins early."""
    pairs = [
        (index.get("mem_workspace", ""), "<run>/mem-ws"),
        (index.get("dir", ""), "<run>"),
        (index.get("memstead", ""), "<memstead>"),
        (index.get("source", ""), "<source>"),
    ]
    mem = index.get("mem", "")
    if mem.startswith("/"):
        pairs.append((mem, "<mem>"))
    pairs = [(a, b) for a, b in pairs if a]
    pairs.sort(key=lambda ab: -len(ab[0]))
    return pairs


def replace_paths(text: str, pairs: list[tuple[str, str]]) -> str:
    for path, placeholder in pairs:
        text = text.replace(path, placeholder)
    return text


def write_record(run_dir: Path, out_dir: Path, index: dict) -> int:
    pairs = path_placeholders(index)
    out_dir.mkdir(parents=True, exist_ok=True)
    written = 0
    for name in RECORD_FILES:
        src = run_dir / name
        if src.exists():
            (out_dir / name).write_text(replace_paths(src.read_text(encoding="utf-8"), pairs), encoding="utf-8")
            written += 1
    for sub in RECORD_DIRS:
        src_dir = run_dir / sub
        if not src_dir.is_dir():
            continue
        (out_dir / sub).mkdir(exist_ok=True)
        for src in sorted(src_dir.iterdir()):
            if src.is_file():
                (out_dir / sub / src.name).write_text(replace_paths(src.read_text(encoding="utf-8"), pairs), encoding="utf-8")
                written += 1
    return written


def cmd_record(args: argparse.Namespace) -> int:
    run_dir = Path(args.dir).resolve()
    index = load_index(run_dir)
    out_dir = Path(args.out).resolve()
    written = write_record(run_dir, out_dir, index)
    leftover = []
    for path in out_dir.rglob("*"):
        if path.is_file():
            text = path.read_text(encoding="utf-8")
            for machine_path, _ in path_placeholders(index):
                if machine_path in text:
                    leftover.append(f"{path.name}: {machine_path}")
    if leftover:
        print("record still carries a machine path: " + "; ".join(leftover))
        return 1
    print(f"recorded {written} file(s) into {out_dir}")
    return 0


# ---------------------------------------------------------------- self-test


def self_test() -> int:
    failures = []

    def check(name: str, cond: bool, detail: str = "") -> None:
        if not cond:
            failures.append(f"{name}: {detail}")

    # Stripping removes citations and leak phrases, keeps the claim.
    s = strip_citations(
        "The engine refuses with HASH_MISMATCH (engine--update-mutation). "
        "The check lives in `crates/memstead-base/src/engine/mutation/update.rs` "
        "(fn apply_update in crates/memstead-base/src/ops/mod.rs). Per the mem, "
        "folder mems are probed too; the code I read agrees. See engine--anchor-primitive."
    )
    check("strip-entity", "engine--" not in s, s)
    check("strip-path", ".rs" not in s and "crates/" not in s, s)
    s5 = strip_citations("It must be adopted instead (memstead-base/src/engine/error.rs, `details()`; server.rs, `HashMismatch` mapping). Fine (engine--x, engine--y)!")
    check("strip-nested-paren", "details()" not in s5 and "HashMismatch" not in s5 and s5 == "It must be adopted instead. Fine!", s5)
    s4 = strip_citations("The check runs first memstead-base/src/engine/mutation/update.rs:409-418 and then commits.")
    check("strip-bare-path", ".rs" not in s4 and "409" not in s4 and "then commits" in s4, s4)
    check("strip-leak", "per the mem" not in s.lower() and "code I read" not in s, s)
    check("strip-keeps", "HASH_MISMATCH" in s and "folder mems are probed too" in s, s)
    s2 = strip_citations("The installed mem does not document the flags; the mem says so itself.")
    check("strip-mem-verb", "installed mem" not in s2 and "the mem says" not in s2, s2)
    s6 = strip_citations("memstead relate notes--optimistic-locking USES notes--validate-at-the-boundary (engine--relate-mutation)")
    check("strip-keeps-other-mem-ids", s6 == "memstead relate notes--optimistic-locking USES notes--validate-at-the-boundary", s6)
    s3 = strip_citations("Install a mem with `memstead install`; the mem then loads read-only.")
    check("strip-keeps-mem-noun", "Install a mem" in s3 and "memstead install" in s3, s3)

    # The order draw is seeded and covers both sides.
    keys = [k for k, _t, _s in QUESTIONS]
    o1, o2 = draw_order(keys, 7), draw_order(keys, 7)
    check("order-seeded", o1 == o2)
    check("order-both", {o["A"] for o in o1.values()} == {"mem", "src"})
    check("order-complement", all({o["A"], o["B"]} == {"mem", "src"} for o in o1.values()))

    # Tally maps letters back to sides and sums what the verdicts say.
    index = {
        "questions": {"Q0": {"small_model": False}, "Q0h": {"small_model": True}, "N1": {"small_model": False}},
        "order": {"Q0": {"A": "src", "B": "mem"}, "Q0h": {"A": "mem", "B": "src"}, "N1": {"A": "mem", "B": "src"}},
    }
    sc = lambda c, co, u, h: {"correctness": c, "completeness": co, "usefulness": u, "honesty": h}  # noqa: E731
    verdicts = {
        "Q0": {"winner": "B", "margin": 2, "scores": {"A": sc(3, 3, 3, 3), "B": sc(5, 4, 4, 5)}, "errors_A": ["a"], "errors_B": [], "rationale": "r"},
        "Q0h": {"winner": "B", "margin": 1, "scores": {"A": sc(2, 2, 2, 2), "B": sc(4, 4, 4, 4)}, "errors_A": ["b", "c"], "errors_B": ["d"], "rationale": "r"},
    }
    r = tally(index, verdicts)
    check("tally-graded", r["graded"] == 2, str(r["graded"]))
    check("tally-wins", r["wins"] == {"mem": 1, "src": 1, "tie": 0}, str(r["wins"]))
    check("tally-margins", r["margins"] == {"mem": 2, "src": 1}, str(r["margins"]))
    check("tally-errors", [e["error"] for e in r["errors"]["src"]] == ["a", "d"], str(r["errors"]))
    check("tally-by-model", r["by_model"]["small"] == {"mem": 0, "src": 1, "tie": 0}, str(r["by_model"]))
    check("tally-scores", r["scores_total"]["mem"]["correctness"] == 7, str(r["scores_total"]))
    check("tally-ungraded", r["rows"][2] == {"question": "N1", "graded": False})
    tie = tally(index, {"N1": {"winner": "tie", "margin": 0, "scores": {"A": sc(3, 3, 3, 3), "B": sc(3, 3, 3, 3)}}})
    check("tally-tie", tie["wins"]["tie"] == 1 and tie["margins"] == {"mem": 0, "src": 0})
    index["created"] = "2026-09-06T00:00:00Z"
    index["mem"] = "x.mem"
    index["source"] = "."
    index["models"] = {"large": "l", "small": "s"}
    index["seed"] = 1
    md = render_result(index, r)
    check("render", "| Q0 | mem | 2 |" in md and "not graded" in md, md)

    # Prompts name the scopes and the questions resolve to their base text.
    idx = {"mem_workspace": "/w", "memstead": "/bin/memstead", "source": "/s", "dir": "/d"}
    check("prompt-mem", "--workspace /w" in reader_prompt("Q0h", "mem", idx))
    check("prompt-src", "/s/crates" in reader_prompt("N1", "src", idx))
    check("question-h", question_text("Q2h") == question_text("Q2"))
    check("grader-n1", "grade by execution" in grader_prompt("N1", idx, "a", "b"))
    check("grader-other", "grade by execution" not in grader_prompt("Q3", idx, "a", "b"))
    check("extract-json", extract_json('noise {"winner": "A"} trailing') == {"winner": "A"})

    # The record replaces every machine path, longest first.
    ridx = {"dir": "/tmp/r", "mem_workspace": "/tmp/r/mem-ws", "memstead": "/opt/bin/memstead", "source": "/src/public", "mem": "/tmp/engine.mem"}
    rec = replace_paths("run /opt/bin/memstead --workspace /tmp/r/mem-ws in /tmp/r over /src/public/crates with /tmp/engine.mem", path_placeholders(ridx))
    check("record-paths", rec == "run <memstead> --workspace <run>/mem-ws in <run> over <source>/crates with <mem>", rec)
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        run = Path(tmp) / "run"
        (run / "prompts").mkdir(parents=True)
        (run / "mem-ws").mkdir()
        (run / "index.json").write_text(json.dumps({"dir": str(run), "memstead": "/opt/bin/memstead", "source": "/src/public", "mem": "x/y", "mem_workspace": str(run / "mem-ws")}), encoding="utf-8")
        (run / "prompts" / "Q0-mem.md").write_text(f"use /opt/bin/memstead --workspace {run / 'mem-ws'}", encoding="utf-8")
        (run / "mem-ws" / "secret.md").write_text("never copied", encoding="utf-8")
        out = Path(tmp) / "out"
        idx = json.loads((run / "index.json").read_text(encoding="utf-8"))
        idx["dir"] = str(run)
        n = write_record(run, out, idx)
        check("record-count", n == 2, str(n))
        check("record-prompt", (out / "prompts" / "Q0-mem.md").read_text(encoding="utf-8") == "use <memstead> --workspace <run>/mem-ws")
        check("record-index", '"<run>"' in (out / "index.json").read_text(encoding="utf-8"))
        check("record-skips-workspace", not (out / "mem-ws").exists())

    if failures:
        for f in failures:
            print(f"FAIL {f}")
        return 1
    print("blind_battery self-test: ok")
    return 0


# ---------------------------------------------------------------- main


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--self-test", action="store_true", help="exercise the stripping, order and tally on fixtures")
    sub = parser.add_subparsers(dest="cmd")
    p = sub.add_parser("prepare", help="write questions, order, prompts and the reader workspace")
    p.add_argument("--dir", required=True)
    p.add_argument("--memstead", required=True, help="the release binary the readers and graders run")
    p.add_argument("--mem", required=True, help="a .mem archive path or a registry <scope>/<name> to install")
    p.add_argument("--source", required=True, help="the open engine repository root (crates/ beneath it)")
    p.add_argument("--schema", default="default@1.3.0", help="schema pin of the reader workspace's own mem")
    p.add_argument("--mem-name", default="engine", help="name of the installed mem under test; its entity ids are the citations the pairing strips")
    p.add_argument("--seed", type=int, default=int(dt.date.today().strftime("%Y%m%d")))
    p.add_argument("--model", default="fable", help="model of the large-model readers and every grader")
    p.add_argument("--small-model", default="haiku", help="model of the two small-model pairs")
    p.set_defaults(func=cmd_prepare)
    p = sub.add_parser("run", help="drive the readers or the graders through claude -p")
    p.add_argument("--dir", required=True)
    p.add_argument("--phase", choices=["readers", "graders"], required=True)
    p.add_argument("--only", help="comma-separated question keys")
    p.add_argument("--redo", action="store_true", help="overwrite existing answers or verdicts")
    p.add_argument("--claude", default="claude", help="the claude executable")
    p.set_defaults(func=cmd_run)
    p = sub.add_parser("pair", help="strip citations, write the pairs and the grader prompts")
    p.add_argument("--dir", required=True)
    p.set_defaults(func=cmd_pair)
    p = sub.add_parser("tally", help="map verdicts back to mem/source and write the result")
    p.add_argument("--dir", required=True)
    p.set_defaults(func=cmd_tally)
    p = sub.add_parser("record", help="copy the run into a committable record with machine paths replaced")
    p.add_argument("--dir", required=True)
    p.add_argument("--out", required=True, help="the record directory, e.g. docs/proof/blind-battery/<date>")
    p.set_defaults(func=cmd_record)
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    if not args.cmd:
        parser.print_help()
        return 2
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
