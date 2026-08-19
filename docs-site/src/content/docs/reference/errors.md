---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 206

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1793` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:356`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:430`<br>`crates/memstead-cli/src/commands/publish.rs:176` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:276` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:269`<br>`crates/memstead-cli/src/commands/publish.rs:529` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1781` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:117`<br>`crates/memstead-mcp/src/server.rs:2986` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1162`<br>`crates/memstead-mcp/src/server.rs:844` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1990` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1164`<br>`crates/memstead-mcp/src/server.rs:944` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:122`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1786` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1193`<br>`crates/memstead-mcp/src/server.rs:1085` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1168`<br>`crates/memstead-base/src/ops/mod.rs:1773` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1177` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1175`<br>`crates/memstead-mcp/src/filesystem_server.rs:459` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1719` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1176`<br>`crates/memstead-mcp/src/filesystem_server.rs:468` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:1782` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1216`<br>`crates/memstead-base/src/ops/mod.rs:1795`<br>`crates/memstead-mcp/src/server.rs:1484` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:292` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:316` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1152`<br>`crates/memstead-mcp/src/server.rs:1589` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1723` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1774` |
| `EMBEDDED_SCHEMA_INVALID` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1205`<br>`crates/memstead-cli/src/commands/install.rs:238`<br>`crates/memstead-mcp/src/server.rs:1383` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1180`<br>`crates/memstead-mcp/src/server.rs:1657` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:1777` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1167`<br>`crates/memstead-mcp/src/server.rs:765` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1171`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:58`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:721`<br>`crates/memstead-cli/src/commands/update.rs:744`<br>`crates/memstead-mcp/src/filesystem_server.rs:370`<br>`crates/memstead-mcp/src/filesystem_server.rs:1042`<br>`crates/memstead-mcp/src/filesystem_server.rs:1875`<br>`crates/memstead-mcp/src/server.rs:755`<br>`crates/memstead-mcp/src/server.rs:1869`<br>`crates/memstead-mcp/src/server.rs:2436` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1749` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1765` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1746` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1750` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:50` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1789` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:297` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1172`<br>`crates/memstead-mcp/src/server.rs:786` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1173` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:1181` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1770` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1718` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1779`<br>`crates/memstead-mcp/src/filesystem_server.rs:1839` |
| `INTERNAL_IO_ERROR` | CLI | `crates/memstead-cli/src/commands/install.rs:81`<br>`crates/memstead-cli/src/commands/quickstart.rs:150`<br>`crates/memstead-cli/src/commands/quickstart.rs:246`<br>`crates/memstead-cli/src/commands/quickstart.rs:321`<br>`crates/memstead-cli/src/commands/quickstart.rs:484`<br>`crates/memstead-cli/src/commands/quickstart.rs:613`<br>`crates/memstead-cli/src/commands/quickstart.rs:720`<br>`crates/memstead-cli/src/commands/quickstart.rs:732`<br>`crates/memstead-cli/src/setup.rs:626` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:67` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1212`<br>`crates/memstead-mcp/src/server.rs:1672` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1190`<br>`crates/memstead-mcp/src/server.rs:310`<br>`crates/memstead-mcp/src/server.rs:325`<br>`crates/memstead-mcp/src/server.rs:1293` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1751`<br>`crates/memstead-base/src/runtime_validator.rs:197` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:204` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1210`<br>`crates/memstead-base/src/engine/error.rs:1211`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:146`<br>`crates/memstead-cli/src/commands/batch.rs:153`<br>`crates/memstead-cli/src/commands/batch.rs:170`<br>`crates/memstead-cli/src/commands/batch.rs:187`<br>`crates/memstead-cli/src/commands/batch.rs:202`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:208`<br>`crates/memstead-cli/src/commands/batch_relate.rs:84`<br>`crates/memstead-cli/src/commands/batch_update.rs:213`<br>`crates/memstead-cli/src/commands/batch_update.rs:224`<br>`crates/memstead-cli/src/commands/batch_update.rs:352`<br>`crates/memstead-cli/src/commands/create.rs:166`<br>`crates/memstead-cli/src/commands/create.rs:173`<br>`crates/memstead-cli/src/commands/create.rs:189`<br>`crates/memstead-cli/src/commands/create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:236`<br>`crates/memstead-cli/src/commands/create.rs:374`<br>`crates/memstead-cli/src/commands/create.rs:453`<br>`crates/memstead-cli/src/commands/create.rs:476`<br>`crates/memstead-cli/src/commands/create.rs:491`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/export.rs:91`<br>`crates/memstead-cli/src/commands/export.rs:122`<br>`crates/memstead-cli/src/commands/export.rs:532`<br>`crates/memstead-cli/src/commands/export.rs:540`<br>`crates/memstead-cli/src/commands/install.rs:61`<br>`crates/memstead-cli/src/commands/mem.rs:1130`<br>`crates/memstead-cli/src/commands/mod.rs:112`<br>`crates/memstead-cli/src/commands/mod.rs:119`<br>`crates/memstead-cli/src/commands/publish.rs:113`<br>`crates/memstead-cli/src/commands/publish.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:133`<br>`crates/memstead-cli/src/commands/quickstart.rs:353`<br>`crates/memstead-cli/src/commands/quickstart.rs:378`<br>`crates/memstead-cli/src/commands/quickstart.rs:386`<br>`crates/memstead-cli/src/commands/quickstart.rs:456`<br>`crates/memstead-cli/src/commands/quickstart.rs:620`<br>`crates/memstead-cli/src/commands/quickstart.rs:630`<br>`crates/memstead-cli/src/commands/quickstart.rs:642`<br>`crates/memstead-cli/src/commands/quickstart.rs:691`<br>`crates/memstead-cli/src/commands/relate.rs:85`<br>`crates/memstead-cli/src/commands/relate.rs:90`<br>`crates/memstead-cli/src/commands/schema.rs:118`<br>`crates/memstead-cli/src/commands/schema.rs:865`<br>`crates/memstead-cli/src/commands/schema.rs:897`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:166`<br>`crates/memstead-cli/src/commands/update.rs:277`<br>`crates/memstead-cli/src/commands/update.rs:290`<br>`crates/memstead-cli/src/commands/update.rs:306`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:334`<br>`crates/memstead-cli/src/commands/update.rs:373`<br>`crates/memstead-cli/src/commands/update.rs:508`<br>`crates/memstead-cli/src/commands/update.rs:516`<br>`crates/memstead-cli/src/commands/update.rs:524`<br>`crates/memstead-cli/src/commands/update.rs:780`<br>`crates/memstead-cli/src/commands/update.rs:787`<br>`crates/memstead-cli/src/commands/update.rs:809`<br>`crates/memstead-cli/src/commands/update.rs:828`<br>`crates/memstead-cli/src/commands/update.rs:835`<br>`crates/memstead-cli/src/commands/update.rs:842`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:1399`<br>`crates/memstead-mcp/src/filesystem_server.rs:1725`<br>`crates/memstead-mcp/src/filesystem_server.rs:1855`<br>`crates/memstead-mcp/src/filesystem_server.rs:1890`<br>`crates/memstead-mcp/src/filesystem_server.rs:2159`<br>`crates/memstead-mcp/src/server.rs:361`<br>`crates/memstead-mcp/src/server.rs:414`<br>`crates/memstead-mcp/src/server.rs:1426`<br>`crates/memstead-mcp/src/server.rs:1439`<br>`crates/memstead-mcp/src/server.rs:2095`<br>`crates/memstead-mcp/src/server.rs:2267`<br>`crates/memstead-mcp/src/server.rs:2313`<br>`crates/memstead-mcp/src/server.rs:2351`<br>`crates/memstead-mcp/src/server.rs:2367`<br>`crates/memstead-mcp/src/server.rs:2480`<br>`crates/memstead-mcp/src/server.rs:2829`<br>`crates/memstead-mcp/src/server.rs:3437`<br>`crates/memstead-mcp/src/server.rs:3583`<br>`crates/memstead-mcp/src/server.rs:3679`<br>`crates/memstead-mcp/src/server.rs:3737`<br>`crates/memstead-mcp/src/server.rs:3837`<br>`crates/memstead-mcp/src/server.rs:3876`<br>`crates/memstead-mcp/src/server.rs:3905` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1192`<br>`crates/memstead-mcp/src/server.rs:1327`<br>`crates/memstead-mcp/src/server.rs:1740` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:201` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/server.rs:191` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1166`<br>`crates/memstead-cli/src/commands/batch_create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:227`<br>`crates/memstead-mcp/src/server.rs:1260` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:41`<br>`crates/memstead-mcp/src/server.rs:3205` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:129` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1191`<br>`crates/memstead-mcp/src/server.rs:1308` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:568`<br>`crates/memstead-cli/src/commands/schema.rs:154`<br>`crates/memstead-cli/src/commands/schema.rs:163`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:200`<br>`crates/memstead-cli/src/commands/schema.rs:977`<br>`crates/memstead-cli/src/commands/schema.rs:986` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:161` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1726` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1157`<br>`crates/memstead-mcp/src/server.rs:883` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1159`<br>`crates/memstead-mcp/src/server.rs:905` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:451` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1222`<br>`crates/memstead-mcp/src/server.rs:1644` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1214`<br>`crates/memstead-mcp/src/server.rs:1455` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1203`<br>`crates/memstead-base/src/engine/error.rs:1208`<br>`crates/memstead-cli/src/commands/workspace.rs:767`<br>`crates/memstead-cli/src/commands/workspace.rs:774`<br>`crates/memstead-mcp/src/filesystem_server.rs:835`<br>`crates/memstead-mcp/src/server.rs:1417`<br>`crates/memstead-mcp/src/server.rs:1619` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1786` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1174` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1209`<br>`crates/memstead-mcp/src/server.rs:1366` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:48` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1735` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1154`<br>`crates/memstead-mcp/src/server.rs:818` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1787` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1774` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1775` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:759` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1757` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1802` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1215`<br>`crates/memstead-base/src/ops/mod.rs:1794`<br>`crates/memstead-mcp/src/server.rs:1501` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1721` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1169`<br>`crates/memstead-base/src/ops/mod.rs:1772` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1197`<br>`crates/memstead-base/src/ops/mod.rs:1720`<br>`crates/memstead-mcp/src/server.rs:1179` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1752` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:534`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1158`<br>`crates/memstead-mcp/src/server.rs:892` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1769` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:216`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1724` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:657` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:183`<br>`crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:514`<br>`crates/memstead-cli/src/commands/export.rs:349` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1771` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1784` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1201`<br>`crates/memstead-base/src/engine/error.rs:1202`<br>`crates/memstead-mcp/src/filesystem_server.rs:837`<br>`crates/memstead-mcp/src/filesystem_server.rs:839`<br>`crates/memstead-mcp/src/server.rs:1601`<br>`crates/memstead-mcp/src/server.rs:1610` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1199`<br>`crates/memstead-mcp/src/server.rs:1216` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1198`<br>`crates/memstead-mcp/src/filesystem_server.rs:811`<br>`crates/memstead-mcp/src/server.rs:1203` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1543`<br>`crates/memstead-cli/src/commands/projection.rs:1588`<br>`crates/memstead-cli/src/commands/projection.rs:1623` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1578` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:468` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:416` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1498`<br>`crates/memstead-cli/src/commands/projection.rs:1916` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1366` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1730`<br>`crates/memstead-cli/src/commands/projection.rs:1764` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:1725` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:713` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:422` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:662` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1609` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1751` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:430`<br>`crates/memstead-cli/src/commands/projection.rs:687`<br>`crates/memstead-cli/src/commands/projection.rs:1349`<br>`crates/memstead-cli/src/commands/projection.rs:1541`<br>`crates/memstead-cli/src/commands/projection.rs:1556`<br>`crates/memstead-cli/src/commands/projection.rs:1720` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:410`<br>`crates/memstead-cli/src/commands/projection.rs:485`<br>`crates/memstead-cli/src/commands/projection.rs:543` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:914` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:940`<br>`crates/memstead-cli/src/commands/projection.rs:1131`<br>`crates/memstead-cli/src/commands/projection.rs:1243`<br>`crates/memstead-cli/src/commands/projection.rs:1252`<br>`crates/memstead-cli/src/commands/projection.rs:1262` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:1183` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:907` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:919` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:902` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:427`<br>`crates/memstead-cli/src/commands/projection.rs:1024`<br>`crates/memstead-cli/src/commands/projection.rs:1404` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1439` |
| `PROJECTION_QUARANTINED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1008` |
| `PROJECTION_STORE_LEGACY` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:498`<br>`crates/memstead-cli/src/commands/projection.rs:1640` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1948` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1975` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1864`<br>`crates/memstead-cli/src/commands/projection.rs:1927` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1161`<br>`crates/memstead-mcp/src/server.rs:861` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1754` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1762` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:1788` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:250` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1163`<br>`crates/memstead-mcp/src/server.rs:935` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:544`<br>`crates/memstead-cli/src/commands/unpublish.rs:100`<br>`crates/memstead-cli/src/registry/mod.rs:92` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:539`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1194`<br>`crates/memstead-mcp/src/server.rs:1103` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1185`<br>`crates/memstead-mcp/src/server.rs:1345` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1218`<br>`crates/memstead-mcp/src/server.rs:1519` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1182`<br>`crates/memstead-mcp/src/server.rs:1559` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1179`<br>`crates/memstead-mcp/src/filesystem_server.rs:510`<br>`crates/memstead-mcp/src/server.rs:1533` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1184`<br>`crates/memstead-mcp/src/server.rs:1576` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1178`<br>`crates/memstead-mcp/src/server.rs:1076` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1196`<br>`crates/memstead-mcp/src/server.rs:1145` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1785` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1213`<br>`crates/memstead-mcp/src/server.rs:1683` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1791` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1790` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:1778` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1779` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1204`<br>`crates/memstead-cli/src/commands/schema.rs:838`<br>`crates/memstead-cli/src/commands/schema.rs:872`<br>`crates/memstead-cli/src/commands/schema.rs:888`<br>`crates/memstead-mcp/src/server.rs:1378` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:138` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1776` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1207`<br>`crates/memstead-mcp/src/server.rs:1408` |
| `SCHEMA_UNSTAMPED_SOURCE_ROT` | engine | `crates/memstead-base/src/ops/mod.rs:1792` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1206`<br>`crates/memstead-cli/src/commands/schema.rs:589`<br>`crates/memstead-cli/src/commands/schema.rs:617`<br>`crates/memstead-cli/src/commands/schema.rs:642`<br>`crates/memstead-cli/src/commands/schema.rs:787`<br>`crates/memstead-cli/src/commands/schema.rs:799`<br>`crates/memstead-mcp/src/server.rs:1396` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1160`<br>`crates/memstead-mcp/src/server.rs:922` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1766` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1753` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1220`<br>`crates/memstead-mcp/src/server.rs:1628` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:202`<br>`crates/memstead-base/src/runtime_validator.rs:203`<br>`crates/memstead-base/src/section_format.rs:522` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:519` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1780` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:520` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1783` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1195`<br>`crates/memstead-mcp/src/server.rs:1194` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:1896` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1187`<br>`crates/memstead-mcp/src/server.rs:1266` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1730` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1189`<br>`crates/memstead-mcp/src/server.rs:1284` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1188`<br>`crates/memstead-mcp/src/server.rs:1275` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1768` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:171`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:1728` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1727` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1767` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:255` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1722` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1165`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:309`<br>`crates/memstead-mcp/src/server.rs:958`<br>`crates/memstead-mcp/src/server.rs:2522` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1744` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1725` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1153`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:351`<br>`crates/memstead-cli/src/commands/export.rs:140`<br>`crates/memstead-cli/src/commands/export.rs:267`<br>`crates/memstead-cli/src/commands/export.rs:389`<br>`crates/memstead-cli/src/commands/uninstall.rs:36`<br>`crates/memstead-mcp/src/filesystem_server.rs:1863`<br>`crates/memstead-mcp/src/server.rs:804`<br>`crates/memstead-mcp/src/server.rs:2289`<br>`crates/memstead-mcp/src/server.rs:2396`<br>`crates/memstead-mcp/src/server.rs:3419` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:196` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1760` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1155`<br>`crates/memstead-mcp/src/server.rs:831` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1156`<br>`crates/memstead-mcp/src/server.rs:874` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:861` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1729` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1186`<br>`crates/memstead-mcp/src/server.rs:1468` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:285` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:2040`<br>`crates/memstead-base/src/workspace_store.rs:157`<br>`crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:410`<br>`crates/memstead-cli/src/commands/publish.rs:390`<br>`crates/memstead-cli/src/setup.rs:41`<br>`crates/memstead-mcp/src/server.rs:4330` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:160` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:158` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:159` |
