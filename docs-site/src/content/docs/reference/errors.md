---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 176

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1412` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:216`<br>`crates/memstead-cli/src/commands/install.rs:548`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:290`<br>`crates/memstead-cli/src/commands/publish.rs:170` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:270` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:263`<br>`crates/memstead-cli/src/commands/publish.rs:535` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1406` |
| `BATCH_REFUSED` | CLI | `crates/memstead-cli/src/commands/batch_update.rs:303` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1018`<br>`crates/memstead-mcp/src/server.rs:777` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1663` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:79`<br>`crates/memstead-cli/src/commands/overview.rs:145`<br>`crates/memstead-cli/src/commands/overview.rs:231`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1715` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1045`<br>`crates/memstead-mcp/src/server.rs:1009` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1029` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1027`<br>`crates/memstead-mcp/src/filesystem_server.rs:437` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1350` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1028`<br>`crates/memstead-mcp/src/filesystem_server.rs:446` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1066`<br>`crates/memstead-base/src/ops/mod.rs:1414`<br>`crates/memstead-mcp/src/server.rs:1401` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:286` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:310` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1009`<br>`crates/memstead-mcp/src/server.rs:1506` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1354` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1403` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1032`<br>`crates/memstead-mcp/src/server.rs:1574` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1022`<br>`crates/memstead-mcp/src/filesystem_server.rs:344`<br>`crates/memstead-mcp/src/server.rs:723` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1023`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:45`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:572`<br>`crates/memstead-cli/src/commands/update.rs:595`<br>`crates/memstead-mcp/src/filesystem_server.rs:348`<br>`crates/memstead-mcp/src/filesystem_server.rs:1011`<br>`crates/memstead-mcp/src/filesystem_server.rs:1573`<br>`crates/memstead-mcp/src/server.rs:713`<br>`crates/memstead-mcp/src/server.rs:1798`<br>`crates/memstead-mcp/src/server.rs:2318` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1379` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1395` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1376` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1380` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:282` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1024`<br>`crates/memstead-mcp/src/server.rs:736` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1025` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:685` |
| `HOST_MEM_NOT_REGISTERED` | CLI | `crates/memstead-cli/src/commands/install.rs:523` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1400` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1349` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/commands/schema.rs:613`<br>`crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1537` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1062`<br>`crates/memstead-mcp/src/server.rs:1589` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1042`<br>`crates/memstead-mcp/src/server.rs:268`<br>`crates/memstead-mcp/src/server.rs:283`<br>`crates/memstead-mcp/src/server.rs:1231` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1381`<br>`crates/memstead-base/src/runtime_validator.rs:196` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:203` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1060`<br>`crates/memstead-base/src/engine/error.rs:1061`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch_update.rs:122`<br>`crates/memstead-cli/src/commands/batch_update.rs:133`<br>`crates/memstead-cli/src/commands/batch_update.rs:150`<br>`crates/memstead-cli/src/commands/batch_update.rs:166`<br>`crates/memstead-cli/src/commands/batch_update.rs:181`<br>`crates/memstead-cli/src/commands/batch_update.rs:335`<br>`crates/memstead-cli/src/commands/batch_update.rs:346`<br>`crates/memstead-cli/src/commands/batch_update.rs:473`<br>`crates/memstead-cli/src/commands/create.rs:141`<br>`crates/memstead-cli/src/commands/create.rs:148`<br>`crates/memstead-cli/src/commands/create.rs:161`<br>`crates/memstead-cli/src/commands/create.rs:168`<br>`crates/memstead-cli/src/commands/create.rs:302`<br>`crates/memstead-cli/src/commands/create.rs:310`<br>`crates/memstead-cli/src/commands/create.rs:376`<br>`crates/memstead-cli/src/commands/create.rs:399`<br>`crates/memstead-cli/src/commands/create.rs:414`<br>`crates/memstead-cli/src/commands/export.rs:64`<br>`crates/memstead-cli/src/commands/mod.rs:125`<br>`crates/memstead-cli/src/commands/mod.rs:132`<br>`crates/memstead-cli/src/commands/publish.rs:107`<br>`crates/memstead-cli/src/commands/publish.rs:115`<br>`crates/memstead-cli/src/commands/quickstart.rs:123`<br>`crates/memstead-cli/src/commands/quickstart.rs:338`<br>`crates/memstead-cli/src/commands/quickstart.rs:363`<br>`crates/memstead-cli/src/commands/quickstart.rs:371`<br>`crates/memstead-cli/src/commands/quickstart.rs:441`<br>`crates/memstead-cli/src/commands/quickstart.rs:602`<br>`crates/memstead-cli/src/commands/quickstart.rs:612`<br>`crates/memstead-cli/src/commands/quickstart.rs:624`<br>`crates/memstead-cli/src/commands/quickstart.rs:661`<br>`crates/memstead-cli/src/commands/relate.rs:77`<br>`crates/memstead-cli/src/commands/relate.rs:82`<br>`crates/memstead-cli/src/commands/schema.rs:106`<br>`crates/memstead-cli/src/commands/schema.rs:714`<br>`crates/memstead-cli/src/commands/schema.rs:746`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:198`<br>`crates/memstead-cli/src/commands/update.rs:205`<br>`crates/memstead-cli/src/commands/update.rs:226`<br>`crates/memstead-cli/src/commands/update.rs:365`<br>`crates/memstead-cli/src/commands/update.rs:373`<br>`crates/memstead-cli/src/commands/update.rs:381`<br>`crates/memstead-cli/src/commands/update.rs:631`<br>`crates/memstead-cli/src/commands/update.rs:638`<br>`crates/memstead-cli/src/commands/update.rs:660`<br>`crates/memstead-cli/src/commands/update.rs:679`<br>`crates/memstead-cli/src/commands/update.rs:686`<br>`crates/memstead-cli/src/commands/update.rs:693`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-mcp/src/filesystem_server.rs:1483`<br>`crates/memstead-mcp/src/filesystem_server.rs:1553`<br>`crates/memstead-mcp/src/filesystem_server.rs:1588`<br>`crates/memstead-mcp/src/filesystem_server.rs:1773`<br>`crates/memstead-mcp/src/server.rs:319`<br>`crates/memstead-mcp/src/server.rs:372`<br>`crates/memstead-mcp/src/server.rs:1343`<br>`crates/memstead-mcp/src/server.rs:1356`<br>`crates/memstead-mcp/src/server.rs:1990`<br>`crates/memstead-mcp/src/server.rs:2161`<br>`crates/memstead-mcp/src/server.rs:2203`<br>`crates/memstead-mcp/src/server.rs:2241`<br>`crates/memstead-mcp/src/server.rs:2257`<br>`crates/memstead-mcp/src/server.rs:2362`<br>`crates/memstead-mcp/src/server.rs:2986`<br>`crates/memstead-mcp/src/server.rs:3200`<br>`crates/memstead-mcp/src/server.rs:3257`<br>`crates/memstead-mcp/src/server.rs:3296`<br>`crates/memstead-mcp/src/server.rs:3325` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1044`<br>`crates/memstead-mcp/src/server.rs:1265`<br>`crates/memstead-mcp/src/server.rs:1669` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `INVALID_TITLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1021`<br>`crates/memstead-mcp/src/server.rs:1198` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:123` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1043`<br>`crates/memstead-mcp/src/server.rs:1246` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/schema.rs:142`<br>`crates/memstead-cli/src/commands/schema.rs:151`<br>`crates/memstead-cli/src/commands/schema.rs:176`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:806`<br>`crates/memstead-cli/src/commands/schema.rs:815` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1357` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1013`<br>`crates/memstead-mcp/src/server.rs:816` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1015`<br>`crates/memstead-mcp/src/server.rs:838` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:457` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1072`<br>`crates/memstead-mcp/src/server.rs:1561` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1064`<br>`crates/memstead-mcp/src/server.rs:1372` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1055`<br>`crates/memstead-base/src/engine/error.rs:1058`<br>`crates/memstead-cli/src/commands/workspace.rs:761`<br>`crates/memstead-cli/src/commands/workspace.rs:768`<br>`crates/memstead-mcp/src/filesystem_server.rs:813`<br>`crates/memstead-mcp/src/server.rs:1334`<br>`crates/memstead-mcp/src/server.rs:1536` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1410` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1026` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1059`<br>`crates/memstead-mcp/src/server.rs:1304` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1654` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1411` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1703` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1404` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:653` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1686` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1731` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1065`<br>`crates/memstead-base/src/ops/mod.rs:1413`<br>`crates/memstead-mcp/src/server.rs:1418` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1352` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/ops/mod.rs:1402` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1049`<br>`crates/memstead-base/src/ops/mod.rs:1351`<br>`crates/memstead-mcp/src/server.rs:1103` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1382` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:540`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1014`<br>`crates/memstead-mcp/src/server.rs:825` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1399` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:210`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1355` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:558` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:437`<br>`crates/memstead-cli/src/commands/export.rs:209`<br>`crates/memstead-cli/src/commands/install.rs:541` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1401` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1408` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1053`<br>`crates/memstead-base/src/engine/error.rs:1054`<br>`crates/memstead-mcp/src/filesystem_server.rs:815`<br>`crates/memstead-mcp/src/filesystem_server.rs:817`<br>`crates/memstead-mcp/src/server.rs:1518`<br>`crates/memstead-mcp/src/server.rs:1527` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1051`<br>`crates/memstead-mcp/src/server.rs:1140` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1050`<br>`crates/memstead-mcp/src/filesystem_server.rs:789`<br>`crates/memstead-mcp/src/server.rs:1127` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1326`<br>`crates/memstead-cli/src/commands/projection.rs:1371`<br>`crates/memstead-cli/src/commands/projection.rs:1406` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1361` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:446` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:408` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1281`<br>`crates/memstead-cli/src/commands/projection.rs:1718` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1160` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1512`<br>`crates/memstead-cli/src/commands/projection.rs:1546` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:1507` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:623` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:414` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:572` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1392` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1533` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:422`<br>`crates/memstead-cli/src/commands/projection.rs:597`<br>`crates/memstead-cli/src/commands/projection.rs:1143`<br>`crates/memstead-cli/src/commands/projection.rs:1324`<br>`crates/memstead-cli/src/commands/projection.rs:1339`<br>`crates/memstead-cli/src/commands/projection.rs:1502` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:402`<br>`crates/memstead-cli/src/commands/projection.rs:486` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:771` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:797`<br>`crates/memstead-cli/src/commands/projection.rs:929`<br>`crates/memstead-cli/src/commands/projection.rs:1041`<br>`crates/memstead-cli/src/commands/projection.rs:1050`<br>`crates/memstead-cli/src/commands/projection.rs:1060` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:981` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:764` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:776` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:759` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:419`<br>`crates/memstead-cli/src/commands/projection.rs:1198`<br>`crates/memstead-cli/src/commands/projection.rs:1418`<br>`crates/memstead-cli/src/commands/projection.rs:1558`<br>`crates/memstead-cli/src/commands/projection.rs:1668` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1222` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1433` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1750` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1777` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1656`<br>`crates/memstead-cli/src/commands/projection.rs:1729` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1017`<br>`crates/memstead-mcp/src/server.rs:794` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1384` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1392` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:475` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:197` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1019`<br>`crates/memstead-mcp/src/server.rs:868` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:550`<br>`crates/memstead-cli/src/commands/unpublish.rs:100` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:545`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1046`<br>`crates/memstead-mcp/src/server.rs:1027` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1037`<br>`crates/memstead-mcp/src/server.rs:1283` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1068`<br>`crates/memstead-mcp/src/server.rs:1436` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1034`<br>`crates/memstead-mcp/src/server.rs:1476` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1031`<br>`crates/memstead-mcp/src/filesystem_server.rs:488`<br>`crates/memstead-mcp/src/server.rs:1450` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1036`<br>`crates/memstead-mcp/src/server.rs:1493` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1030`<br>`crates/memstead-mcp/src/server.rs:1000` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1048`<br>`crates/memstead-mcp/src/server.rs:1069` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1409` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1063`<br>`crates/memstead-mcp/src/server.rs:1600` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1056`<br>`crates/memstead-cli/src/commands/schema.rs:687`<br>`crates/memstead-cli/src/commands/schema.rs:721`<br>`crates/memstead-cli/src/commands/schema.rs:737`<br>`crates/memstead-mcp/src/server.rs:1316` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:126` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1405` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1057`<br>`crates/memstead-mcp/src/server.rs:1325` |
| `SCHEMA_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/schema.rs:543`<br>`crates/memstead-cli/src/commands/schema.rs:672` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1016`<br>`crates/memstead-mcp/src/server.rs:855` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1396` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1383` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1070`<br>`crates/memstead-mcp/src/server.rs:1545` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:201`<br>`crates/memstead-base/src/runtime_validator.rs:202` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1407` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1047`<br>`crates/memstead-mcp/src/server.rs:1118` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:1698` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1039`<br>`crates/memstead-mcp/src/server.rs:1204` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1360` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1041`<br>`crates/memstead-mcp/src/server.rs:1222` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1040`<br>`crates/memstead-mcp/src/server.rs:1213` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1398` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:159`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1358` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1397` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:213` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1353` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1020`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:284`<br>`crates/memstead-mcp/src/server.rs:882` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1374` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1356` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1010`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:288`<br>`crates/memstead-cli/src/commands/export.rs:127`<br>`crates/memstead-cli/src/commands/export.rs:249`<br>`crates/memstead-mcp/src/filesystem_server.rs:1561`<br>`crates/memstead-mcp/src/server.rs:754`<br>`crates/memstead-mcp/src/server.rs:2179`<br>`crates/memstead-mcp/src/server.rs:2278`<br>`crates/memstead-mcp/src/server.rs:2968` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1390` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1011`<br>`crates/memstead-mcp/src/server.rs:764` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1012`<br>`crates/memstead-mcp/src/server.rs:807` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:194` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1359` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1038`<br>`crates/memstead-mcp/src/server.rs:1385` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:270` |
| `WORKSPACE_CONFIG_INVALID` | CLI | `crates/memstead-cli/src/commands/install.rs:283`<br>`crates/memstead-cli/src/commands/install.rs:294`<br>`crates/memstead-cli/src/commands/install.rs:348`<br>`crates/memstead-cli/src/commands/install.rs:359` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:275`<br>`crates/memstead-cli/src/commands/install.rs:334`<br>`crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_CONFIG_WRITE_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:398` |
| `WORKSPACE_NOT_INITIALISED` | CLI, MCP | `crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:270`<br>`crates/memstead-cli/src/commands/publish.rs:384`<br>`crates/memstead-cli/src/commands/publish.rs:408`<br>`crates/memstead-cli/src/commands/workspace.rs:735`<br>`crates/memstead-cli/src/setup.rs:40`<br>`crates/memstead-mcp/src/server.rs:3713` |
