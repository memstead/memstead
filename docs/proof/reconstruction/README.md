# Reconstruction measurement — one observed run, 2026-07-03

This folder is the source for the claim on Memstead's public surfaces:

> A fresh agent rebuilt the full working picture of a domain from its mem
> using roughly **one-thirteenth** of the tokens the conversations that
> authored that knowledge had consumed.

It is **one observed measurement, not a benchmark**. Everything needed to
audit and rerun it is in this folder: the measured mem, the counting script,
the task prompt, the fresh agent's complete reconstruction transcript, the
brief it produced, and the per-session token accounting for all 22 authoring
conversations. Only the 22 raw authoring transcripts themselves are withheld
— they embed bulk fetched third-party web content we do not republish; their
sha256 hashes and per-session counts are in `authoring-sessions.json`, so the
denominator stays auditable without them. Local filesystem paths in the
published records are screened (the home directory is shown as `~`).

## What was measured

Two token counts, produced by the same script over the same kind of record
(Claude Code session transcripts, JSONL), with the same tokenizer:

| Side | What it is | Content tokens |
|---|---|---:|
| Authoring (denominator) | The 22 agent conversations that researched and authored the `landing-page-design` mem (2026-06-23/24) | **852,773** |
| Reconstruction (numerator) | One fresh-context agent conversation (2026-07-03) that read the finished mem through the `memstead` CLI and demonstrated understanding | **65,161** |

**Ratio: 65,161 / 852,773 ≈ 1/13.1.**

Secondary observation: the mem's full serialized content (all 287 entity
markdown files) is **108,034 tokens** — so even an agent that read *every*
entity outright would consume about **1/7.9** of the authoring cost. The
reconstruction agent got to a defensible working picture cheaper than that
by orienting first (communities, type lists) and reading selectively.

"Content tokens" means: every user turn (including tool results fed to the
model), every assistant turn (text and tool-call arguments), tokenized with
tiktoken's `o200k_base` — see `count_tokens.py`. The same script and
tokenizer are applied to both sides, so the *ratio* is robust to the
tokenizer choice to first order (it is not the Anthropic tokenizer).

For completeness, the API-level accounting Claude Code recorded (input +
cache-write + cache-read + output over every call) was 37,594,207 tokens
for the authoring sessions vs 998,698 for the reconstruction run (≈ 1/37.6).
We do **not** headline that number: cache mechanics make it an accounting of
API traffic, not of conversation size.

## The corpus

`landing-page-design` is a curated domain-expertise mem: 287 entities on
conversion-oriented landing-page design (19 principles, 88 practices,
46 pitfalls, 18 caveats, 28 examples, 26 concepts, 62 cited sources), built
with Memstead's ingest pipeline over one overnight run of web research on
2026-06-23/24. A copy is included here as `landing-page-design-0.1.0.mem`
so the reconstruction side is rerunnable byte-for-byte.

## Files

| File | What it is |
|---|---|
| `count_tokens.py` | The counting script (Python 3, needs `tiktoken`). Applied identically to both sides. |
| `landing-page-design-0.1.0.mem` | The measured mem, exported as a portable archive (`memstead install ./landing-page-design-0.1.0.mem`). |
| `reconstruction-transcript.jsonl` | The fresh agent's complete reconstruction session (JSONL), the numerator. Local paths screened (home directory shown as `~`). |
| `reconstruction-brief.md` | The domain brief the reconstruction agent produced — the demonstrated understanding. |
| `authoring-sessions.json` | Per-session accounting for all 22 authoring conversations (sha256, time span, turn count, content tokens, API usage) — the denominator. |

Withheld: only the 22 raw authoring transcripts themselves — they embed bulk
fetched third-party web content we do not republish. Their sha256 hashes and
per-session token counts are in `authoring-sessions.json`, so the denominator
is auditable without them.

## How the run worked

1. **Denominator.** The mem was authored by a `/loop`-driven ingest skill;
   each iteration ran as a forked Claude Code execution whose transcript
   records which mem it targeted (`round-robin picked:
   landing-page-design-graph`). All 22 transcripts that targeted this mem
   were identified (21 in the overnight loop session, 1 in the setup
   session — the mem's entire git history, 458 commits, falls inside their
   time windows) and counted with `count_tokens.py`.
2. **Numerator.** A fresh-context agent was spawned with the task prompt
   quoted below under "Rerun it": read the mem through the `memstead` CLI
   only (overview / list / entity / relations / context — no files, no git,
   no web, no training-knowledge shortcuts: every claim in the deliverable
   must cite an entity it read), then write a domain brief. It read the
   cluster overview, full type listings, and ~30 entities in full, and
   produced the brief. Its transcript was counted with the same script:
   65,161 content tokens (52,895 of that is the task prompt plus CLI output
   the agent read; 12,266 is the agent's own reasoning, tool calls, and the
   brief).
3. **Ratio.** 852,773 / 65,161 ≈ 13.1.

## Rerun it

The denominator is a fixed historical record — its per-session accounting
(sha256 hashes, time spans, per-file counts) is in `authoring-sessions.json`,
but it cannot be re-produced. The reconstruction side is fully rerunnable:

```sh
# 1. Get the engine (https://memstead.com), then mount the measured mem
#    into a fresh workspace. (Local-archive installs currently need the
#    embedded schema installed first — that ordering quirk is ours, noted
#    here so the recipe works as written.)
mkdir lpd-workspace && cd lpd-workspace
memstead mem-repo init
memstead workspace allow-create --schema '*' notes
memstead mem init notes --schema default@1.0.0
unzip landing-page-design-0.1.0.mem '.memstead/schema/*' -d unpack
memstead schema install ./unpack/.memstead/schema
memstead install ./landing-page-design-0.1.0.mem
memstead stats     # expect: 287 nodes, 681 edges

# 2. Give a fresh agent (no prior context) the task prompt below,
#    scoped to CLI reads only.

# 3. Count its transcript
pip install tiktoken
python3 count_tokens.py <fresh-agent-transcript>.jsonl
```

The task prompt, verbatim except two local paths replaced by placeholders
(`<workspace>` — the directory prepared in step 1; `<output-path>` — where
the agent should write its brief):

> You are a fresh-context agent taking part in a measured reconstruction run. You have NO prior briefing about the knowledge you are about to acquire, and you must acquire it ONLY through the Memstead CLI reads described below — that is the point of the measurement.
>
> Setting: the directory `<workspace>` is a Memstead workspace. One of its mems is named `landing-page-design`. Your job is to build a genuine working understanding of the domain expertise that mem holds, and then demonstrate that understanding in writing.
>
> Hard rules:
> - Read ONLY via the `memstead` CLI (it is on PATH; run it with cwd `<workspace>`). Allowed subcommands: overview, stats, search, list, entity, relations, context, type, health — plus `--help` on any of them. Scope your reads to the mem `landing-page-design` where the command supports it.
> - Do NOT read files under mem-repo/ or anywhere else on disk, do NOT run git, do NOT use WebSearch/WebFetch, do NOT spawn subagents. No mutations of any kind (no create/update/relate/delete/rename).
> - Work from what the graph tells you, not from your training knowledge. Every substantive claim in your deliverable must be traceable to an entity you actually read — cite entity IDs. Prefer specifics that could only come from the mem: named studies and sources, concrete figures, dates, scoped conditions.
>
> Approach: orient first (communities/summaries), then read selectively — the entities you need to genuinely cover the whole domain picture as this mem presents it, across all its communities. Read as much as you need for real understanding; don't skim to save tokens and don't exhaustively dump every entity either. Behave like an agent that actually needs this expertise for upcoming work.
>
> Deliverable: write a working domain brief to `<output-path>` containing:
> 1. What this domain is and how the mem organizes it (its communities/clusters).
> 2. The core principles (the "why" layer) — each with entity ID citations.
> 3. For each major cluster: the key practices and the key pitfalls, with the concrete specifics the mem records (figures, named sources, years, applicability conditions), citing entity IDs.
> 4. Where the knowledge comes from: the kinds of sources the mem cites, with several named examples.
> 5. Three concrete, non-obvious recommendations you could now defend in a design review, each traced to entities.
>
> Then reply with a 5-line summary of what you read (roughly how many entities, which commands) and confirm the brief is written.

A rerun will not reproduce 65,161 exactly — agents read differently — but
it should land in the same order of magnitude, and that is the claim.

## Caveats — read before citing

- **One observed run.** The reconstruction was executed exactly once, not
  selected from multiple attempts. No variance estimate exists.
- **Not a benchmark.** One mem, one topic, first-party. No comparison with
  any other tool is made or implied.
- **"Understanding" is demonstrated, not scored.** The evidence is the
  brief itself (`reconstruction-brief.md` in this folder) — judge it. There
  is no formal eval; the task required every
  claim to be entity-cited and to carry specifics (named studies, figures,
  dates) that exist only in the mem, which bounds how much the agent could
  fake from training knowledge, but does not eliminate topic familiarity.
- **The denominator is conservative.** It counts only the 22 forked
  authoring conversations. The loop-driver session that scheduled them, the
  workspace-setup sessions, and the schema-authoring work are excluded —
  including them would only raise the denominator (and the ratio).
- **The numerator is conservative too.** It includes the task prompt and
  the agent's own output, not just the graph reads.
- **Authoring cost includes research.** The authoring sessions searched and
  read the open web to *acquire* the knowledge; the reconstruction agent
  only had to *recover* it from the mem. That asymmetry is the point being
  measured — a mem substitutes for re-deriving the knowledge — but do not
  read the ratio as "Memstead compresses text 13×".
- **The archive differs from the measured state by three removed edges.**
  At measurement time the mem carried three cross-mem relationships into a
  sibling mem that is not included here; they were removed through the
  engine after the run (684 → 681 edges) so the archive is self-contained
  and passes strict install validation. Entity content is otherwise
  identical; the 108,034 serialized-token figure was measured pre-removal.
- **What is published, what is withheld.** The reconstruction transcript,
  the brief, and the per-session authoring accounting are in this folder
  (local filesystem paths screened, home directory shown as `~`). Only the 22 raw
  authoring transcripts are withheld — they embed bulk fetched third-party
  web content we do not republish; `authoring-sessions.json` carries their
  sha256 hashes and token counts so the denominator stays auditable.
- **Prior number, replaced.** An earlier informal observation from April
  2026 reported roughly the same ~13:1 ratio on a hand-built, pre-engine
  graph. That experiment was retired from any public citation on
  2026-07-02; this measurement replaces it. The near-identical ratio is a
  coincidence of this single run, not a constant of the system.
