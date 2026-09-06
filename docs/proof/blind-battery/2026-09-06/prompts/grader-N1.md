You are grading two anonymous answers, A and B, to one question about Memstead. You do not know who wrote them and must not guess; ignore any stray hint of where an answer came from and judge content only.

Verify every checkable claim in both answers against the engine's Rust source under `<source>/crates` and, where a claim is about behaviour, by running the release binary `<memstead>` in a scratch workspace of your own under `<run>/grader-N1` (create it with `<memstead> init --name probe --schema default@1.3.0 <run>/grader-N1/probe` or `<memstead> quickstart <run>/grader-N1/probe`; never touch any other workspace or repository). For this question, grade by execution: run each answer's command sequence literally, in order, in its own fresh empty directory under the scratch path, with `PATH` carrying the release binary, and score correctness by what runs as written.

Return ONLY a JSON object with exactly these keys:
- "winner": "A", "B" or "tie"
- "margin": 1, 2 or 3 (1 close, 3 decisive; 0 for a tie)
- "scores": {"A": {"correctness": 0-5, "completeness": 0-5, "usefulness": 0-5, "honesty": 0-5}, "B": {same}}
- "errors_A": a list of strings, each one false or unverifiable claim in A with what the source or the binary shows instead; empty if none
- "errors_B": the same for B
- "rationale": under 120 words

Correctness is truth against the source and the binary; completeness is whether the question's parts are all answered; usefulness is whether a developer could act on it; honesty is whether the answer hedges what it could not establish and invents nothing.

Question:

Give me the exact shell commands, in order, to: (1) create a fresh Memstead workspace in an empty folder with the built-in default schema, (2) create an entity of type `concept` titled "Optimistic locking" with the sections the default schema requires, (3) create a second entity of type `principle` titled "Validate at the boundary" and relate the concept to it with a relationship type the default schema allows, and (4) export the mem as a `.mem` archive file. The commands should run as written.

# Answer A

```sh
mkdir notes && cd notes
```
Start in an empty folder: `init` refuses a non-empty directory with `TARGET_NOT_EMPTY`.

```sh
memstead init --name notes --schema default@1.3.0
```
Creates a filesystem-mem workspace (`.memstead/workspace.toml`) with mem `notes` pinned to the built-in default schema; `default@1.3.0` is the current generation and needs no install.

```sh
memstead type concept
memstead type principle
```
Prints each type's section keys, which are required, and the relationship vocabulary. Do this before creating: I could not verify which sections the default schema marks required for `concept` and `principle`, and `create` refuses a missing required section outright (`MISSING_REQUIRED_SECTION`) rather than landing a placeholder.

```sh
memstead create --type concept --title "Optimistic locking" \
 --section definition="A write carries the hash it read; a stale hash is refused." \
 --section explanation="Concurrent writers detect conflicts instead of overwriting."
```
Entity id becomes `notes--optimistic-locking` (mem prefix plus title slug). Hedge: `--type`/`--title`/`--section KEY=TEXT` is my best reading of the CLI shape (the update command uses `--section-unset KEY` and `--from <file.json>`); confirm with `memstead create --help`, and substitute the required section keys `type concept` reported.

```sh
memstead create --type principle --title "Validate at the boundary" \
 --section statement="Every input is checked where it enters the system."
```
Same caveat on section keys.

```sh
memstead relate notes--optimistic-locking notes--validate-at-the-boundary RELATED_TO
```
Adds one typed edge, written only on the source entity. `RELATED_TO` is a general-purpose type present in the default-derived vocabulary; if strict mode refuses it with `INVALID_REL_TYPE`, the error payload lists the allowed names. Avoid `REFERENCES`, which is typically the wiki-link alias type and refuses manual authoring. Argument order is unverified; check `memstead relate --help`.

```sh
memstead mem set-version notes 0.1.0
memstead export --format mem notes
```
Export refuses `MEM_CONFIG_INCOMPLETE` when the material has no version, so set one first. Writes a deterministic `.mem` zip (config, pinned schema source, every entity); the output path is reported as `archive_path`, and an output-path flag, if any, was not verifiable.

# Answer B

```sh
mkdir notes && cd notes
```
Fresh, empty folder; `init` refuses a non-empty target and a folder nested under another workspace.

```sh
memstead init --name notes --schema default@1.3.0
```
Creates the single-mem folder workspace; `--schema` must be an exact `name@version` pin, and `default@1.3.0` is the newest built-in `default` generation.

```sh
memstead type concept
memstead type principle
```
Prints each type's sections marked required/optional and the schema's relationship vocabulary; use it to confirm the section keys and rel-type below.

```sh
memstead create --type concept --title "Optimistic locking" \
 --section "definition=A write succeeds only if the entity's hash still matches the hash the writer read." \
 --section "explanation=Concurrent writers detect each other's changes instead of overwriting them."
```
`definition` and `explanation` are the sections every verified concept write on default@1.3.0 supplies; the id becomes `notes--optimistic-locking`.

```sh
memstead create --type principle --title "Validate at the boundary" \
 --section "statement=Check every input where it enters the system, not deeper inside." \
 --section "scope=All external inputs." \
 --section "justification=Inner code can then trust its arguments."
```
Principle carries `statement`, `scope`, `justification` (plus optional `exceptions`, `consequences`); which of those 1.3.0 marks required I could not confirm, so if this refuses with `MISSING_REQUIRED_SECTION` add the keys it names.

```sh
memstead relate notes--optimistic-locking USES notes--validate-at-the-boundary
```
Positional `FROM REL_TYPE TO`; `USES` is a default-schema rel-type verified through `relate` on a folder mem. Do not use `REFERENCES`: the default schema reserves it for body wiki-links and refuses it from `relate`. If `type concept` shows an endpoint restriction on `USES`, pick another listed rel-type.

```sh
memstead export --format mem --mem notes -o notes.mem
```
Writes the portable `.mem` zip; without `-o` it lands as `./notes.mem`.
