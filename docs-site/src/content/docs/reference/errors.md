---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 181

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1582` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:216`<br>`crates/memstead-cli/src/commands/install.rs:548`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:290`<br>`crates/memstead-cli/src/commands/publish.rs:176` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:276` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:269`<br>`crates/memstead-cli/src/commands/publish.rs:529` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1573` |
| `BATCH_REFUSED` | CLI | `crates/memstead-cli/src/commands/batch.rs:92` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1048`<br>`crates/memstead-mcp/src/server.rs:777` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1671` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:79`<br>`crates/memstead-cli/src/commands/overview.rs:145`<br>`crates/memstead-cli/src/commands/overview.rs:231`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1727` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1075`<br>`crates/memstead-mcp/src/server.rs:1009` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1059` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1057`<br>`crates/memstead-mcp/src/filesystem_server.rs:437` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1515` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1058`<br>`crates/memstead-mcp/src/filesystem_server.rs:446` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1097`<br>`crates/memstead-base/src/ops/mod.rs:1584`<br>`crates/memstead-mcp/src/server.rs:1413` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:292` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:316` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1039`<br>`crates/memstead-mcp/src/server.rs:1518` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1519` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1568` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1062`<br>`crates/memstead-mcp/src/server.rs:1586` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1052`<br>`crates/memstead-mcp/src/filesystem_server.rs:344`<br>`crates/memstead-mcp/src/server.rs:723` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1053`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:45`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:716`<br>`crates/memstead-cli/src/commands/update.rs:739`<br>`crates/memstead-mcp/src/filesystem_server.rs:348`<br>`crates/memstead-mcp/src/filesystem_server.rs:1012`<br>`crates/memstead-mcp/src/filesystem_server.rs:1581`<br>`crates/memstead-mcp/src/server.rs:713`<br>`crates/memstead-mcp/src/server.rs:1810`<br>`crates/memstead-mcp/src/server.rs:2331` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1544` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1560` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1541` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1545` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1579` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:282` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1054`<br>`crates/memstead-mcp/src/server.rs:736` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1055` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:712` |
| `HOST_MEM_NOT_REGISTERED` | CLI | `crates/memstead-cli/src/commands/install.rs:523` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1565` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1514` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/commands/schema.rs:615`<br>`crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1545` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1093`<br>`crates/memstead-mcp/src/server.rs:1601` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1072`<br>`crates/memstead-mcp/src/server.rs:268`<br>`crates/memstead-mcp/src/server.rs:283`<br>`crates/memstead-mcp/src/server.rs:1231` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1546`<br>`crates/memstead-base/src/runtime_validator.rs:196` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:203` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1091`<br>`crates/memstead-base/src/engine/error.rs:1092`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:121`<br>`crates/memstead-cli/src/commands/batch.rs:128`<br>`crates/memstead-cli/src/commands/batch.rs:145`<br>`crates/memstead-cli/src/commands/batch.rs:162`<br>`crates/memstead-cli/src/commands/batch.rs:177`<br>`crates/memstead-cli/src/commands/batch_create.rs:102`<br>`crates/memstead-cli/src/commands/batch_create.rs:190`<br>`crates/memstead-cli/src/commands/batch_relate.rs:78`<br>`crates/memstead-cli/src/commands/batch_update.rs:198`<br>`crates/memstead-cli/src/commands/batch_update.rs:209`<br>`crates/memstead-cli/src/commands/batch_update.rs:337`<br>`crates/memstead-cli/src/commands/create.rs:165`<br>`crates/memstead-cli/src/commands/create.rs:172`<br>`crates/memstead-cli/src/commands/create.rs:188`<br>`crates/memstead-cli/src/commands/create.rs:195`<br>`crates/memstead-cli/src/commands/create.rs:234`<br>`crates/memstead-cli/src/commands/create.rs:363`<br>`crates/memstead-cli/src/commands/create.rs:371`<br>`crates/memstead-cli/src/commands/create.rs:437`<br>`crates/memstead-cli/src/commands/create.rs:460`<br>`crates/memstead-cli/src/commands/create.rs:475`<br>`crates/memstead-cli/src/commands/export.rs:64`<br>`crates/memstead-cli/src/commands/mem.rs:1016`<br>`crates/memstead-cli/src/commands/mod.rs:109`<br>`crates/memstead-cli/src/commands/mod.rs:116`<br>`crates/memstead-cli/src/commands/publish.rs:113`<br>`crates/memstead-cli/src/commands/publish.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:123`<br>`crates/memstead-cli/src/commands/quickstart.rs:338`<br>`crates/memstead-cli/src/commands/quickstart.rs:363`<br>`crates/memstead-cli/src/commands/quickstart.rs:371`<br>`crates/memstead-cli/src/commands/quickstart.rs:441`<br>`crates/memstead-cli/src/commands/quickstart.rs:602`<br>`crates/memstead-cli/src/commands/quickstart.rs:612`<br>`crates/memstead-cli/src/commands/quickstart.rs:624`<br>`crates/memstead-cli/src/commands/quickstart.rs:661`<br>`crates/memstead-cli/src/commands/relate.rs:77`<br>`crates/memstead-cli/src/commands/relate.rs:82`<br>`crates/memstead-cli/src/commands/schema.rs:106`<br>`crates/memstead-cli/src/commands/schema.rs:737`<br>`crates/memstead-cli/src/commands/schema.rs:769`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:166`<br>`crates/memstead-cli/src/commands/update.rs:277`<br>`crates/memstead-cli/src/commands/update.rs:290`<br>`crates/memstead-cli/src/commands/update.rs:306`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:334`<br>`crates/memstead-cli/src/commands/update.rs:373`<br>`crates/memstead-cli/src/commands/update.rs:508`<br>`crates/memstead-cli/src/commands/update.rs:516`<br>`crates/memstead-cli/src/commands/update.rs:524`<br>`crates/memstead-cli/src/commands/update.rs:775`<br>`crates/memstead-cli/src/commands/update.rs:782`<br>`crates/memstead-cli/src/commands/update.rs:804`<br>`crates/memstead-cli/src/commands/update.rs:823`<br>`crates/memstead-cli/src/commands/update.rs:830`<br>`crates/memstead-cli/src/commands/update.rs:837`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-cli/src/main.rs:73`<br>`crates/memstead-mcp/src/filesystem_server.rs:1491`<br>`crates/memstead-mcp/src/filesystem_server.rs:1561`<br>`crates/memstead-mcp/src/filesystem_server.rs:1596`<br>`crates/memstead-mcp/src/filesystem_server.rs:1781`<br>`crates/memstead-mcp/src/server.rs:319`<br>`crates/memstead-mcp/src/server.rs:372`<br>`crates/memstead-mcp/src/server.rs:1355`<br>`crates/memstead-mcp/src/server.rs:1368`<br>`crates/memstead-mcp/src/server.rs:2002`<br>`crates/memstead-mcp/src/server.rs:2174`<br>`crates/memstead-mcp/src/server.rs:2216`<br>`crates/memstead-mcp/src/server.rs:2254`<br>`crates/memstead-mcp/src/server.rs:2270`<br>`crates/memstead-mcp/src/server.rs:2375`<br>`crates/memstead-mcp/src/server.rs:3006`<br>`crates/memstead-mcp/src/server.rs:3152`<br>`crates/memstead-mcp/src/server.rs:3248`<br>`crates/memstead-mcp/src/server.rs:3305`<br>`crates/memstead-mcp/src/server.rs:3344`<br>`crates/memstead-mcp/src/server.rs:3373` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1074`<br>`crates/memstead-mcp/src/server.rs:1265`<br>`crates/memstead-mcp/src/server.rs:1681` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1051`<br>`crates/memstead-cli/src/commands/batch_create.rs:179`<br>`crates/memstead-cli/src/commands/create.rs:226`<br>`crates/memstead-mcp/src/server.rs:1198` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:129` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1073`<br>`crates/memstead-mcp/src/server.rs:1246` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/schema.rs:142`<br>`crates/memstead-cli/src/commands/schema.rs:151`<br>`crates/memstead-cli/src/commands/schema.rs:176`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:829`<br>`crates/memstead-cli/src/commands/schema.rs:838` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1522` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1043`<br>`crates/memstead-mcp/src/server.rs:816` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1045`<br>`crates/memstead-mcp/src/server.rs:838` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:451` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1103`<br>`crates/memstead-mcp/src/server.rs:1573` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1095`<br>`crates/memstead-mcp/src/server.rs:1384` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1085`<br>`crates/memstead-base/src/engine/error.rs:1089`<br>`crates/memstead-cli/src/commands/workspace.rs:761`<br>`crates/memstead-cli/src/commands/workspace.rs:768`<br>`crates/memstead-mcp/src/filesystem_server.rs:813`<br>`crates/memstead-mcp/src/server.rs:1346`<br>`crates/memstead-mcp/src/server.rs:1548` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1577` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1056` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1090`<br>`crates/memstead-mcp/src/server.rs:1304` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1666` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1578` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1715` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1569` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:655` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1698` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1743` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1096`<br>`crates/memstead-base/src/ops/mod.rs:1583`<br>`crates/memstead-mcp/src/server.rs:1430` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1517` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/ops/mod.rs:1567` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1079`<br>`crates/memstead-base/src/ops/mod.rs:1516`<br>`crates/memstead-mcp/src/server.rs:1103` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1547` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:534`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1044`<br>`crates/memstead-mcp/src/server.rs:825` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1564` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:216`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1520` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:560` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:166`<br>`crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:498`<br>`crates/memstead-cli/src/commands/export.rs:209`<br>`crates/memstead-cli/src/commands/install.rs:541` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1566` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1575` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1083`<br>`crates/memstead-base/src/engine/error.rs:1084`<br>`crates/memstead-mcp/src/filesystem_server.rs:815`<br>`crates/memstead-mcp/src/filesystem_server.rs:817`<br>`crates/memstead-mcp/src/server.rs:1530`<br>`crates/memstead-mcp/src/server.rs:1539` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1081`<br>`crates/memstead-mcp/src/server.rs:1140` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1080`<br>`crates/memstead-mcp/src/filesystem_server.rs:789`<br>`crates/memstead-mcp/src/server.rs:1127` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1328`<br>`crates/memstead-cli/src/commands/projection.rs:1373`<br>`crates/memstead-cli/src/commands/projection.rs:1408` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1363` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:446` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:408` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1283`<br>`crates/memstead-cli/src/commands/projection.rs:1720` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1162` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1514`<br>`crates/memstead-cli/src/commands/projection.rs:1548` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:1509` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:623` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:414` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:572` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1394` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1535` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:422`<br>`crates/memstead-cli/src/commands/projection.rs:597`<br>`crates/memstead-cli/src/commands/projection.rs:1145`<br>`crates/memstead-cli/src/commands/projection.rs:1326`<br>`crates/memstead-cli/src/commands/projection.rs:1341`<br>`crates/memstead-cli/src/commands/projection.rs:1504` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:402`<br>`crates/memstead-cli/src/commands/projection.rs:486` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:773` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:799`<br>`crates/memstead-cli/src/commands/projection.rs:931`<br>`crates/memstead-cli/src/commands/projection.rs:1043`<br>`crates/memstead-cli/src/commands/projection.rs:1052`<br>`crates/memstead-cli/src/commands/projection.rs:1062` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:983` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:766` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:778` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:761` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:419`<br>`crates/memstead-cli/src/commands/projection.rs:1200`<br>`crates/memstead-cli/src/commands/projection.rs:1420`<br>`crates/memstead-cli/src/commands/projection.rs:1560`<br>`crates/memstead-cli/src/commands/projection.rs:1670` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1224` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1435` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1752` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1779` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1658`<br>`crates/memstead-cli/src/commands/projection.rs:1731` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1047`<br>`crates/memstead-mcp/src/server.rs:794` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1549` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1557` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:475` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:197` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1049`<br>`crates/memstead-mcp/src/server.rs:868` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:544`<br>`crates/memstead-cli/src/commands/unpublish.rs:100` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:539`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1076`<br>`crates/memstead-mcp/src/server.rs:1027` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1067`<br>`crates/memstead-mcp/src/server.rs:1283` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1099`<br>`crates/memstead-mcp/src/server.rs:1448` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1064`<br>`crates/memstead-mcp/src/server.rs:1488` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1061`<br>`crates/memstead-mcp/src/filesystem_server.rs:488`<br>`crates/memstead-mcp/src/server.rs:1462` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1066`<br>`crates/memstead-mcp/src/server.rs:1505` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1060`<br>`crates/memstead-mcp/src/server.rs:1000` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1078`<br>`crates/memstead-mcp/src/server.rs:1069` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1576` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1094`<br>`crates/memstead-mcp/src/server.rs:1612` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1581` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1580` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1571` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1086`<br>`crates/memstead-cli/src/commands/schema.rs:710`<br>`crates/memstead-cli/src/commands/schema.rs:744`<br>`crates/memstead-cli/src/commands/schema.rs:760`<br>`crates/memstead-mcp/src/server.rs:1316` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:126` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1570` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1088`<br>`crates/memstead-mcp/src/server.rs:1337` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1087`<br>`crates/memstead-cli/src/commands/schema.rs:545`<br>`crates/memstead-cli/src/commands/schema.rs:679`<br>`crates/memstead-mcp/src/server.rs:1325` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1046`<br>`crates/memstead-mcp/src/server.rs:855` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1561` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1548` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1101`<br>`crates/memstead-mcp/src/server.rs:1557` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:201`<br>`crates/memstead-base/src/runtime_validator.rs:202` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1572` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1574` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1077`<br>`crates/memstead-mcp/src/server.rs:1118` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:1700` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1069`<br>`crates/memstead-mcp/src/server.rs:1204` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1525` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1071`<br>`crates/memstead-mcp/src/server.rs:1222` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1070`<br>`crates/memstead-mcp/src/server.rs:1213` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1563` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:159`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1523` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1562` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:213` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1518` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1050`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:284`<br>`crates/memstead-mcp/src/server.rs:882` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1539` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1521` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1040`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:349`<br>`crates/memstead-cli/src/commands/export.rs:127`<br>`crates/memstead-cli/src/commands/export.rs:249`<br>`crates/memstead-mcp/src/filesystem_server.rs:1569`<br>`crates/memstead-mcp/src/server.rs:754`<br>`crates/memstead-mcp/src/server.rs:2192`<br>`crates/memstead-mcp/src/server.rs:2291`<br>`crates/memstead-mcp/src/server.rs:2988` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1555` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1041`<br>`crates/memstead-mcp/src/server.rs:764` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1042`<br>`crates/memstead-mcp/src/server.rs:807` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:194` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1524` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1068`<br>`crates/memstead-mcp/src/server.rs:1397` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:270` |
| `WORKSPACE_CONFIG_INVALID` | CLI | `crates/memstead-cli/src/commands/install.rs:283`<br>`crates/memstead-cli/src/commands/install.rs:294`<br>`crates/memstead-cli/src/commands/install.rs:348`<br>`crates/memstead-cli/src/commands/install.rs:359` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:275`<br>`crates/memstead-cli/src/commands/install.rs:334`<br>`crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_CONFIG_WRITE_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:398` |
| `WORKSPACE_NOT_INITIALISED` | CLI, MCP | `crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:270`<br>`crates/memstead-cli/src/commands/publish.rs:390`<br>`crates/memstead-cli/src/commands/workspace.rs:735`<br>`crates/memstead-cli/src/setup.rs:40`<br>`crates/memstead-mcp/src/server.rs:3761` |
