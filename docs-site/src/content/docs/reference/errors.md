---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 245

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:2074` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:723`<br>`crates/memstead-cli/src/commands/publish.rs:242`<br>`crates/memstead-cli/src/commands/type_cmd.rs:226` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ANCHORS_SIDECAR_UNREADABLE` | CLI | `crates/memstead-cli/src/commands/anchors.rs:67`<br>`crates/memstead-cli/src/commands/projection.rs:2306`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:154` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:267`<br>`crates/memstead-cli/src/commands/publish.rs:341` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:384` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:377`<br>`crates/memstead-cli/src/commands/publish.rs:636` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:682`<br>`crates/memstead-cli/src/lib.rs:55` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:2061` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:117`<br>`crates/memstead-cli/src/commands/check.rs:344`<br>`crates/memstead-mcp/src/filesystem_server.rs:1680`<br>`crates/memstead-mcp/src/server.rs:3259` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1343`<br>`crates/memstead-mcp/src/server.rs:934` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:2234` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1345`<br>`crates/memstead-mcp/src/server.rs:1034` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:194`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:43` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1925` |
| `CONFIG_WRITE_INTERVENED` | engine | `crates/memstead-base/src/ops/mod.rs:2052` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1392`<br>`crates/memstead-mcp/src/filesystem_server.rs:756`<br>`crates/memstead-mcp/src/server.rs:1175` |
| `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND` | engine | `crates/memstead-base/src/engine/error.rs:1414` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1349`<br>`crates/memstead-base/src/engine/outcomes.rs:649`<br>`crates/memstead-base/src/ops/mod.rs:2048` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1359` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1356`<br>`crates/memstead-mcp/src/filesystem_server.rs:519` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1993` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1357`<br>`crates/memstead-mcp/src/filesystem_server.rs:528` |
| `CROSS_SCHEMA_LINK_UNDECLARED` | engine | `crates/memstead-base/src/ops/mod.rs:2064` |
| `DANGLING_LINK_NOT_RELATED` | engine | `crates/memstead-base/src/ops/mod.rs:3714` |
| `DANGLING_LINK_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3713` |
| `DANGLING_RELATION_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3715` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:2062` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1423`<br>`crates/memstead-base/src/ops/mod.rs:2076`<br>`crates/memstead-mcp/src/filesystem_server.rs:963`<br>`crates/memstead-mcp/src/server.rs:1611` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:400` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:424` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1332`<br>`crates/memstead-mcp/src/server.rs:1727` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1997` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:2049` |
| `EMBEDDED_SCHEMA_INVALID` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1407`<br>`crates/memstead-cli/src/commands/install.rs:273`<br>`crates/memstead-mcp/src/server.rs:1500` |
| `EMPTY_UNDECLARED_HEADING` | engine | `crates/memstead-base/src/runtime_validator.rs:248` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1379`<br>`crates/memstead-mcp/src/filesystem_server.rs:1066`<br>`crates/memstead-mcp/src/server.rs:1795` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:2057` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1348`<br>`crates/memstead-mcp/src/filesystem_server.rs:415`<br>`crates/memstead-mcp/src/server.rs:835` |
| `ENTITY_ID_MISSING_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1358`<br>`crates/memstead-mcp/src/server.rs:830` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1352`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:59`<br>`crates/memstead-cli/src/commands/delete.rs:88`<br>`crates/memstead-cli/src/commands/delete.rs:139`<br>`crates/memstead-cli/src/commands/delete.rs:163`<br>`crates/memstead-cli/src/commands/entity.rs:60`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:157`<br>`crates/memstead-cli/src/commands/rename.rs:191`<br>`crates/memstead-cli/src/commands/retype.rs:230`<br>`crates/memstead-cli/src/commands/update.rs:861`<br>`crates/memstead-cli/src/commands/update.rs:885`<br>`crates/memstead-mcp/src/filesystem_server.rs:430`<br>`crates/memstead-mcp/src/filesystem_server.rs:1135`<br>`crates/memstead-mcp/src/filesystem_server.rs:2100`<br>`crates/memstead-mcp/src/server.rs:820`<br>`crates/memstead-mcp/src/server.rs:2008`<br>`crates/memstead-mcp/src/server.rs:2642` |
| `EXPECTED_HASH_REQUIRED` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1433`<br>`crates/memstead-mcp/src/server.rs:2996` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:2023` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:2039` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:2020` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:2024` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:228` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:2070` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:645` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:34` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1353`<br>`crates/memstead-mcp/src/server.rs:867` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1354` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:947` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:2044` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1992` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:29`<br>`crates/memstead-mcp/src/filesystem_server.rs:1958`<br>`crates/memstead-mcp/src/filesystem_server.rs:2034`<br>`crates/memstead-mcp/src/filesystem_server.rs:2064` |
| `INTERNAL_IO_ERROR` | CLI | `crates/memstead-cli/src/commands/install.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:230`<br>`crates/memstead-cli/src/commands/quickstart.rs:370`<br>`crates/memstead-cli/src/commands/quickstart.rs:674`<br>`crates/memstead-cli/src/commands/quickstart.rs:857`<br>`crates/memstead-cli/src/commands/quickstart.rs:986`<br>`crates/memstead-cli/src/commands/quickstart.rs:1096`<br>`crates/memstead-cli/src/commands/quickstart.rs:1108`<br>`crates/memstead-cli/src/setup.rs:715` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:73` |
| `INVALID_CHECK_FINDING` | engine | `crates/memstead-base/src/check.rs:65` |
| `INVALID_CHECK_KIND` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:109`<br>`crates/memstead-mcp/src/filesystem_server.rs:2454`<br>`crates/memstead-mcp/src/server.rs:3505` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1418`<br>`crates/memstead-base/src/engine/error.rs:1419`<br>`crates/memstead-mcp/src/filesystem_server.rs:1083`<br>`crates/memstead-mcp/src/filesystem_server.rs:2259`<br>`crates/memstead-mcp/src/server.rs:1810` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1389`<br>`crates/memstead-mcp/src/filesystem_server.rs:707`<br>`crates/memstead-mcp/src/server.rs:361`<br>`crates/memstead-mcp/src/server.rs:376`<br>`crates/memstead-mcp/src/server.rs:1410` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:2025`<br>`crates/memstead-base/src/runtime_validator.rs:243` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:252` |
| `INVALID_IDENTITY` | CLI, MCP | `crates/memstead-cli/src/main.rs:131`<br>`crates/memstead-mcp/src/filesystem_server.rs:273`<br>`crates/memstead-mcp/src/server.rs:210` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1412`<br>`crates/memstead-base/src/engine/error.rs:1417`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:146`<br>`crates/memstead-cli/src/commands/batch.rs:153`<br>`crates/memstead-cli/src/commands/batch.rs:170`<br>`crates/memstead-cli/src/commands/batch.rs:187`<br>`crates/memstead-cli/src/commands/batch.rs:202`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:208`<br>`crates/memstead-cli/src/commands/batch_relate.rs:83`<br>`crates/memstead-cli/src/commands/batch_update.rs:262`<br>`crates/memstead-cli/src/commands/batch_update.rs:273`<br>`crates/memstead-cli/src/commands/batch_update.rs:292`<br>`crates/memstead-cli/src/commands/batch_update.rs:429`<br>`crates/memstead-cli/src/commands/check.rs:252`<br>`crates/memstead-cli/src/commands/check.rs:259`<br>`crates/memstead-cli/src/commands/check.rs:269`<br>`crates/memstead-cli/src/commands/conflicts.rs:100`<br>`crates/memstead-cli/src/commands/create.rs:168`<br>`crates/memstead-cli/src/commands/create.rs:175`<br>`crates/memstead-cli/src/commands/create.rs:191`<br>`crates/memstead-cli/src/commands/create.rs:198`<br>`crates/memstead-cli/src/commands/create.rs:238`<br>`crates/memstead-cli/src/commands/create.rs:376`<br>`crates/memstead-cli/src/commands/create.rs:460`<br>`crates/memstead-cli/src/commands/create.rs:483`<br>`crates/memstead-cli/src/commands/create.rs:498`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/due.rs:55`<br>`crates/memstead-cli/src/commands/export.rs:180`<br>`crates/memstead-cli/src/commands/export.rs:189`<br>`crates/memstead-cli/src/commands/export.rs:199`<br>`crates/memstead-cli/src/commands/export.rs:208`<br>`crates/memstead-cli/src/commands/export.rs:239`<br>`crates/memstead-cli/src/commands/export.rs:271`<br>`crates/memstead-cli/src/commands/export.rs:285`<br>`crates/memstead-cli/src/commands/export.rs:984`<br>`crates/memstead-cli/src/commands/export.rs:989`<br>`crates/memstead-cli/src/commands/export.rs:1026`<br>`crates/memstead-cli/src/commands/export.rs:1034`<br>`crates/memstead-cli/src/commands/health.rs:166`<br>`crates/memstead-cli/src/commands/install.rs:69`<br>`crates/memstead-cli/src/commands/mem.rs:1151`<br>`crates/memstead-cli/src/commands/mod.rs:113`<br>`crates/memstead-cli/src/commands/mod.rs:120`<br>`crates/memstead-cli/src/commands/projection.rs:1893`<br>`crates/memstead-cli/src/commands/publish.rs:128`<br>`crates/memstead-cli/src/commands/publish.rs:136`<br>`crates/memstead-cli/src/commands/publish.rs:158`<br>`crates/memstead-cli/src/commands/quickstart.rs:192`<br>`crates/memstead-cli/src/commands/quickstart.rs:212`<br>`crates/memstead-cli/src/commands/quickstart.rs:726`<br>`crates/memstead-cli/src/commands/quickstart.rs:750`<br>`crates/memstead-cli/src/commands/quickstart.rs:758`<br>`crates/memstead-cli/src/commands/quickstart.rs:829`<br>`crates/memstead-cli/src/commands/quickstart.rs:993`<br>`crates/memstead-cli/src/commands/quickstart.rs:1003`<br>`crates/memstead-cli/src/commands/quickstart.rs:1015`<br>`crates/memstead-cli/src/commands/quickstart.rs:1066`<br>`crates/memstead-cli/src/commands/relate.rs:88`<br>`crates/memstead-cli/src/commands/relate.rs:93`<br>`crates/memstead-cli/src/commands/retype.rs:92`<br>`crates/memstead-cli/src/commands/retype.rs:101`<br>`crates/memstead-cli/src/commands/schema.rs:192`<br>`crates/memstead-cli/src/commands/schema.rs:327`<br>`crates/memstead-cli/src/commands/schema.rs:365`<br>`crates/memstead-cli/src/commands/schema.rs:1299`<br>`crates/memstead-cli/src/commands/schema.rs:1331`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:195`<br>`crates/memstead-cli/src/commands/update.rs:357`<br>`crates/memstead-cli/src/commands/update.rs:370`<br>`crates/memstead-cli/src/commands/update.rs:386`<br>`crates/memstead-cli/src/commands/update.rs:393`<br>`crates/memstead-cli/src/commands/update.rs:414`<br>`crates/memstead-cli/src/commands/update.rs:457`<br>`crates/memstead-cli/src/commands/update.rs:621`<br>`crates/memstead-cli/src/commands/update.rs:629`<br>`crates/memstead-cli/src/commands/update.rs:637`<br>`crates/memstead-cli/src/commands/update.rs:943`<br>`crates/memstead-cli/src/commands/update.rs:950`<br>`crates/memstead-cli/src/commands/update.rs:972`<br>`crates/memstead-cli/src/commands/update.rs:992`<br>`crates/memstead-cli/src/commands/update.rs:999`<br>`crates/memstead-cli/src/commands/update.rs:1010`<br>`crates/memstead-cli/src/commands/workspace.rs:717`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:1014`<br>`crates/memstead-mcp/src/filesystem_server.rs:1553`<br>`crates/memstead-mcp/src/filesystem_server.rs:1885`<br>`crates/memstead-mcp/src/filesystem_server.rs:2080`<br>`crates/memstead-mcp/src/filesystem_server.rs:2115`<br>`crates/memstead-mcp/src/filesystem_server.rs:2382`<br>`crates/memstead-mcp/src/filesystem_server.rs:2571`<br>`crates/memstead-mcp/src/server.rs:412`<br>`crates/memstead-mcp/src/server.rs:465`<br>`crates/memstead-mcp/src/server.rs:1543`<br>`crates/memstead-mcp/src/server.rs:1566`<br>`crates/memstead-mcp/src/server.rs:2290`<br>`crates/memstead-mcp/src/server.rs:2474`<br>`crates/memstead-mcp/src/server.rs:2520`<br>`crates/memstead-mcp/src/server.rs:2557`<br>`crates/memstead-mcp/src/server.rs:2573`<br>`crates/memstead-mcp/src/server.rs:2686`<br>`crates/memstead-mcp/src/server.rs:3101`<br>`crates/memstead-mcp/src/server.rs:3700`<br>`crates/memstead-mcp/src/server.rs:3881`<br>`crates/memstead-mcp/src/server.rs:4024`<br>`crates/memstead-mcp/src/server.rs:4117`<br>`crates/memstead-mcp/src/server.rs:4174`<br>`crates/memstead-mcp/src/server.rs:4273`<br>`crates/memstead-mcp/src/server.rs:4312`<br>`crates/memstead-mcp/src/server.rs:4341` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1391`<br>`crates/memstead-mcp/src/filesystem_server.rs:743`<br>`crates/memstead-mcp/src/server.rs:1444`<br>`crates/memstead-mcp/src/server.rs:1879` |
| `INVALID_OBSERVATION` | engine, CLI | `crates/memstead-base/src/anchor.rs:1176`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:89`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:96`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:108`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:122` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/engine/outcomes.rs:647`<br>`crates/memstead-base/src/runtime_validator.rs:247` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:246` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/filesystem_server.rs:294`<br>`crates/memstead-mcp/src/server.rs:242` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:523` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1347`<br>`crates/memstead-cli/src/commands/batch_create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:229`<br>`crates/memstead-mcp/src/filesystem_server.rs:409`<br>`crates/memstead-mcp/src/server.rs:1377` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:154`<br>`crates/memstead-mcp/src/filesystem_server.rs:2437`<br>`crates/memstead-mcp/src/server.rs:3484` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:144` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1390`<br>`crates/memstead-mcp/src/filesystem_server.rs:723`<br>`crates/memstead-mcp/src/server.rs:1425` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:943`<br>`crates/memstead-cli/src/commands/export.rs:1070`<br>`crates/memstead-cli/src/commands/schema.rs:401`<br>`crates/memstead-cli/src/commands/schema.rs:410`<br>`crates/memstead-cli/src/commands/schema.rs:435`<br>`crates/memstead-cli/src/commands/schema.rs:447`<br>`crates/memstead-cli/src/commands/schema.rs:1411`<br>`crates/memstead-cli/src/commands/schema.rs:1420` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:165` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:2000` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1338`<br>`crates/memstead-mcp/src/server.rs:973` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1340`<br>`crates/memstead-mcp/src/server.rs:995` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:558` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1429`<br>`crates/memstead-mcp/src/filesystem_server.rs:1057`<br>`crates/memstead-mcp/src/server.rs:1782` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1421`<br>`crates/memstead-mcp/src/filesystem_server.rs:1033`<br>`crates/memstead-mcp/src/server.rs:1582` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1405`<br>`crates/memstead-base/src/engine/error.rs:1410`<br>`crates/memstead-cli/src/commands/workspace.rs:900`<br>`crates/memstead-cli/src/commands/workspace.rs:907`<br>`crates/memstead-mcp/src/filesystem_server.rs:926`<br>`crates/memstead-mcp/src/server.rs:1534`<br>`crates/memstead-mcp/src/server.rs:1757` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:2067` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1355` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1411`<br>`crates/memstead-mcp/src/server.rs:1483` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:53` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1874` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1335`<br>`crates/memstead-mcp/src/server.rs:908` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:2068` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1913` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:2050` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:1175` |
| `MEM_ROSTER_CHANGED` | engine | `crates/memstead-base/src/ops/mod.rs:2051` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1896` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1941` |
| `MEM_UNMOUNTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1334`<br>`crates/memstead-mcp/src/server.rs:895` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1422`<br>`crates/memstead-base/src/ops/mod.rs:2075`<br>`crates/memstead-mcp/src/filesystem_server.rs:950`<br>`crates/memstead-mcp/src/server.rs:1628` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1995` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1350`<br>`crates/memstead-base/src/engine/outcomes.rs:648`<br>`crates/memstead-base/src/ops/mod.rs:2046` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1396`<br>`crates/memstead-base/src/engine/outcomes.rs:642`<br>`crates/memstead-base/src/ops/mod.rs:1994`<br>`crates/memstead-mcp/src/filesystem_server.rs:897`<br>`crates/memstead-mcp/src/server.rs:1303` |
| `MOUNT_UNBACKED` | engine | `crates/memstead-base/src/ops/mod.rs:2056` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:2026` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:641`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1339`<br>`crates/memstead-mcp/src/server.rs:982` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:2043` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:308`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NOT_CONFLICTED` | engine | `crates/memstead-base/src/engine/error.rs:1416` |
| `NO_ACTIVE_BINDING` | CLI | `crates/memstead-cli/src/commands/projection.rs:1834` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1998` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:1070` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:183`<br>`crates/memstead-cli/src/commands/changes.rs:72`<br>`crates/memstead-cli/src/commands/create.rs:521`<br>`crates/memstead-cli/src/commands/export.rs:716` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:2045` |
| `OUT_OF_BAND_EDITS_UNDETECTED` | engine | `crates/memstead-base/src/ops/mod.rs:2054` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:2065` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1403`<br>`crates/memstead-base/src/engine/error.rs:1404`<br>`crates/memstead-mcp/src/filesystem_server.rs:928`<br>`crates/memstead-mcp/src/filesystem_server.rs:930`<br>`crates/memstead-mcp/src/server.rs:1739`<br>`crates/memstead-mcp/src/server.rs:1748` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1398`<br>`crates/memstead-mcp/src/filesystem_server.rs:911`<br>`crates/memstead-mcp/src/server.rs:1332` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1397`<br>`crates/memstead-mcp/src/filesystem_server.rs:900`<br>`crates/memstead-mcp/src/server.rs:1318` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1915`<br>`crates/memstead-cli/src/commands/projection.rs:1960`<br>`crates/memstead-cli/src/commands/projection.rs:1995` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1950` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:697` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:637` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:600`<br>`crates/memstead-cli/src/commands/projection.rs:1629`<br>`crates/memstead-cli/src/commands/projection.rs:2357` |
| `PROJECTION_EDIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1776` |
| `PROJECTION_EDIT_INVALID_JSON` | CLI | `crates/memstead-cli/src/commands/projection.rs:1753` |
| `PROJECTION_EDIT_REFUSED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1759` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1497` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2116`<br>`crates/memstead-cli/src/commands/projection.rs:2150` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:2089` |
| `PROJECTION_EXCLUDE_PARTIAL_ENUMERATION` | CLI | `crates/memstead-cli/src/commands/projection.rs:2111` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:930` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:643` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:879`<br>`crates/memstead-cli/src/commands/quickstart.rs:596` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1981` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2137` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:651`<br>`crates/memstead-cli/src/commands/projection.rs:904`<br>`crates/memstead-cli/src/commands/projection.rs:1480`<br>`crates/memstead-cli/src/commands/projection.rs:1908`<br>`crates/memstead-cli/src/commands/projection.rs:1928`<br>`crates/memstead-cli/src/commands/projection.rs:2082` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:631`<br>`crates/memstead-cli/src/commands/projection.rs:714`<br>`crates/memstead-cli/src/commands/projection.rs:760`<br>`crates/memstead-cli/src/commands/projection.rs:1844` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:1041` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1067`<br>`crates/memstead-cli/src/commands/projection.rs:1262`<br>`crates/memstead-cli/src/commands/projection.rs:1374`<br>`crates/memstead-cli/src/commands/projection.rs:1383`<br>`crates/memstead-cli/src/commands/projection.rs:1393` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:1314` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:1034` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1046` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1029` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:648`<br>`crates/memstead-cli/src/commands/projection.rs:1151`<br>`crates/memstead-cli/src/commands/projection.rs:1535`<br>`crates/memstead-cli/src/commands/projection.rs:1744` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1570` |
| `PROJECTION_QUARANTINED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1135` |
| `PROJECTION_SCOPE_UNINTERPRETABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:658`<br>`crates/memstead-cli/src/commands/projection.rs:1912` |
| `PROJECTION_STORE_LEGACY` | engine | `crates/memstead-base/src/workspace_store.rs:166` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:610` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2506` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2518` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2276`<br>`crates/memstead-cli/src/commands/projection.rs:2368` |
| `PROJECTION_VERIFY_FINDINGS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2539` |
| `PROJECTION_VERIFY_INCONCLUSIVE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2566` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1342`<br>`crates/memstead-mcp/src/server.rs:951` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:2028` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:2036` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:2069` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:285` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:244` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1344`<br>`crates/memstead-mcp/src/server.rs:1025` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:651`<br>`crates/memstead-cli/src/commands/unpublish.rs:100`<br>`crates/memstead-cli/src/registry/mod.rs:92` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:646`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1393`<br>`crates/memstead-mcp/src/filesystem_server.rs:794`<br>`crates/memstead-mcp/src/server.rs:1210` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1384`<br>`crates/memstead-mcp/src/server.rs:1462` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1425`<br>`crates/memstead-mcp/src/filesystem_server.rs:977`<br>`crates/memstead-mcp/src/server.rs:1646` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1381`<br>`crates/memstead-mcp/src/server.rs:1697` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1361`<br>`crates/memstead-mcp/src/filesystem_server.rs:570`<br>`crates/memstead-mcp/src/server.rs:1671` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1383`<br>`crates/memstead-mcp/src/server.rs:1714` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1360`<br>`crates/memstead-mcp/src/server.rs:1166` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1395`<br>`crates/memstead-base/src/engine/outcomes.rs:643`<br>`crates/memstead-mcp/src/filesystem_server.rs:839`<br>`crates/memstead-mcp/src/server.rs:1242` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:2066` |
| `RETYPE_NO_OP` | engine | `crates/memstead-base/src/engine/error.rs:1376` |
| `RETYPE_REFERRER_UNPROBEABLE` | engine | `crates/memstead-base/src/engine/error.rs:1378` |
| `RETYPE_REFUSED` | engine | `crates/memstead-base/src/engine/error.rs:1373` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1420`<br>`crates/memstead-mcp/src/filesystem_server.rs:1089`<br>`crates/memstead-mcp/src/server.rs:1822` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:2072` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:2071` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:2058` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:2059` |
| `SCHEMA_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/schema.rs:937` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1406`<br>`crates/memstead-cli/src/commands/schema.rs:238`<br>`crates/memstead-cli/src/commands/schema.rs:340`<br>`crates/memstead-cli/src/commands/schema.rs:1272`<br>`crates/memstead-cli/src/commands/schema.rs:1306`<br>`crates/memstead-cli/src/commands/schema.rs:1322`<br>`crates/memstead-mcp/src/server.rs:1495` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:385` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:2055` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1409`<br>`crates/memstead-mcp/src/server.rs:1525` |
| `SCHEMA_UNSTAMPED_SOURCE_ROT` | engine | `crates/memstead-base/src/ops/mod.rs:2073` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1408`<br>`crates/memstead-cli/src/commands/schema.rs:858`<br>`crates/memstead-cli/src/commands/schema.rs:886`<br>`crates/memstead-cli/src/commands/schema.rs:911`<br>`crates/memstead-cli/src/commands/schema.rs:1209`<br>`crates/memstead-cli/src/commands/schema.rs:1221`<br>`crates/memstead-mcp/src/server.rs:1513` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1341`<br>`crates/memstead-mcp/src/server.rs:1012` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:2040` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:2027` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1427`<br>`crates/memstead-mcp/src/filesystem_server.rs:1043`<br>`crates/memstead-mcp/src/server.rs:1766` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:250`<br>`crates/memstead-base/src/runtime_validator.rs:251`<br>`crates/memstead-base/src/section_format.rs:524` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:2060` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:522` |
| `SECTION_MAP_COLLISION` | engine | `crates/memstead-base/src/engine/outcomes.rs:646` |
| `SECTION_MAP_SOURCE_MISSING` | engine | `crates/memstead-base/src/engine/outcomes.rs:645` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:245` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:2063` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1394`<br>`crates/memstead-mcp/src/filesystem_server.rs:799`<br>`crates/memstead-mcp/src/server.rs:1309` |
| `SHORT_ID_RESOLVED` | engine | `crates/memstead-base/src/ops/mod.rs:2053` |
| `SIGNAL_THRESHOLD_CROSSED` | engine | `crates/memstead-base/src/ops/mod.rs:2047` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2336` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1386`<br>`crates/memstead-mcp/src/server.rs:1383` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:2004` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1388`<br>`crates/memstead-mcp/src/server.rs:1401` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1387`<br>`crates/memstead-mcp/src/server.rs:1392` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:2042` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:418`<br>`crates/memstead-cli/src/lib.rs:39` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:2002` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:2001` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:2041` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:306` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1996` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:167` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1346`<br>`crates/memstead-cli/src/commands/type_cmd.rs:79`<br>`crates/memstead-mcp/src/filesystem_server.rs:369`<br>`crates/memstead-mcp/src/filesystem_server.rs:2150`<br>`crates/memstead-mcp/src/server.rs:1048`<br>`crates/memstead-mcp/src/server.rs:2728` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:2018` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1999` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1333`<br>`crates/memstead-cli/src/commands/changes.rs:232`<br>`crates/memstead-cli/src/commands/create.rs:353`<br>`crates/memstead-cli/src/commands/export.rs:327`<br>`crates/memstead-cli/src/commands/export.rs:531`<br>`crates/memstead-cli/src/commands/export.rs:756`<br>`crates/memstead-cli/src/commands/uninstall.rs:41`<br>`crates/memstead-mcp/src/filesystem_server.rs:2088`<br>`crates/memstead-mcp/src/filesystem_server.rs:2583`<br>`crates/memstead-mcp/src/server.rs:885`<br>`crates/memstead-mcp/src/server.rs:2496`<br>`crates/memstead-mcp/src/server.rs:2602`<br>`crates/memstead-mcp/src/server.rs:3863` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:242` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:2034` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1336`<br>`crates/memstead-mcp/src/server.rs:921` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1337`<br>`crates/memstead-mcp/src/server.rs:964` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/engine/outcomes.rs:641`<br>`crates/memstead-base/src/runtime_validator.rs:241` |
| `UNRESOLVED_STUB` | engine | `crates/memstead-base/src/ops/integrity.rs:394` |
| `UNSUPPORTED_PARAM` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:320` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:1023` |
| `UNTERMINATED_FENCE` | engine | `crates/memstead-base/src/runtime_validator.rs:249` |
| `UNTERMINATED_FENCE_IN_STORED_BODY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1400`<br>`crates/memstead-mcp/src/filesystem_server.rs:354`<br>`crates/memstead-mcp/src/server.rs:847` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:2003` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1385`<br>`crates/memstead-mcp/src/server.rs:1595` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:50` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:633` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:539` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI | `crates/memstead-base/src/engine/error.rs:2406`<br>`crates/memstead-base/src/workspace_store.rs:161`<br>`crates/memstead-cli/src/commands/changes.rs:253`<br>`crates/memstead-cli/src/commands/publish.rs:497`<br>`crates/memstead-cli/src/setup.rs:41` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:168` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
