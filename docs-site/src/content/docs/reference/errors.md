---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 149

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1412` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:216`<br>`crates/memstead-cli/src/commands/install.rs:548`<br>`crates/memstead-cli/src/commands/type_cmd.rs:156` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:290`<br>`crates/memstead-cli/src/commands/publish.rs:174` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:274` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:267`<br>`crates/memstead-cli/src/commands/publish.rs:539` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1406` |
| `BATCH_REFUSED` | CLI | `crates/memstead-cli/src/commands/batch_update.rs:297` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:997`<br>`crates/memstead-mcp/src/server.rs:779` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1636` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:79`<br>`crates/memstead-cli/src/commands/overview.rs:150`<br>`crates/memstead-cli/src/commands/overview.rs:238`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1693` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1024`<br>`crates/memstead-mcp/src/server.rs:1011` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1008` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1006`<br>`crates/memstead-mcp/src/filesystem_server.rs:437` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1350` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1007`<br>`crates/memstead-mcp/src/filesystem_server.rs:446` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1044`<br>`crates/memstead-base/src/ops/mod.rs:1414`<br>`crates/memstead-mcp/src/server.rs:1403` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:290` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:314` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:988`<br>`crates/memstead-mcp/src/server.rs:1508` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1354` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1403` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1011`<br>`crates/memstead-mcp/src/server.rs:1576` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1001`<br>`crates/memstead-mcp/src/filesystem_server.rs:344`<br>`crates/memstead-mcp/src/server.rs:725` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1002`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:45`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:528`<br>`crates/memstead-cli/src/commands/update.rs:551`<br>`crates/memstead-mcp/src/filesystem_server.rs:348`<br>`crates/memstead-mcp/src/filesystem_server.rs:996`<br>`crates/memstead-mcp/src/filesystem_server.rs:1546`<br>`crates/memstead-mcp/src/server.rs:715`<br>`crates/memstead-mcp/src/server.rs:1776`<br>`crates/memstead-mcp/src/server.rs:2272` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1379` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1395` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1376` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1380` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:282` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1003`<br>`crates/memstead-mcp/src/server.rs:738` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1004` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:685` |
| `HOST_MEM_NOT_REGISTERED` | CLI | `crates/memstead-cli/src/commands/install.rs:523` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1400` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1349` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/commands/schema.rs:613`<br>`crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1510` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1041`<br>`crates/memstead-mcp/src/server.rs:1591` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1021`<br>`crates/memstead-mcp/src/server.rs:270`<br>`crates/memstead-mcp/src/server.rs:285`<br>`crates/memstead-mcp/src/server.rs:1233` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1381`<br>`crates/memstead-base/src/runtime_validator.rs:196` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:203` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1039`<br>`crates/memstead-base/src/engine/error.rs:1040`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/batch_update.rs:116`<br>`crates/memstead-cli/src/commands/batch_update.rs:127`<br>`crates/memstead-cli/src/commands/batch_update.rs:144`<br>`crates/memstead-cli/src/commands/batch_update.rs:160`<br>`crates/memstead-cli/src/commands/batch_update.rs:175`<br>`crates/memstead-cli/src/commands/batch_update.rs:329`<br>`crates/memstead-cli/src/commands/batch_update.rs:340`<br>`crates/memstead-cli/src/commands/batch_update.rs:466`<br>`crates/memstead-cli/src/commands/create.rs:127`<br>`crates/memstead-cli/src/commands/create.rs:134`<br>`crates/memstead-cli/src/commands/create.rs:147`<br>`crates/memstead-cli/src/commands/create.rs:154`<br>`crates/memstead-cli/src/commands/create.rs:286`<br>`crates/memstead-cli/src/commands/create.rs:294`<br>`crates/memstead-cli/src/commands/create.rs:359`<br>`crates/memstead-cli/src/commands/create.rs:374`<br>`crates/memstead-cli/src/commands/export.rs:64`<br>`crates/memstead-cli/src/commands/mod.rs:124`<br>`crates/memstead-cli/src/commands/mod.rs:131`<br>`crates/memstead-cli/src/commands/publish.rs:107`<br>`crates/memstead-cli/src/commands/publish.rs:115`<br>`crates/memstead-cli/src/commands/quickstart.rs:123`<br>`crates/memstead-cli/src/commands/quickstart.rs:338`<br>`crates/memstead-cli/src/commands/quickstart.rs:363`<br>`crates/memstead-cli/src/commands/quickstart.rs:371`<br>`crates/memstead-cli/src/commands/quickstart.rs:441`<br>`crates/memstead-cli/src/commands/quickstart.rs:601`<br>`crates/memstead-cli/src/commands/quickstart.rs:611`<br>`crates/memstead-cli/src/commands/quickstart.rs:623`<br>`crates/memstead-cli/src/commands/quickstart.rs:660`<br>`crates/memstead-cli/src/commands/relate.rs:77`<br>`crates/memstead-cli/src/commands/relate.rs:82`<br>`crates/memstead-cli/src/commands/schema.rs:106`<br>`crates/memstead-cli/src/commands/schema.rs:714`<br>`crates/memstead-cli/src/commands/schema.rs:746`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:165`<br>`crates/memstead-cli/src/commands/update.rs:172`<br>`crates/memstead-cli/src/commands/update.rs:185`<br>`crates/memstead-cli/src/commands/update.rs:322`<br>`crates/memstead-cli/src/commands/update.rs:330`<br>`crates/memstead-cli/src/commands/update.rs:338`<br>`crates/memstead-cli/src/commands/update.rs:587`<br>`crates/memstead-cli/src/commands/update.rs:594`<br>`crates/memstead-cli/src/commands/update.rs:616`<br>`crates/memstead-cli/src/commands/update.rs:635`<br>`crates/memstead-cli/src/commands/update.rs:642`<br>`crates/memstead-cli/src/commands/update.rs:649`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-mcp/src/filesystem_server.rs:1456`<br>`crates/memstead-mcp/src/filesystem_server.rs:1526`<br>`crates/memstead-mcp/src/filesystem_server.rs:1561`<br>`crates/memstead-mcp/src/filesystem_server.rs:1746`<br>`crates/memstead-mcp/src/server.rs:321`<br>`crates/memstead-mcp/src/server.rs:374`<br>`crates/memstead-mcp/src/server.rs:1345`<br>`crates/memstead-mcp/src/server.rs:1358`<br>`crates/memstead-mcp/src/server.rs:1944`<br>`crates/memstead-mcp/src/server.rs:2115`<br>`crates/memstead-mcp/src/server.rs:2157`<br>`crates/memstead-mcp/src/server.rs:2195`<br>`crates/memstead-mcp/src/server.rs:2211`<br>`crates/memstead-mcp/src/server.rs:2316`<br>`crates/memstead-mcp/src/server.rs:2926`<br>`crates/memstead-mcp/src/server.rs:3140`<br>`crates/memstead-mcp/src/server.rs:3197`<br>`crates/memstead-mcp/src/server.rs:3236`<br>`crates/memstead-mcp/src/server.rs:3265` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1023`<br>`crates/memstead-mcp/src/server.rs:1267`<br>`crates/memstead-mcp/src/server.rs:1647` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `INVALID_TITLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1000`<br>`crates/memstead-mcp/src/server.rs:1200` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:123` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1022`<br>`crates/memstead-mcp/src/server.rs:1248` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/schema.rs:142`<br>`crates/memstead-cli/src/commands/schema.rs:151`<br>`crates/memstead-cli/src/commands/schema.rs:176`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:806`<br>`crates/memstead-cli/src/commands/schema.rs:815` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1357` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:992`<br>`crates/memstead-mcp/src/server.rs:818` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:994`<br>`crates/memstead-mcp/src/server.rs:840` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:461` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1050`<br>`crates/memstead-mcp/src/server.rs:1563` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1042`<br>`crates/memstead-mcp/src/server.rs:1374` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1034`<br>`crates/memstead-base/src/engine/error.rs:1037`<br>`crates/memstead-cli/src/commands/workspace.rs:761`<br>`crates/memstead-cli/src/commands/workspace.rs:768`<br>`crates/memstead-mcp/src/filesystem_server.rs:813`<br>`crates/memstead-mcp/src/server.rs:1336`<br>`crates/memstead-mcp/src/server.rs:1538` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1410` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1005` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1038`<br>`crates/memstead-mcp/src/server.rs:1306` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1632` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1411` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1681` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1404` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:653` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1664` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1709` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1043`<br>`crates/memstead-base/src/ops/mod.rs:1413`<br>`crates/memstead-mcp/src/server.rs:1420` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1352` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/ops/mod.rs:1402` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1028`<br>`crates/memstead-base/src/ops/mod.rs:1351`<br>`crates/memstead-mcp/src/server.rs:1105` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1382` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:544`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:993`<br>`crates/memstead-mcp/src/server.rs:827` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1399` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:214`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1355` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/ingest.rs:60`<br>`crates/memstead-cli/src/commands/pipeline.rs:40`<br>`crates/memstead-cli/src/commands/schema.rs:558` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:397`<br>`crates/memstead-cli/src/commands/export.rs:209`<br>`crates/memstead-cli/src/commands/install.rs:541` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1401` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1408` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1032`<br>`crates/memstead-base/src/engine/error.rs:1033`<br>`crates/memstead-mcp/src/filesystem_server.rs:815`<br>`crates/memstead-mcp/src/filesystem_server.rs:817`<br>`crates/memstead-mcp/src/server.rs:1520`<br>`crates/memstead-mcp/src/server.rs:1529` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1030`<br>`crates/memstead-mcp/src/server.rs:1142` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1029`<br>`crates/memstead-mcp/src/filesystem_server.rs:789`<br>`crates/memstead-mcp/src/server.rs:1129` |
| `PIPELINE_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/ingest.rs:80` |
| `PIPELINE_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/pipeline.rs:48` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:996`<br>`crates/memstead-mcp/src/server.rs:796` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1384` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1392` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:475` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:197` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:998`<br>`crates/memstead-mcp/src/server.rs:870` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:554`<br>`crates/memstead-cli/src/commands/unpublish.rs:100` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:549`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1025`<br>`crates/memstead-mcp/src/server.rs:1029` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1016`<br>`crates/memstead-mcp/src/server.rs:1285` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1046`<br>`crates/memstead-mcp/src/server.rs:1438` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1013`<br>`crates/memstead-mcp/src/server.rs:1478` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1010`<br>`crates/memstead-mcp/src/filesystem_server.rs:488`<br>`crates/memstead-mcp/src/server.rs:1452` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1015`<br>`crates/memstead-mcp/src/server.rs:1495` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1009`<br>`crates/memstead-mcp/src/server.rs:1002` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1027`<br>`crates/memstead-mcp/src/server.rs:1071` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1409` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1035`<br>`crates/memstead-cli/src/commands/schema.rs:687`<br>`crates/memstead-cli/src/commands/schema.rs:721`<br>`crates/memstead-cli/src/commands/schema.rs:737`<br>`crates/memstead-mcp/src/server.rs:1318` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:126` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1405` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1036`<br>`crates/memstead-mcp/src/server.rs:1327` |
| `SCHEMA_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/schema.rs:543`<br>`crates/memstead-cli/src/commands/schema.rs:672` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:995`<br>`crates/memstead-mcp/src/server.rs:857` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1396` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1383` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1048`<br>`crates/memstead-mcp/src/server.rs:1547` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:201`<br>`crates/memstead-base/src/runtime_validator.rs:202` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1407` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1026`<br>`crates/memstead-mcp/src/server.rs:1120` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1018`<br>`crates/memstead-mcp/src/server.rs:1206` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1360` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1020`<br>`crates/memstead-mcp/src/server.rs:1224` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1019`<br>`crates/memstead-mcp/src/server.rs:1215` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1398` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:159`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1358` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1397` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:215` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1353` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:999`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:284`<br>`crates/memstead-mcp/src/server.rs:884` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1374` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1356` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:989`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:272`<br>`crates/memstead-cli/src/commands/export.rs:127`<br>`crates/memstead-cli/src/commands/export.rs:249`<br>`crates/memstead-mcp/src/filesystem_server.rs:1534`<br>`crates/memstead-mcp/src/server.rs:756`<br>`crates/memstead-mcp/src/server.rs:2133`<br>`crates/memstead-mcp/src/server.rs:2232`<br>`crates/memstead-mcp/src/server.rs:2908` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1390` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:990`<br>`crates/memstead-mcp/src/server.rs:766` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:991`<br>`crates/memstead-mcp/src/server.rs:809` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:194` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1359` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1017`<br>`crates/memstead-mcp/src/server.rs:1387` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:270` |
| `WORKSPACE_CONFIG_INVALID` | CLI | `crates/memstead-cli/src/commands/install.rs:283`<br>`crates/memstead-cli/src/commands/install.rs:294`<br>`crates/memstead-cli/src/commands/install.rs:348`<br>`crates/memstead-cli/src/commands/install.rs:359` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:275`<br>`crates/memstead-cli/src/commands/install.rs:334`<br>`crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_CONFIG_WRITE_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:398` |
| `WORKSPACE_NOT_INITIALISED` | CLI, MCP | `crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:270`<br>`crates/memstead-cli/src/commands/publish.rs:388`<br>`crates/memstead-cli/src/commands/publish.rs:412`<br>`crates/memstead-cli/src/commands/workspace.rs:735`<br>`crates/memstead-cli/src/setup.rs:40`<br>`crates/memstead-mcp/src/server.rs:3646` |
