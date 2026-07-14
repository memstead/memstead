# Source-repo record — divergence campaign

Pre-registered. Immutable after the first real run except by a dated amendment note (see [README.md](README.md)).

## The repo

| Field | Value |
|---|---|
| Repository | `karalang/kara` — the Kāra programming language and compiler |
| Clone URL | `https://github.com/karalang/kara` |
| Pinned snapshot (HEAD) | `349066162dbfef551861d21c44c63a53260f79b5` (2026-07-14) |
| Root commit | `47460e75ba838cc632a2084077ea88cdbe68769e` (2026-05-04) |
| License | `Apache-2.0 OR MIT` (dual; `LICENSE-APACHE`, `LICENSE-MIT`) |
| Stars / forks at selection | 38 / 0 |
| Domain | Programming language + compiler (lexer, parser, type/effect system, LLVM codegen) |

The campaign consumes the repo at the pinned HEAD only. Slice boundaries are in [slices.json](slices.json).

## Selection is a filter, not a hope

The six criteria below are the pre-registered decision. A candidate that fails **any** one is rejected regardless of how well it scores on the others (acceptance-criterion-2 refusal complement). Each criterion carries the evidence gathered against the live repo at the pinned snapshot.

### 1. The sliced history predominantly postdates the pinned model's training cutoff

**Pass — entirely, not merely predominantly.** The pinned writer/reader/judge model is `claude-opus-4-8` (knowledge cutoff January 2026; see [models.json](models.json)). The repo's **root commit is 2026-05-04** and HEAD is 2026-07-14 — every one of the 3,685 commits in every slice postdates the cutoff. The model cannot have seen this repository in training. This is the strongest possible satisfaction of the criterion and the reason a 2026-created repo was preferred over an older repo sliced to a recent window.

### 2. The history contains the required structural events, as the selection filter

**Pass.** Verified against the pinned snapshot; each event is pinned by commit SHA and assigned to its slice in [slices.json](slices.json) under `structural_events`:

- **Rename** — file rename `docs/demo_ideas.md → docs/dogfooding.md` (git rename-detection R086, commit `523661e1`, slice 5); plus a symbol rename (`930adcdb`, "continue-label machine rename", slice 9).
- **Revert** — a genuine `git revert`: `948d5527` reverts `af97e03e` (`fix(codegen): rc-inc nested bare-shared field … render UAF`), slice 9.
- **Supersession / deprecation** — the deprecation surface itself lands in `0cb76707` (`#[unstable]/#[deprecated]` lint, slice 4); a concrete supersession in `f2df625c` (the paired component form is removed, superseded by the embedded-WIT default `ad2ef651`, slice 5). The CHANGELOG's headline "Complete language redesign — Replaced the original `fn`/`flow`/`record`/`->` pipeline design with a Rust-inspired systems language" records a whole-language supersession at the repo's origin.

### 3. Ground truth for the query battery is mechanically derivable from commits/changelog

**Pass — unusually well.** The repo maintains a structured **bug ledger** at `docs/bug-ledger.jsonl` (450 records at HEAD), one JSON object per line with fields `id`, `date`, `source`, `surface` (the affected compiler surface — `interp`, `codegen`, `typecheck`, `parser`, …), `class`, `severity`, `status` (`open` / `fixed` / `partial-fixed` / `not-reproduced` / `invalid`), `fix` (fixing commit SHA), `title`, `tracker`. At HEAD: 7 `open`, 439 `fixed`, plus a handful of other statuses. This is a mechanically-parseable status/incident database that supplies unambiguous ground truth for three of the four query classes directly — status filters (which bugs are open), aggregation (how many bugs touch a given surface), currency/supersession (the fixing commit for a bug). The CHANGELOG (`CHANGELOG.md`, Keep-a-Changelog format) and the git log supply the rest. No judgment call is needed to derive a ground-truth answer.

### 4. The license permits redistributing the embedded excerpts

**Pass.** Dual `Apache-2.0 OR MIT`. Both permit redistribution of source excerpts with attribution; the package embeds only small slice excerpts and cites the repo and commit SHA.

### 5. Domain is not agent-memory/knowledge-tooling, and vocabulary is disjoint from both arms' tell lists

**Pass, with one disclosed caveat.** The domain is a compiler — its natural vocabulary (lexer, parser, AST, type inference, effect system, ownership/borrow, LLVM-IR, codegen, monomorphization) is disjoint from Memstead's Arm-B tell vocabulary (mem, entity, schema, relationship, `memstead_*`) and from the Arm-A substrate vocabulary (frontmatter, wikilink, file path, grep). Generic tokens that could brush both worlds — "type", "reference", "module" — are common English/CS words, not arm tells, and will be handled by the tell lists authored in the next session; the disjointness must be re-verified there against the finalized tell lists (this is the open task for criterion 4 of plan 01, not a gap in this selection).

**Disclosed caveat (surfaced for the operator, not a criterion failure):** kara ships an "AI-first compiler interface" feature (structured JSON diagnostics, a compiler query API) and carries a `CLAUDE.md`, i.e. it is itself an AI-orchestrated project. This is *adjacent* framing, not the domain: the repo is a compiler, and "AI-first" describes one output format of the diagnostics, not the subject the mems will model. It does not overlap agent-*memory* or knowledge-graph tooling. It is recorded here so the published proof is transparent about it and the operator can weigh it; nothing in the criteria excludes an AI-built compiler, and the contamination screen (criterion 6) is unaffected because the repo is post-cutoff regardless.

### 6. Low fame, with a stated ceiling, so the contamination screen stays a backstop

**Pass.** **Star ceiling for this campaign: 500 stars.** kara is at 38 stars / 0 forks — an order of magnitude under the ceiling. Combined with the post-cutoff history (criterion 1), the bare-model contamination screen (threshold 0.5, inherited unchanged from the 2026-07-08 substrate eval) is a backstop, not load-bearing.

## Rejected alternatives (surveyed this selection)

- **`us-irs/spacepackets-rs`** (Apache-2.0, 40★, rich Keep-a-Changelog with abundant renames/removals) — **rejected on criterion 1**: its structurally-rich history is Sep–Nov 2025 (pre-cutoff); 2026 held only 30 scattered commits, one release, and a single trivial `chore: revert clippy settings`. The sliced window could not be made to predominantly postdate the cutoff without gutting the structural events.
- **`alsuren/mijia-homie`** (MQTT bridge, 127 commits in 2026) — **rejected on criteria 2 and 3**: the 2026 activity is almost entirely dependabot dependency bumps; no changelog, no git-detected renames, no substantive reverts, no mechanically-derivable status ground truth.
- **The 2026-created cohort at large** — dominated by agent-memory / knowledge-tooling repos (local-first memory engines, context filesystems, MCP servers), each rejected on criterion 5.
- Categorically rejected in plan 01's design: a famous OSS repo (readers/judge answer from pretraining), a synthetic chronicle (author knows the target schema), the project's own public engine repo (triple vocabulary collision), the operator's private repos (no public rerun kit).

## Reproducing this record

```
git clone https://github.com/karalang/kara
cd kara && git checkout 349066162dbfef551861d21c44c63a53260f79b5
# criterion 1: all history in 2026
git log --reverse --format=%cd --date=short | head -1   # 2026-05-04
# criterion 2: the pinned event commits resolve and carry the stated changes
git show --stat 523661e1 948d5527 f2df625c 0cb76707 930adcdb
# criterion 3: the ledger
wc -l docs/bug-ledger.jsonl                              # 450
# criterion 6: fame
gh api repos/karalang/kara --jq '.stargazers_count'     # 38 at selection
```
