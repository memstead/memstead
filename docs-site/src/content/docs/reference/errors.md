---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 181

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1644` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:344`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:418`<br>`crates/memstead-cli/src/commands/publish.rs:176` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:276` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:269`<br>`crates/memstead-cli/src/commands/publish.rs:529` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1634` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:92`<br>`crates/memstead-mcp/src/server.rs:2836` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1082`<br>`crates/memstead-mcp/src/server.rs:783` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1843` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:79`<br>`crates/memstead-cli/src/commands/overview.rs:145`<br>`crates/memstead-cli/src/commands/overview.rs:231`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1733` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1111`<br>`crates/memstead-mcp/src/server.rs:1015` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1087`<br>`crates/memstead-base/src/ops/mod.rs:1628` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1095` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1093`<br>`crates/memstead-mcp/src/filesystem_server.rs:444` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1575` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1094`<br>`crates/memstead-mcp/src/filesystem_server.rs:453` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1133`<br>`crates/memstead-base/src/ops/mod.rs:1646`<br>`crates/memstead-mcp/src/server.rs:1419` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:292` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:316` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1073`<br>`crates/memstead-mcp/src/server.rs:1524` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1579` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1629` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1098`<br>`crates/memstead-mcp/src/server.rs:1592` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1086`<br>`crates/memstead-mcp/src/filesystem_server.rs:344`<br>`crates/memstead-mcp/src/server.rs:723` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1089`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:45`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:716`<br>`crates/memstead-cli/src/commands/update.rs:739`<br>`crates/memstead-mcp/src/filesystem_server.rs:355`<br>`crates/memstead-mcp/src/filesystem_server.rs:1019`<br>`crates/memstead-mcp/src/filesystem_server.rs:1753`<br>`crates/memstead-mcp/src/server.rs:713`<br>`crates/memstead-mcp/src/server.rs:1816`<br>`crates/memstead-mcp/src/server.rs:2337` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1604` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1620` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1601` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1605` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1641` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:282` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1090`<br>`crates/memstead-mcp/src/server.rs:742` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1091` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:859` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1625` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1574` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/commands/schema.rs:618`<br>`crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1679`<br>`crates/memstead-mcp/src/filesystem_server.rs:1717` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1129`<br>`crates/memstead-mcp/src/server.rs:1607` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1108`<br>`crates/memstead-mcp/src/server.rs:268`<br>`crates/memstead-mcp/src/server.rs:283`<br>`crates/memstead-mcp/src/server.rs:1237` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1606`<br>`crates/memstead-base/src/runtime_validator.rs:197` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:204` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1127`<br>`crates/memstead-base/src/engine/error.rs:1128`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:121`<br>`crates/memstead-cli/src/commands/batch.rs:128`<br>`crates/memstead-cli/src/commands/batch.rs:145`<br>`crates/memstead-cli/src/commands/batch.rs:162`<br>`crates/memstead-cli/src/commands/batch.rs:177`<br>`crates/memstead-cli/src/commands/batch_create.rs:102`<br>`crates/memstead-cli/src/commands/batch_create.rs:190`<br>`crates/memstead-cli/src/commands/batch_relate.rs:78`<br>`crates/memstead-cli/src/commands/batch_update.rs:198`<br>`crates/memstead-cli/src/commands/batch_update.rs:209`<br>`crates/memstead-cli/src/commands/batch_update.rs:337`<br>`crates/memstead-cli/src/commands/create.rs:165`<br>`crates/memstead-cli/src/commands/create.rs:172`<br>`crates/memstead-cli/src/commands/create.rs:188`<br>`crates/memstead-cli/src/commands/create.rs:195`<br>`crates/memstead-cli/src/commands/create.rs:234`<br>`crates/memstead-cli/src/commands/create.rs:363`<br>`crates/memstead-cli/src/commands/create.rs:371`<br>`crates/memstead-cli/src/commands/create.rs:437`<br>`crates/memstead-cli/src/commands/create.rs:460`<br>`crates/memstead-cli/src/commands/create.rs:475`<br>`crates/memstead-cli/src/commands/export.rs:84`<br>`crates/memstead-cli/src/commands/export.rs:114`<br>`crates/memstead-cli/src/commands/install.rs:61`<br>`crates/memstead-cli/src/commands/mem.rs:1118`<br>`crates/memstead-cli/src/commands/mod.rs:109`<br>`crates/memstead-cli/src/commands/mod.rs:116`<br>`crates/memstead-cli/src/commands/publish.rs:113`<br>`crates/memstead-cli/src/commands/publish.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:123`<br>`crates/memstead-cli/src/commands/quickstart.rs:338`<br>`crates/memstead-cli/src/commands/quickstart.rs:363`<br>`crates/memstead-cli/src/commands/quickstart.rs:371`<br>`crates/memstead-cli/src/commands/quickstart.rs:441`<br>`crates/memstead-cli/src/commands/quickstart.rs:602`<br>`crates/memstead-cli/src/commands/quickstart.rs:612`<br>`crates/memstead-cli/src/commands/quickstart.rs:624`<br>`crates/memstead-cli/src/commands/quickstart.rs:661`<br>`crates/memstead-cli/src/commands/relate.rs:77`<br>`crates/memstead-cli/src/commands/relate.rs:82`<br>`crates/memstead-cli/src/commands/schema.rs:106`<br>`crates/memstead-cli/src/commands/schema.rs:743`<br>`crates/memstead-cli/src/commands/schema.rs:775`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:166`<br>`crates/memstead-cli/src/commands/update.rs:277`<br>`crates/memstead-cli/src/commands/update.rs:290`<br>`crates/memstead-cli/src/commands/update.rs:306`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:334`<br>`crates/memstead-cli/src/commands/update.rs:373`<br>`crates/memstead-cli/src/commands/update.rs:508`<br>`crates/memstead-cli/src/commands/update.rs:516`<br>`crates/memstead-cli/src/commands/update.rs:524`<br>`crates/memstead-cli/src/commands/update.rs:775`<br>`crates/memstead-cli/src/commands/update.rs:782`<br>`crates/memstead-cli/src/commands/update.rs:804`<br>`crates/memstead-cli/src/commands/update.rs:823`<br>`crates/memstead-cli/src/commands/update.rs:830`<br>`crates/memstead-cli/src/commands/update.rs:837`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-cli/src/main.rs:73`<br>`crates/memstead-mcp/src/filesystem_server.rs:1335`<br>`crates/memstead-mcp/src/filesystem_server.rs:1642`<br>`crates/memstead-mcp/src/filesystem_server.rs:1733`<br>`crates/memstead-mcp/src/filesystem_server.rs:1768`<br>`crates/memstead-mcp/src/filesystem_server.rs:1953`<br>`crates/memstead-mcp/src/server.rs:319`<br>`crates/memstead-mcp/src/server.rs:372`<br>`crates/memstead-mcp/src/server.rs:1361`<br>`crates/memstead-mcp/src/server.rs:1374`<br>`crates/memstead-mcp/src/server.rs:2008`<br>`crates/memstead-mcp/src/server.rs:2180`<br>`crates/memstead-mcp/src/server.rs:2222`<br>`crates/memstead-mcp/src/server.rs:2260`<br>`crates/memstead-mcp/src/server.rs:2276`<br>`crates/memstead-mcp/src/server.rs:2381`<br>`crates/memstead-mcp/src/server.rs:2682`<br>`crates/memstead-mcp/src/server.rs:3182`<br>`crates/memstead-mcp/src/server.rs:3328`<br>`crates/memstead-mcp/src/server.rs:3424`<br>`crates/memstead-mcp/src/server.rs:3482`<br>`crates/memstead-mcp/src/server.rs:3582`<br>`crates/memstead-mcp/src/server.rs:3621`<br>`crates/memstead-mcp/src/server.rs:3650` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1110`<br>`crates/memstead-mcp/src/server.rs:1271`<br>`crates/memstead-mcp/src/server.rs:1687` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:201` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1085`<br>`crates/memstead-cli/src/commands/batch_create.rs:179`<br>`crates/memstead-cli/src/commands/create.rs:226`<br>`crates/memstead-mcp/src/server.rs:1204` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:129` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1109`<br>`crates/memstead-mcp/src/server.rs:1252` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/schema.rs:142`<br>`crates/memstead-cli/src/commands/schema.rs:151`<br>`crates/memstead-cli/src/commands/schema.rs:176`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:835`<br>`crates/memstead-cli/src/commands/schema.rs:844` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1582` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1077`<br>`crates/memstead-mcp/src/server.rs:822` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1079`<br>`crates/memstead-mcp/src/server.rs:844` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:451` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1139`<br>`crates/memstead-mcp/src/server.rs:1579` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1131`<br>`crates/memstead-mcp/src/server.rs:1390` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1121`<br>`crates/memstead-base/src/engine/error.rs:1125`<br>`crates/memstead-cli/src/commands/workspace.rs:761`<br>`crates/memstead-cli/src/commands/workspace.rs:768`<br>`crates/memstead-mcp/src/filesystem_server.rs:820`<br>`crates/memstead-mcp/src/server.rs:1352`<br>`crates/memstead-mcp/src/server.rs:1554` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1638` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1092` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1126`<br>`crates/memstead-mcp/src/server.rs:1310` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:48` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1672` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1639` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1721` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1630` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:658` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1704` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1749` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1132`<br>`crates/memstead-base/src/ops/mod.rs:1645`<br>`crates/memstead-mcp/src/server.rs:1436` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1577` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1088`<br>`crates/memstead-base/src/ops/mod.rs:1627` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1115`<br>`crates/memstead-base/src/ops/mod.rs:1576`<br>`crates/memstead-mcp/src/server.rs:1109` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1607` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:534`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1078`<br>`crates/memstead-mcp/src/server.rs:831` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1624` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:216`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1580` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:563` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:166`<br>`crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:498`<br>`crates/memstead-cli/src/commands/export.rs:337` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1626` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1636` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1119`<br>`crates/memstead-base/src/engine/error.rs:1120`<br>`crates/memstead-mcp/src/filesystem_server.rs:822`<br>`crates/memstead-mcp/src/filesystem_server.rs:824`<br>`crates/memstead-mcp/src/server.rs:1536`<br>`crates/memstead-mcp/src/server.rs:1545` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1117`<br>`crates/memstead-mcp/src/server.rs:1146` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1116`<br>`crates/memstead-mcp/src/filesystem_server.rs:796`<br>`crates/memstead-mcp/src/server.rs:1133` |
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
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1081`<br>`crates/memstead-mcp/src/server.rs:800` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1609` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1617` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:1640` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:226` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1083`<br>`crates/memstead-mcp/src/server.rs:874` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:544`<br>`crates/memstead-cli/src/commands/unpublish.rs:100` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:539`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1112`<br>`crates/memstead-mcp/src/server.rs:1033` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1103`<br>`crates/memstead-mcp/src/server.rs:1289` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1135`<br>`crates/memstead-mcp/src/server.rs:1454` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1100`<br>`crates/memstead-mcp/src/server.rs:1494` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1097`<br>`crates/memstead-mcp/src/filesystem_server.rs:495`<br>`crates/memstead-mcp/src/server.rs:1468` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1102`<br>`crates/memstead-mcp/src/server.rs:1511` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1096`<br>`crates/memstead-mcp/src/server.rs:1006` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1114`<br>`crates/memstead-mcp/src/server.rs:1075` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1637` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1130`<br>`crates/memstead-mcp/src/server.rs:1618` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1643` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1642` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1632` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1122`<br>`crates/memstead-cli/src/commands/schema.rs:716`<br>`crates/memstead-cli/src/commands/schema.rs:750`<br>`crates/memstead-cli/src/commands/schema.rs:766`<br>`crates/memstead-mcp/src/server.rs:1322` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:126` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1631` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1124`<br>`crates/memstead-mcp/src/server.rs:1343` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1123`<br>`crates/memstead-cli/src/commands/schema.rs:548`<br>`crates/memstead-cli/src/commands/schema.rs:685`<br>`crates/memstead-mcp/src/server.rs:1331` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1080`<br>`crates/memstead-mcp/src/server.rs:861` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1621` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1608` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1137`<br>`crates/memstead-mcp/src/server.rs:1563` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:202`<br>`crates/memstead-base/src/runtime_validator.rs:203` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1633` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1635` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1113`<br>`crates/memstead-mcp/src/server.rs:1124` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:1700` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1105`<br>`crates/memstead-mcp/src/server.rs:1210` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1585` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1107`<br>`crates/memstead-mcp/src/server.rs:1228` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1106`<br>`crates/memstead-mcp/src/server.rs:1219` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1623` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:159`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1583` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1622` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:213` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1578` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1084`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:284`<br>`crates/memstead-mcp/src/server.rs:888` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1599` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1581` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1074`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:349`<br>`crates/memstead-cli/src/commands/export.rs:132`<br>`crates/memstead-cli/src/commands/export.rs:255`<br>`crates/memstead-cli/src/commands/export.rs:377`<br>`crates/memstead-cli/src/commands/uninstall.rs:36`<br>`crates/memstead-mcp/src/filesystem_server.rs:1741`<br>`crates/memstead-mcp/src/server.rs:760`<br>`crates/memstead-mcp/src/server.rs:2198`<br>`crates/memstead-mcp/src/server.rs:2297`<br>`crates/memstead-mcp/src/server.rs:3164` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:196` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1615` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1075`<br>`crates/memstead-mcp/src/server.rs:770` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1076`<br>`crates/memstead-mcp/src/server.rs:813` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1584` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1104`<br>`crates/memstead-mcp/src/server.rs:1403` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:270` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_NOT_INITIALISED` | CLI, MCP | `crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:398`<br>`crates/memstead-cli/src/commands/publish.rs:390`<br>`crates/memstead-cli/src/commands/workspace.rs:735`<br>`crates/memstead-cli/src/setup.rs:40`<br>`crates/memstead-mcp/src/server.rs:4076` |
