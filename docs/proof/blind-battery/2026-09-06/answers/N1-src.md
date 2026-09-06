```sh
mkdir notes && cd notes
```
Fresh, empty folder; `init` refuses a non-empty target and a folder nested under another workspace (memstead-cli/src/commands/init.rs: `run`, `ensure_empty`, `find_ancestor_workspace`).

```sh
memstead init --name notes --schema default@1.3.0
```
Creates the single-mem folder workspace; `--schema` must be an exact `name@version` pin, and `default@1.3.0` is the newest built-in `default` generation (init.rs: `InitArgs`; memstead-cli/src/commands/quickstart.rs pins `default` 1.3.0; memstead-cli/tests/export_redaction.rs: `folder_workspace`).

```sh
memstead type concept
memstead type principle
```
Prints each type's sections marked required/optional and the schema's relationship vocabulary; use it to confirm the section keys and rel-type below (memstead-cli/src/commands/type_cmd.rs: `run`; memstead-base/src/render.rs: `render_type_info_markdown_in`).

```sh
memstead create --type concept --title "Optimistic locking" \
  --section "definition=A write succeeds only if the entity's hash still matches the hash the writer read." \
  --section "explanation=Concurrent writers detect each other's changes instead of overwriting them."
```
`definition` and `explanation` are the sections every verified concept write on default@1.3.0 supplies; the id becomes `notes--optimistic-locking` (memstead-cli/src/commands/create.rs: `Args`; export_redaction.rs: `create`; memstead-cli/tests/quickstart_and_schema_new.rs: `create_relation_lands_edges_on_a_filesystem_mem_workspace`; memstead-cli/tests/write_commands.rs: `relate_works_on_filesystem_mem_workspace`).

```sh
memstead create --type principle --title "Validate at the boundary" \
  --section "statement=Check every input where it enters the system, not deeper inside." \
  --section "scope=All external inputs." \
  --section "justification=Inner code can then trust its arguments."
```
Principle carries `statement`, `scope`, `justification` (plus optional `exceptions`, `consequences`); which of those 1.3.0 marks required I could not confirm, so if this refuses with `MISSING_REQUIRED_SECTION` add the keys it names (memstead-base/src/entity/loader.rs: `load_mixed_schema_mem_uses_per_file_schema`; memstead-base/src/engine/mutation/create.rs: `create_entity_refuses_missing_required_sections_with_typed_envelope`).

```sh
memstead relate notes--optimistic-locking USES notes--validate-at-the-boundary
```
Positional `FROM REL_TYPE TO`; `USES` is a default-schema rel-type verified through `relate` on a folder mem. Do not use `REFERENCES`: the default schema reserves it for body wiki-links and refuses it from `relate` (memstead-cli/src/commands/relate.rs: `Args`; write_commands.rs: `relate_works_on_filesystem_mem_workspace`; memstead-base/src/engine/mutation/relate.rs alias tests). If `type concept` shows an endpoint restriction on `USES`, pick another listed rel-type.

```sh
memstead export --format mem --mem notes -o notes.mem
```
Writes the portable `.mem` zip; without `-o` it lands as `./notes.mem` (memstead-cli/src/commands/export.rs: `run_mem_filesystem`; export_redaction.rs: `export`).
