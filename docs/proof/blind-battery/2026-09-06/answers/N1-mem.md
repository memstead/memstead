```sh
mkdir notes && cd notes
```
Start in an empty folder: `init` refuses a non-empty directory with `TARGET_NOT_EMPTY` (engine--cli-command-surface, engine--cli-cold-start-quickstart-and-schema-scaffold-surface).

```sh
memstead init --name notes --schema default@1.3.0
```
Creates a filesystem-mem workspace (`.memstead/workspace.toml`) with mem `notes` pinned to the built-in default schema; `default@1.3.0` is the current generation and needs no install (engine--worked-example-schemas, engine--built-in-schema-catalogue).

```sh
memstead type concept
memstead type principle
```
Prints each type's section keys, which are required, and the relationship vocabulary. Do this before creating: I could not verify which sections the default schema marks required for `concept` and `principle`, and `create` refuses a missing required section outright (`MISSING_REQUIRED_SECTION`) rather than landing a placeholder (engine--create-mutation, engine--schema-describe-projection-surface).

```sh
memstead create --type concept --title "Optimistic locking" \
  --section definition="A write carries the hash it read; a stale hash is refused." \
  --section explanation="Concurrent writers detect conflicts instead of overwriting."
```
Entity id becomes `notes--optimistic-locking` (mem prefix plus title slug). Hedge: `--type`/`--title`/`--section KEY=TEXT` is my best reading of the CLI shape (the update command uses `--section-unset KEY` and `--from <file.json>`); confirm with `memstead create --help`, and substitute the required section keys `type concept` reported (engine--create-mutation, engine--entity, engine--update-mutation).

```sh
memstead create --type principle --title "Validate at the boundary" \
  --section statement="Every input is checked where it enters the system."
```
Same caveat on section keys (engine--create-mutation).

```sh
memstead relate notes--optimistic-locking notes--validate-at-the-boundary RELATED_TO
```
Adds one typed edge, written only on the source entity. `RELATED_TO` is a general-purpose type present in the default-derived vocabulary; if strict mode refuses it with `INVALID_REL_TYPE`, the error payload lists the allowed names. Avoid `REFERENCES`, which is typically the wiki-link alias type and refuses manual authoring. Argument order is unverified; check `memstead relate --help` (engine--relate-mutation, engine--schema-definition-format, engine--alias-synthesis-pass).

```sh
memstead mem set-version notes 0.1.0
memstead export --format mem notes
```
Export refuses `MEM_CONFIG_INCOMPLETE` when the mem has no version, so set one first. Writes a deterministic `.mem` zip (config, pinned schema source, every entity); the output path is reported as `archive_path`, and an output-path flag, if any, was not verifiable (engine--mem-archive-export-surface, engine--mem-lifecycle-operations).
