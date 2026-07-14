# Arm definitions and prompts

Pre-registered. Immutable after the first real run except by a dated amendment note (see [README.md](README.md)).

## The unit of comparison: substrate + native access surface

The two arms differ in exactly one thing — the substrate the knowledge base lives in, and therefore the native surface used to read and write it. Everything else (the model, the token allowance, the source material each round, the content and quality guidance in the prompts) is held identical.

- **Arm A — tolerant markdown directory.** A directory of markdown files, each with YAML frontmatter carrying a free-string `type:` and a body, related files joined by `[[wikilinks]]`, an optional `index.md`. Every write is accepted as-is; there is no validation and no typed vocabulary. Writers mutate it with filesystem write tools; readers read it with filesystem read/search tools. This is the OKF / basic-memory posture.
- **Arm B — engine-gated Memstead mem.** A Memstead mem pinned to the built-in `software@0.1.0` schema. Every write goes through the MCP mutation surface and is validated at write time; a write that violates the schema is refused with a recovery payload the writer repairs from. Readers read it with the Memstead read tools.

### Why Arm A's reader gets filesystem tools (and not the Memstead read tools)

The comparison is deliberately substrate-**with-its-native-access-surface**, not bytes-with-identical-tools. The product under test ships as a pair: the engine's write-time typing *and* the typed read surface that typing makes possible. Handing Arm A the Memstead read tools would test a configuration nobody ships; handing Arm B only grep would strip away the very layer under test. So each arm is read with the surface that its substrate ships with — filesystem read/search for a markdown directory, the Memstead read tools for a mem. The consequence is stated plainly: **tool-surface differences are part of the measured variable, not a confound to be denied.** This cuts both ways and is defensible from the baseline's side — a markdown directory's honest, native access surface *is* filesystem read and text search; that is exactly how OKF/basic-memory users query their notes. The confound this introduces on the accuracy endpoint (write-gate and read-surface are inseparable) is bracketed by two pre-registered mechanisms: the **slope qualifier** (a static read-surface advantage present already at round 1 cannot band positive — see [bands.md](bands.md)) and the co-primary, read-surface-free **integrity band** (which measures the corpora themselves, not answers).

## Prompt parity contract

Each prompt below is a **shared skeleton** plus one clearly delimited **substrate block**. The shared skeleton is byte-identical across the two arms; only the substrate block differs, and it contains *only* substrate/access mechanics — never a quality exhortation, never a mention that a measurement or comparison is happening, never a hint about what the queries will ask. A mechanical diff of the two writer prompts (or the two reader prompts) must surface **only** the substrate block. The substrate blocks necessarily describe different mechanics (validation vs none, typed tools vs files) — that asymmetry *is* the treatment under test, not a parity violation.

**Writers are blind to being measured.** No writer prompt mentions an experiment, a comparison, quality metrics, a judge, or the query battery.

---

## Writer prompt — full rounds (1, 2, 4, 5, 7, 8, 10)

**Shared skeleton (identical across arms):**

> You maintain a knowledge base about a software project. New source material about the project arrives this round: commits and changelog entries, a bug tracker, and design and roadmap documents. Read this round's new material and update the knowledge base so it reflects the current state of the project as accurately and completely as you can — its design decisions, its implementation phases and how they depend on one another, its bug tracker, and how things have changed over time, including renames, reversals, and features that supersede or deprecate earlier ones. Keep the knowledge base internally consistent: when the new material changes or overturns something already recorded, update or replace the earlier information rather than leaving both versions in place.
>
> `{SUBSTRATE BLOCK}`
>
> This round's new source material:
>
> `{ROUND SLICE CONTENT}`

**Substrate block — Arm A:**

> The knowledge base is a directory of markdown files. Each file has YAML frontmatter with a `type:` field you choose and a markdown body; relate files to one another with `[[wikilinks]]`. Read the current directory and create, edit, or delete files with your filesystem tools. There are no restrictions on file structure, type names, or links.

**Substrate block — Arm B:**

> The knowledge base is a Memstead mem accessed through the MCP tools. Call `memstead_schema` once to see the schema the mem is pinned to, then read the current state with `memstead_overview` / `memstead_search` / `memstead_entity` and record the material by creating, updating, and relating entities with `memstead_create` / `memstead_update` / `memstead_relate`. Writes are validated against the schema; when a write is refused, fix it from the recovery payload the refusal returns and resubmit.

---

## Writer prompt — hurry rounds (3, 6, 9)

Hurry rounds run at **half the full-round token allowance** ([bands.md](bands.md)) with a terser instruction — realistic time pressure, not adversarial sabotage. The terse skeleton is identical across arms; the same substrate blocks as above apply verbatim.

**Shared skeleton (identical across arms):**

> Quickly bring the knowledge base up to date with this round's new source material about the software project. Read the new material and record what changed — new decisions, phase and dependency changes, new or resolved bugs, and anything renamed, reversed, or superseded — keeping the knowledge base consistent with itself.
>
> `{SUBSTRATE BLOCK}`
>
> This round's new source material:
>
> `{ROUND SLICE CONTENT}`

---

## Reader prompt (checkpoints: after rounds 1, 3, 5, 10)

Run at `n = 3` trials per query per arm under the fixed reader token budget ([bands.md](bands.md)).

**Shared skeleton (identical across arms):**

> Answer the following question about the software project, using only the knowledge base. Answer the question directly and concisely: state the answer itself, not how you found it, which tools you used, or where in the knowledge base it is stored. If the knowledge base does not contain enough information to answer, say so plainly.
>
> `{SUBSTRATE BLOCK}`
>
> Question: `{QUERY}`

**Substrate block — Arm A:**

> The knowledge base is a directory of markdown files. Read it with your filesystem tools.

**Substrate block — Arm B:**

> The knowledge base is a Memstead mem. Read it with `memstead_overview`, `memstead_search`, and `memstead_entity`.

The instruction to **answer directly and not describe how or where the answer was found** is the primary defence against retrieval-method tells (a reader narrating "I searched the files" or "I queried the mem"); the tell lists in [tell-lists.json](tell-lists.json) are the backstop for leaks that survive it. This instruction is in the shared skeleton, so it applies identically to both arms.
