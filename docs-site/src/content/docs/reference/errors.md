---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 232

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:2021` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:554`<br>`crates/memstead-cli/src/commands/publish.rs:242`<br>`crates/memstead-cli/src/commands/type_cmd.rs:226` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ANCHORS_SIDECAR_UNREADABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2261` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:267`<br>`crates/memstead-cli/src/commands/publish.rs:341` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:384` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:377`<br>`crates/memstead-cli/src/commands/publish.rs:636` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:513`<br>`crates/memstead-cli/src/lib.rs:55` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:2008` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:117`<br>`crates/memstead-cli/src/commands/check.rs:259`<br>`crates/memstead-mcp/src/filesystem_server.rs:1667`<br>`crates/memstead-mcp/src/server.rs:3201` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1277`<br>`crates/memstead-mcp/src/server.rs:920` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:2206` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1279`<br>`crates/memstead-mcp/src/server.rs:1020` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:183`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:43` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1900` |
| `CONFIG_WRITE_INTERVENED` | engine | `crates/memstead-base/src/ops/mod.rs:2000` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1308`<br>`crates/memstead-mcp/src/filesystem_server.rs:749`<br>`crates/memstead-mcp/src/server.rs:1161` |
| `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND` | engine | `crates/memstead-base/src/engine/error.rs:1330` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1283`<br>`crates/memstead-base/src/ops/mod.rs:1997` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1292` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1290`<br>`crates/memstead-mcp/src/filesystem_server.rs:512` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1942` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1291`<br>`crates/memstead-mcp/src/filesystem_server.rs:521` |
| `CROSS_SCHEMA_LINK_UNDECLARED` | engine | `crates/memstead-base/src/ops/mod.rs:2011` |
| `DANGLING_LINK_NOT_RELATED` | engine | `crates/memstead-base/src/ops/mod.rs:3604` |
| `DANGLING_LINK_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3603` |
| `DANGLING_RELATION_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3605` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:2009` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1339`<br>`crates/memstead-base/src/ops/mod.rs:2023`<br>`crates/memstead-mcp/src/filesystem_server.rs:956`<br>`crates/memstead-mcp/src/server.rs:1597` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:400` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:424` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1267`<br>`crates/memstead-mcp/src/server.rs:1702` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1946` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1998` |
| `EMBEDDED_SCHEMA_INVALID` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1323`<br>`crates/memstead-cli/src/commands/install.rs:273`<br>`crates/memstead-mcp/src/server.rs:1486` |
| `EMPTY_UNDECLARED_HEADING` | engine | `crates/memstead-base/src/runtime_validator.rs:247` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1295`<br>`crates/memstead-mcp/src/filesystem_server.rs:1058`<br>`crates/memstead-mcp/src/server.rs:1770` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:2004` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1282`<br>`crates/memstead-mcp/src/filesystem_server.rs:408`<br>`crates/memstead-mcp/src/server.rs:830` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1286`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:58`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:142`<br>`crates/memstead-cli/src/commands/rename.rs:176`<br>`crates/memstead-cli/src/commands/update.rs:828`<br>`crates/memstead-cli/src/commands/update.rs:852`<br>`crates/memstead-mcp/src/filesystem_server.rs:423`<br>`crates/memstead-mcp/src/filesystem_server.rs:1127`<br>`crates/memstead-mcp/src/filesystem_server.rs:2072`<br>`crates/memstead-mcp/src/server.rs:820`<br>`crates/memstead-mcp/src/server.rs:1983`<br>`crates/memstead-mcp/src/server.rs:2597` |
| `EXPECTED_HASH_REQUIRED` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1425`<br>`crates/memstead-mcp/src/server.rs:2944` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1972` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1988` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1969` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1973` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:100` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:2017` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:645` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:34` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1287`<br>`crates/memstead-mcp/src/server.rs:862` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1288` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:1670` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1993` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1941` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:29`<br>`crates/memstead-mcp/src/filesystem_server.rs:1943`<br>`crates/memstead-mcp/src/filesystem_server.rs:2006`<br>`crates/memstead-mcp/src/filesystem_server.rs:2036` |
| `INTERNAL_IO_ERROR` | CLI | `crates/memstead-cli/src/commands/install.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:230`<br>`crates/memstead-cli/src/commands/quickstart.rs:370`<br>`crates/memstead-cli/src/commands/quickstart.rs:674`<br>`crates/memstead-cli/src/commands/quickstart.rs:857`<br>`crates/memstead-cli/src/commands/quickstart.rs:986`<br>`crates/memstead-cli/src/commands/quickstart.rs:1096`<br>`crates/memstead-cli/src/commands/quickstart.rs:1108`<br>`crates/memstead-cli/src/setup.rs:715` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:67` |
| `INVALID_CHECK_KIND` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:2368`<br>`crates/memstead-mcp/src/server.rs:3442` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1334`<br>`crates/memstead-base/src/engine/error.rs:1335`<br>`crates/memstead-mcp/src/filesystem_server.rs:1075`<br>`crates/memstead-mcp/src/filesystem_server.rs:2231`<br>`crates/memstead-mcp/src/server.rs:1785` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1305`<br>`crates/memstead-mcp/src/filesystem_server.rs:700`<br>`crates/memstead-mcp/src/server.rs:361`<br>`crates/memstead-mcp/src/server.rs:376`<br>`crates/memstead-mcp/src/server.rs:1396` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1974`<br>`crates/memstead-base/src/runtime_validator.rs:242` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:251` |
| `INVALID_IDENTITY` | CLI, MCP | `crates/memstead-cli/src/main.rs:131`<br>`crates/memstead-mcp/src/filesystem_server.rs:273`<br>`crates/memstead-mcp/src/server.rs:210` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1328`<br>`crates/memstead-base/src/engine/error.rs:1333`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:146`<br>`crates/memstead-cli/src/commands/batch.rs:153`<br>`crates/memstead-cli/src/commands/batch.rs:170`<br>`crates/memstead-cli/src/commands/batch.rs:187`<br>`crates/memstead-cli/src/commands/batch.rs:202`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:208`<br>`crates/memstead-cli/src/commands/batch_relate.rs:83`<br>`crates/memstead-cli/src/commands/batch_update.rs:262`<br>`crates/memstead-cli/src/commands/batch_update.rs:273`<br>`crates/memstead-cli/src/commands/batch_update.rs:292`<br>`crates/memstead-cli/src/commands/batch_update.rs:429`<br>`crates/memstead-cli/src/commands/check.rs:183`<br>`crates/memstead-cli/src/commands/check.rs:190`<br>`crates/memstead-cli/src/commands/check.rs:200`<br>`crates/memstead-cli/src/commands/conflicts.rs:100`<br>`crates/memstead-cli/src/commands/create.rs:168`<br>`crates/memstead-cli/src/commands/create.rs:175`<br>`crates/memstead-cli/src/commands/create.rs:191`<br>`crates/memstead-cli/src/commands/create.rs:198`<br>`crates/memstead-cli/src/commands/create.rs:238`<br>`crates/memstead-cli/src/commands/create.rs:376`<br>`crates/memstead-cli/src/commands/create.rs:460`<br>`crates/memstead-cli/src/commands/create.rs:483`<br>`crates/memstead-cli/src/commands/create.rs:498`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/export.rs:111`<br>`crates/memstead-cli/src/commands/export.rs:142`<br>`crates/memstead-cli/src/commands/export.rs:174`<br>`crates/memstead-cli/src/commands/export.rs:188`<br>`crates/memstead-cli/src/commands/export.rs:789`<br>`crates/memstead-cli/src/commands/export.rs:794`<br>`crates/memstead-cli/src/commands/export.rs:826`<br>`crates/memstead-cli/src/commands/export.rs:834`<br>`crates/memstead-cli/src/commands/install.rs:69`<br>`crates/memstead-cli/src/commands/mem.rs:1147`<br>`crates/memstead-cli/src/commands/mod.rs:113`<br>`crates/memstead-cli/src/commands/mod.rs:120`<br>`crates/memstead-cli/src/commands/projection.rs:1885`<br>`crates/memstead-cli/src/commands/publish.rs:128`<br>`crates/memstead-cli/src/commands/publish.rs:136`<br>`crates/memstead-cli/src/commands/publish.rs:158`<br>`crates/memstead-cli/src/commands/quickstart.rs:192`<br>`crates/memstead-cli/src/commands/quickstart.rs:212`<br>`crates/memstead-cli/src/commands/quickstart.rs:726`<br>`crates/memstead-cli/src/commands/quickstart.rs:750`<br>`crates/memstead-cli/src/commands/quickstart.rs:758`<br>`crates/memstead-cli/src/commands/quickstart.rs:829`<br>`crates/memstead-cli/src/commands/quickstart.rs:993`<br>`crates/memstead-cli/src/commands/quickstart.rs:1003`<br>`crates/memstead-cli/src/commands/quickstart.rs:1015`<br>`crates/memstead-cli/src/commands/quickstart.rs:1066`<br>`crates/memstead-cli/src/commands/relate.rs:85`<br>`crates/memstead-cli/src/commands/relate.rs:90`<br>`crates/memstead-cli/src/commands/schema.rs:147`<br>`crates/memstead-cli/src/commands/schema.rs:278`<br>`crates/memstead-cli/src/commands/schema.rs:316`<br>`crates/memstead-cli/src/commands/schema.rs:1103`<br>`crates/memstead-cli/src/commands/schema.rs:1135`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:184`<br>`crates/memstead-cli/src/commands/update.rs:346`<br>`crates/memstead-cli/src/commands/update.rs:359`<br>`crates/memstead-cli/src/commands/update.rs:375`<br>`crates/memstead-cli/src/commands/update.rs:382`<br>`crates/memstead-cli/src/commands/update.rs:403`<br>`crates/memstead-cli/src/commands/update.rs:446`<br>`crates/memstead-cli/src/commands/update.rs:601`<br>`crates/memstead-cli/src/commands/update.rs:609`<br>`crates/memstead-cli/src/commands/update.rs:617`<br>`crates/memstead-cli/src/commands/update.rs:910`<br>`crates/memstead-cli/src/commands/update.rs:917`<br>`crates/memstead-cli/src/commands/update.rs:939`<br>`crates/memstead-cli/src/commands/update.rs:959`<br>`crates/memstead-cli/src/commands/update.rs:966`<br>`crates/memstead-cli/src/commands/update.rs:977`<br>`crates/memstead-cli/src/commands/workspace.rs:717`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:1006`<br>`crates/memstead-mcp/src/filesystem_server.rs:1540`<br>`crates/memstead-mcp/src/filesystem_server.rs:1872`<br>`crates/memstead-mcp/src/filesystem_server.rs:2052`<br>`crates/memstead-mcp/src/filesystem_server.rs:2087`<br>`crates/memstead-mcp/src/filesystem_server.rs:2467`<br>`crates/memstead-mcp/src/server.rs:412`<br>`crates/memstead-mcp/src/server.rs:465`<br>`crates/memstead-mcp/src/server.rs:1529`<br>`crates/memstead-mcp/src/server.rs:1552`<br>`crates/memstead-mcp/src/server.rs:2245`<br>`crates/memstead-mcp/src/server.rs:2429`<br>`crates/memstead-mcp/src/server.rs:2475`<br>`crates/memstead-mcp/src/server.rs:2512`<br>`crates/memstead-mcp/src/server.rs:2528`<br>`crates/memstead-mcp/src/server.rs:2641`<br>`crates/memstead-mcp/src/server.rs:3043`<br>`crates/memstead-mcp/src/server.rs:3708`<br>`crates/memstead-mcp/src/server.rs:3851`<br>`crates/memstead-mcp/src/server.rs:3944`<br>`crates/memstead-mcp/src/server.rs:4001`<br>`crates/memstead-mcp/src/server.rs:4100`<br>`crates/memstead-mcp/src/server.rs:4139`<br>`crates/memstead-mcp/src/server.rs:4168` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1307`<br>`crates/memstead-mcp/src/filesystem_server.rs:736`<br>`crates/memstead-mcp/src/server.rs:1430`<br>`crates/memstead-mcp/src/server.rs:1854` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:246` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:245` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/filesystem_server.rs:294`<br>`crates/memstead-mcp/src/server.rs:242` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:523` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1281`<br>`crates/memstead-cli/src/commands/batch_create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:229`<br>`crates/memstead-mcp/src/filesystem_server.rs:402`<br>`crates/memstead-mcp/src/server.rs:1363` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:108`<br>`crates/memstead-mcp/src/filesystem_server.rs:2353`<br>`crates/memstead-mcp/src/server.rs:3423` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:144` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1306`<br>`crates/memstead-mcp/src/filesystem_server.rs:716`<br>`crates/memstead-mcp/src/server.rs:1411` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:748`<br>`crates/memstead-cli/src/commands/export.rs:862`<br>`crates/memstead-cli/src/commands/schema.rs:352`<br>`crates/memstead-cli/src/commands/schema.rs:361`<br>`crates/memstead-cli/src/commands/schema.rs:386`<br>`crates/memstead-cli/src/commands/schema.rs:398`<br>`crates/memstead-cli/src/commands/schema.rs:1215`<br>`crates/memstead-cli/src/commands/schema.rs:1224` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:165` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1949` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1272`<br>`crates/memstead-mcp/src/server.rs:959` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1274`<br>`crates/memstead-mcp/src/server.rs:981` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:558` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1345`<br>`crates/memstead-mcp/src/filesystem_server.rs:1049`<br>`crates/memstead-mcp/src/server.rs:1757` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1337`<br>`crates/memstead-mcp/src/filesystem_server.rs:1025`<br>`crates/memstead-mcp/src/server.rs:1568` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1321`<br>`crates/memstead-base/src/engine/error.rs:1326`<br>`crates/memstead-cli/src/commands/workspace.rs:900`<br>`crates/memstead-cli/src/commands/workspace.rs:907`<br>`crates/memstead-mcp/src/filesystem_server.rs:919`<br>`crates/memstead-mcp/src/server.rs:1520`<br>`crates/memstead-mcp/src/server.rs:1732` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:2014` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1289` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1327`<br>`crates/memstead-mcp/src/server.rs:1469` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:53` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1849` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1269`<br>`crates/memstead-mcp/src/server.rs:894` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:2015` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1888` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1999` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:979` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1871` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1916` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1338`<br>`crates/memstead-base/src/ops/mod.rs:2022`<br>`crates/memstead-mcp/src/filesystem_server.rs:943`<br>`crates/memstead-mcp/src/server.rs:1614` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1944` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1284`<br>`crates/memstead-base/src/ops/mod.rs:1995` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1312`<br>`crates/memstead-base/src/ops/mod.rs:1943`<br>`crates/memstead-mcp/src/filesystem_server.rs:890`<br>`crates/memstead-mcp/src/server.rs:1289` |
| `MOUNT_UNBACKED` | engine | `crates/memstead-base/src/ops/mod.rs:2003` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1975` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:641`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1273`<br>`crates/memstead-mcp/src/server.rs:968` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1992` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:308`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NOT_CONFLICTED` | engine | `crates/memstead-base/src/engine/error.rs:1332` |
| `NO_ACTIVE_BINDING` | CLI | `crates/memstead-cli/src/commands/projection.rs:1826` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1947` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:877` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:183`<br>`crates/memstead-cli/src/commands/changes.rs:72`<br>`crates/memstead-cli/src/commands/create.rs:521`<br>`crates/memstead-cli/src/commands/export.rs:547` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1994` |
| `OUT_OF_BAND_EDITS_UNDETECTED` | engine | `crates/memstead-base/src/ops/mod.rs:2001` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:2012` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1319`<br>`crates/memstead-base/src/engine/error.rs:1320`<br>`crates/memstead-mcp/src/filesystem_server.rs:921`<br>`crates/memstead-mcp/src/filesystem_server.rs:923`<br>`crates/memstead-mcp/src/server.rs:1714`<br>`crates/memstead-mcp/src/server.rs:1723` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1314`<br>`crates/memstead-mcp/src/filesystem_server.rs:904`<br>`crates/memstead-mcp/src/server.rs:1318` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1313`<br>`crates/memstead-mcp/src/filesystem_server.rs:893`<br>`crates/memstead-mcp/src/server.rs:1304` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1907`<br>`crates/memstead-cli/src/commands/projection.rs:1952`<br>`crates/memstead-cli/src/commands/projection.rs:1987` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1942` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:689` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:629` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:592`<br>`crates/memstead-cli/src/commands/projection.rs:1621`<br>`crates/memstead-cli/src/commands/projection.rs:2311` |
| `PROJECTION_EDIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1768` |
| `PROJECTION_EDIT_INVALID_JSON` | CLI | `crates/memstead-cli/src/commands/projection.rs:1745` |
| `PROJECTION_EDIT_REFUSED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1751` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1489` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2090`<br>`crates/memstead-cli/src/commands/projection.rs:2124` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:2079` |
| `PROJECTION_EXCLUDE_PARTIAL_ENUMERATION` | CLI | `crates/memstead-cli/src/commands/projection.rs:2085` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:922` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:635` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:871`<br>`crates/memstead-cli/src/commands/quickstart.rs:596` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1973` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2111` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:643`<br>`crates/memstead-cli/src/commands/projection.rs:896`<br>`crates/memstead-cli/src/commands/projection.rs:1472`<br>`crates/memstead-cli/src/commands/projection.rs:1900`<br>`crates/memstead-cli/src/commands/projection.rs:1920`<br>`crates/memstead-cli/src/commands/projection.rs:2074` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:623`<br>`crates/memstead-cli/src/commands/projection.rs:706`<br>`crates/memstead-cli/src/commands/projection.rs:752`<br>`crates/memstead-cli/src/commands/projection.rs:1836` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:1033` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1059`<br>`crates/memstead-cli/src/commands/projection.rs:1254`<br>`crates/memstead-cli/src/commands/projection.rs:1366`<br>`crates/memstead-cli/src/commands/projection.rs:1375`<br>`crates/memstead-cli/src/commands/projection.rs:1385` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:1306` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:1026` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1038` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1021` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:640`<br>`crates/memstead-cli/src/commands/projection.rs:1143`<br>`crates/memstead-cli/src/commands/projection.rs:1527`<br>`crates/memstead-cli/src/commands/projection.rs:1736` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1562` |
| `PROJECTION_QUARANTINED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1127` |
| `PROJECTION_SCOPE_UNINTERPRETABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:650`<br>`crates/memstead-cli/src/commands/projection.rs:1904` |
| `PROJECTION_STORE_LEGACY` | engine | `crates/memstead-base/src/workspace_store.rs:166` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:602` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2460` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2472` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2231`<br>`crates/memstead-cli/src/commands/projection.rs:2322` |
| `PROJECTION_VERIFY_FINDINGS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2493` |
| `PROJECTION_VERIFY_INCONCLUSIVE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2520` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1276`<br>`crates/memstead-mcp/src/server.rs:937` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1977` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1985` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:2016` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:285` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:243` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1278`<br>`crates/memstead-mcp/src/server.rs:1011` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:651`<br>`crates/memstead-cli/src/commands/unpublish.rs:100`<br>`crates/memstead-cli/src/registry/mod.rs:92` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:646`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1309`<br>`crates/memstead-mcp/src/filesystem_server.rs:787`<br>`crates/memstead-mcp/src/server.rs:1196` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1300`<br>`crates/memstead-mcp/src/server.rs:1448` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1341`<br>`crates/memstead-mcp/src/filesystem_server.rs:970`<br>`crates/memstead-mcp/src/server.rs:1632` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1297`<br>`crates/memstead-mcp/src/server.rs:1672` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1294`<br>`crates/memstead-mcp/src/filesystem_server.rs:563`<br>`crates/memstead-mcp/src/server.rs:1646` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1299`<br>`crates/memstead-mcp/src/server.rs:1689` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1293`<br>`crates/memstead-mcp/src/server.rs:1152` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1311`<br>`crates/memstead-mcp/src/filesystem_server.rs:832`<br>`crates/memstead-mcp/src/server.rs:1228` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:2013` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1336`<br>`crates/memstead-mcp/src/filesystem_server.rs:1081`<br>`crates/memstead-mcp/src/server.rs:1797` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:2019` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:2018` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:2005` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:2006` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1322`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:291`<br>`crates/memstead-cli/src/commands/schema.rs:1076`<br>`crates/memstead-cli/src/commands/schema.rs:1110`<br>`crates/memstead-cli/src/commands/schema.rs:1126`<br>`crates/memstead-mcp/src/server.rs:1481` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:336` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:2002` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1325`<br>`crates/memstead-mcp/src/server.rs:1511` |
| `SCHEMA_UNSTAMPED_SOURCE_ROT` | engine | `crates/memstead-base/src/ops/mod.rs:2020` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1324`<br>`crates/memstead-cli/src/commands/schema.rs:809`<br>`crates/memstead-cli/src/commands/schema.rs:837`<br>`crates/memstead-cli/src/commands/schema.rs:862`<br>`crates/memstead-cli/src/commands/schema.rs:1013`<br>`crates/memstead-cli/src/commands/schema.rs:1025`<br>`crates/memstead-mcp/src/server.rs:1499` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1275`<br>`crates/memstead-mcp/src/server.rs:998` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1989` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1976` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1343`<br>`crates/memstead-mcp/src/filesystem_server.rs:1035`<br>`crates/memstead-mcp/src/server.rs:1741` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:249`<br>`crates/memstead-base/src/runtime_validator.rs:250`<br>`crates/memstead-base/src/section_format.rs:524` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:2007` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:522` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:244` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:2010` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1310`<br>`crates/memstead-mcp/src/filesystem_server.rs:792`<br>`crates/memstead-mcp/src/server.rs:1295` |
| `SIGNAL_THRESHOLD_CROSSED` | engine | `crates/memstead-base/src/ops/mod.rs:1996` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2290` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1302`<br>`crates/memstead-mcp/src/server.rs:1369` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1953` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1304`<br>`crates/memstead-mcp/src/server.rs:1387` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1303`<br>`crates/memstead-mcp/src/server.rs:1378` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1991` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:369`<br>`crates/memstead-cli/src/lib.rs:39` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:1951` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1950` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1990` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:306` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1945` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:167` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1280`<br>`crates/memstead-cli/src/commands/type_cmd.rs:79`<br>`crates/memstead-mcp/src/filesystem_server.rs:362`<br>`crates/memstead-mcp/src/filesystem_server.rs:2122`<br>`crates/memstead-mcp/src/server.rs:1034`<br>`crates/memstead-mcp/src/server.rs:2683` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1967` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1948` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1268`<br>`crates/memstead-cli/src/commands/changes.rs:232`<br>`crates/memstead-cli/src/commands/create.rs:353`<br>`crates/memstead-cli/src/commands/export.rs:211`<br>`crates/memstead-cli/src/commands/export.rs:374`<br>`crates/memstead-cli/src/commands/export.rs:587`<br>`crates/memstead-cli/src/commands/uninstall.rs:41`<br>`crates/memstead-mcp/src/filesystem_server.rs:2060`<br>`crates/memstead-mcp/src/filesystem_server.rs:2479`<br>`crates/memstead-mcp/src/server.rs:880`<br>`crates/memstead-mcp/src/server.rs:2451`<br>`crates/memstead-mcp/src/server.rs:2557`<br>`crates/memstead-mcp/src/server.rs:3690` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:241` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1983` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1270`<br>`crates/memstead-mcp/src/server.rs:907` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1271`<br>`crates/memstead-mcp/src/server.rs:950` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:240` |
| `UNSUPPORTED_PARAM` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:320` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:1023` |
| `UNTERMINATED_FENCE` | engine | `crates/memstead-base/src/runtime_validator.rs:248` |
| `UNTERMINATED_FENCE_IN_STORED_BODY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1316`<br>`crates/memstead-mcp/src/filesystem_server.rs:347`<br>`crates/memstead-mcp/src/server.rs:842` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1952` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1301`<br>`crates/memstead-mcp/src/server.rs:1581` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:50` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:633` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:539` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI | `crates/memstead-base/src/engine/error.rs:2276`<br>`crates/memstead-base/src/workspace_store.rs:161`<br>`crates/memstead-cli/src/commands/changes.rs:253`<br>`crates/memstead-cli/src/commands/publish.rs:497`<br>`crates/memstead-cli/src/setup.rs:41` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:168` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
