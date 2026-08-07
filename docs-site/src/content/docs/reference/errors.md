---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 185

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
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:92`<br>`crates/memstead-mcp/src/server.rs:2838` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1097`<br>`crates/memstead-mcp/src/server.rs:785` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1849` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:79`<br>`crates/memstead-cli/src/commands/overview.rs:145`<br>`crates/memstead-cli/src/commands/overview.rs:231`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1735` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1127`<br>`crates/memstead-mcp/src/server.rs:1017` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1102`<br>`crates/memstead-base/src/ops/mod.rs:1628` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1111` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1109`<br>`crates/memstead-mcp/src/filesystem_server.rs:445` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1575` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1110`<br>`crates/memstead-mcp/src/filesystem_server.rs:454` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1149`<br>`crates/memstead-base/src/ops/mod.rs:1646`<br>`crates/memstead-mcp/src/server.rs:1421` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:292` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:316` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1088`<br>`crates/memstead-mcp/src/server.rs:1526` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1579` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1629` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1114`<br>`crates/memstead-mcp/src/server.rs:1594` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1101`<br>`crates/memstead-mcp/src/filesystem_server.rs:344`<br>`crates/memstead-mcp/src/server.rs:723` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1105`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:45`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:716`<br>`crates/memstead-cli/src/commands/update.rs:739`<br>`crates/memstead-mcp/src/filesystem_server.rs:356`<br>`crates/memstead-mcp/src/filesystem_server.rs:1020`<br>`crates/memstead-mcp/src/filesystem_server.rs:1759`<br>`crates/memstead-mcp/src/server.rs:713`<br>`crates/memstead-mcp/src/server.rs:1818`<br>`crates/memstead-mcp/src/server.rs:2339` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1604` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1620` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1601` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1605` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1641` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:282` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1106`<br>`crates/memstead-mcp/src/server.rs:744` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1107` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:881` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1625` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1574` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/commands/schema.rs:620`<br>`crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1680`<br>`crates/memstead-mcp/src/filesystem_server.rs:1723` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:67` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1145`<br>`crates/memstead-mcp/src/server.rs:1609` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1124`<br>`crates/memstead-mcp/src/server.rs:268`<br>`crates/memstead-mcp/src/server.rs:283`<br>`crates/memstead-mcp/src/server.rs:1239` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1606`<br>`crates/memstead-base/src/runtime_validator.rs:197` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:204` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1143`<br>`crates/memstead-base/src/engine/error.rs:1144`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:121`<br>`crates/memstead-cli/src/commands/batch.rs:128`<br>`crates/memstead-cli/src/commands/batch.rs:145`<br>`crates/memstead-cli/src/commands/batch.rs:162`<br>`crates/memstead-cli/src/commands/batch.rs:177`<br>`crates/memstead-cli/src/commands/batch_create.rs:102`<br>`crates/memstead-cli/src/commands/batch_create.rs:190`<br>`crates/memstead-cli/src/commands/batch_relate.rs:78`<br>`crates/memstead-cli/src/commands/batch_update.rs:198`<br>`crates/memstead-cli/src/commands/batch_update.rs:209`<br>`crates/memstead-cli/src/commands/batch_update.rs:337`<br>`crates/memstead-cli/src/commands/create.rs:165`<br>`crates/memstead-cli/src/commands/create.rs:172`<br>`crates/memstead-cli/src/commands/create.rs:188`<br>`crates/memstead-cli/src/commands/create.rs:195`<br>`crates/memstead-cli/src/commands/create.rs:234`<br>`crates/memstead-cli/src/commands/create.rs:363`<br>`crates/memstead-cli/src/commands/create.rs:371`<br>`crates/memstead-cli/src/commands/create.rs:437`<br>`crates/memstead-cli/src/commands/create.rs:460`<br>`crates/memstead-cli/src/commands/create.rs:475`<br>`crates/memstead-cli/src/commands/export.rs:84`<br>`crates/memstead-cli/src/commands/export.rs:114`<br>`crates/memstead-cli/src/commands/install.rs:61`<br>`crates/memstead-cli/src/commands/mem.rs:1118`<br>`crates/memstead-cli/src/commands/mod.rs:109`<br>`crates/memstead-cli/src/commands/mod.rs:116`<br>`crates/memstead-cli/src/commands/publish.rs:113`<br>`crates/memstead-cli/src/commands/publish.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:123`<br>`crates/memstead-cli/src/commands/quickstart.rs:338`<br>`crates/memstead-cli/src/commands/quickstart.rs:363`<br>`crates/memstead-cli/src/commands/quickstart.rs:371`<br>`crates/memstead-cli/src/commands/quickstart.rs:441`<br>`crates/memstead-cli/src/commands/quickstart.rs:602`<br>`crates/memstead-cli/src/commands/quickstart.rs:612`<br>`crates/memstead-cli/src/commands/quickstart.rs:624`<br>`crates/memstead-cli/src/commands/quickstart.rs:661`<br>`crates/memstead-cli/src/commands/relate.rs:77`<br>`crates/memstead-cli/src/commands/relate.rs:82`<br>`crates/memstead-cli/src/commands/schema.rs:106`<br>`crates/memstead-cli/src/commands/schema.rs:746`<br>`crates/memstead-cli/src/commands/schema.rs:778`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:166`<br>`crates/memstead-cli/src/commands/update.rs:277`<br>`crates/memstead-cli/src/commands/update.rs:290`<br>`crates/memstead-cli/src/commands/update.rs:306`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:334`<br>`crates/memstead-cli/src/commands/update.rs:373`<br>`crates/memstead-cli/src/commands/update.rs:508`<br>`crates/memstead-cli/src/commands/update.rs:516`<br>`crates/memstead-cli/src/commands/update.rs:524`<br>`crates/memstead-cli/src/commands/update.rs:775`<br>`crates/memstead-cli/src/commands/update.rs:782`<br>`crates/memstead-cli/src/commands/update.rs:804`<br>`crates/memstead-cli/src/commands/update.rs:823`<br>`crates/memstead-cli/src/commands/update.rs:830`<br>`crates/memstead-cli/src/commands/update.rs:837`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-cli/src/main.rs:73`<br>`crates/memstead-mcp/src/filesystem_server.rs:1336`<br>`crates/memstead-mcp/src/filesystem_server.rs:1643`<br>`crates/memstead-mcp/src/filesystem_server.rs:1739`<br>`crates/memstead-mcp/src/filesystem_server.rs:1774`<br>`crates/memstead-mcp/src/filesystem_server.rs:1959`<br>`crates/memstead-mcp/src/server.rs:319`<br>`crates/memstead-mcp/src/server.rs:372`<br>`crates/memstead-mcp/src/server.rs:1363`<br>`crates/memstead-mcp/src/server.rs:1376`<br>`crates/memstead-mcp/src/server.rs:2010`<br>`crates/memstead-mcp/src/server.rs:2182`<br>`crates/memstead-mcp/src/server.rs:2224`<br>`crates/memstead-mcp/src/server.rs:2262`<br>`crates/memstead-mcp/src/server.rs:2278`<br>`crates/memstead-mcp/src/server.rs:2383`<br>`crates/memstead-mcp/src/server.rs:2684`<br>`crates/memstead-mcp/src/server.rs:3184`<br>`crates/memstead-mcp/src/server.rs:3330`<br>`crates/memstead-mcp/src/server.rs:3426`<br>`crates/memstead-mcp/src/server.rs:3484`<br>`crates/memstead-mcp/src/server.rs:3584`<br>`crates/memstead-mcp/src/server.rs:3623`<br>`crates/memstead-mcp/src/server.rs:3652` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1126`<br>`crates/memstead-mcp/src/server.rs:1273`<br>`crates/memstead-mcp/src/server.rs:1689` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:201` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:520` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1100`<br>`crates/memstead-cli/src/commands/batch_create.rs:179`<br>`crates/memstead-cli/src/commands/create.rs:226`<br>`crates/memstead-mcp/src/server.rs:1206` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:129` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1125`<br>`crates/memstead-mcp/src/server.rs:1254` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/schema.rs:142`<br>`crates/memstead-cli/src/commands/schema.rs:151`<br>`crates/memstead-cli/src/commands/schema.rs:176`<br>`crates/memstead-cli/src/commands/schema.rs:188`<br>`crates/memstead-cli/src/commands/schema.rs:838`<br>`crates/memstead-cli/src/commands/schema.rs:847` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1582` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1092`<br>`crates/memstead-mcp/src/server.rs:824` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1094`<br>`crates/memstead-mcp/src/server.rs:846` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:451` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1155`<br>`crates/memstead-mcp/src/server.rs:1581` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1147`<br>`crates/memstead-mcp/src/server.rs:1392` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1137`<br>`crates/memstead-base/src/engine/error.rs:1141`<br>`crates/memstead-cli/src/commands/workspace.rs:761`<br>`crates/memstead-cli/src/commands/workspace.rs:768`<br>`crates/memstead-mcp/src/filesystem_server.rs:821`<br>`crates/memstead-mcp/src/server.rs:1354`<br>`crates/memstead-mcp/src/server.rs:1556` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1638` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1108` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1142`<br>`crates/memstead-mcp/src/server.rs:1312` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:48` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1674` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1639` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1723` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1630` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:660` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1706` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1751` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1148`<br>`crates/memstead-base/src/ops/mod.rs:1645`<br>`crates/memstead-mcp/src/server.rs:1438` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1577` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1103`<br>`crates/memstead-base/src/ops/mod.rs:1627` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1131`<br>`crates/memstead-base/src/ops/mod.rs:1576`<br>`crates/memstead-mcp/src/server.rs:1111` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1607` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:534`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1093`<br>`crates/memstead-mcp/src/server.rs:833` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1624` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:216`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1580` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:565` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:166`<br>`crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:498`<br>`crates/memstead-cli/src/commands/export.rs:337` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1626` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1636` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1135`<br>`crates/memstead-base/src/engine/error.rs:1136`<br>`crates/memstead-mcp/src/filesystem_server.rs:823`<br>`crates/memstead-mcp/src/filesystem_server.rs:825`<br>`crates/memstead-mcp/src/server.rs:1538`<br>`crates/memstead-mcp/src/server.rs:1547` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1133`<br>`crates/memstead-mcp/src/server.rs:1148` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1132`<br>`crates/memstead-mcp/src/filesystem_server.rs:797`<br>`crates/memstead-mcp/src/server.rs:1135` |
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
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1096`<br>`crates/memstead-mcp/src/server.rs:802` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1609` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1617` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:1640` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:226` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1098`<br>`crates/memstead-mcp/src/server.rs:876` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:544`<br>`crates/memstead-cli/src/commands/unpublish.rs:100` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:539`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1128`<br>`crates/memstead-mcp/src/server.rs:1035` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1119`<br>`crates/memstead-mcp/src/server.rs:1291` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1151`<br>`crates/memstead-mcp/src/server.rs:1456` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1116`<br>`crates/memstead-mcp/src/server.rs:1496` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1113`<br>`crates/memstead-mcp/src/filesystem_server.rs:496`<br>`crates/memstead-mcp/src/server.rs:1470` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1118`<br>`crates/memstead-mcp/src/server.rs:1513` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1112`<br>`crates/memstead-mcp/src/server.rs:1008` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1130`<br>`crates/memstead-mcp/src/server.rs:1077` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1637` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1146`<br>`crates/memstead-mcp/src/server.rs:1620` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1643` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1642` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1632` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1138`<br>`crates/memstead-cli/src/commands/schema.rs:719`<br>`crates/memstead-cli/src/commands/schema.rs:753`<br>`crates/memstead-cli/src/commands/schema.rs:769`<br>`crates/memstead-mcp/src/server.rs:1324` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:126` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1631` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1140`<br>`crates/memstead-mcp/src/server.rs:1345` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1139`<br>`crates/memstead-cli/src/commands/schema.rs:550`<br>`crates/memstead-cli/src/commands/schema.rs:688`<br>`crates/memstead-mcp/src/server.rs:1333` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1095`<br>`crates/memstead-mcp/src/server.rs:863` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1621` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1608` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1153`<br>`crates/memstead-mcp/src/server.rs:1565` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:202`<br>`crates/memstead-base/src/runtime_validator.rs:203`<br>`crates/memstead-base/src/section_format.rs:521` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:518` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1633` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:519` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1635` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1129`<br>`crates/memstead-mcp/src/server.rs:1126` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:1700` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1121`<br>`crates/memstead-mcp/src/server.rs:1212` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1585` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1123`<br>`crates/memstead-mcp/src/server.rs:1230` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1122`<br>`crates/memstead-mcp/src/server.rs:1221` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1623` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:159`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1583` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1622` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:213` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1578` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1099`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:284`<br>`crates/memstead-mcp/src/server.rs:890` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1599` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1581` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1089`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:349`<br>`crates/memstead-cli/src/commands/export.rs:132`<br>`crates/memstead-cli/src/commands/export.rs:255`<br>`crates/memstead-cli/src/commands/export.rs:377`<br>`crates/memstead-cli/src/commands/uninstall.rs:36`<br>`crates/memstead-mcp/src/filesystem_server.rs:1747`<br>`crates/memstead-mcp/src/server.rs:762`<br>`crates/memstead-mcp/src/server.rs:2200`<br>`crates/memstead-mcp/src/server.rs:2299`<br>`crates/memstead-mcp/src/server.rs:3166` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:196` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1615` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1090`<br>`crates/memstead-mcp/src/server.rs:772` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1091`<br>`crates/memstead-mcp/src/server.rs:815` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1584` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1120`<br>`crates/memstead-mcp/src/server.rs:1405` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:270` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_NOT_INITIALISED` | CLI, MCP | `crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:398`<br>`crates/memstead-cli/src/commands/publish.rs:390`<br>`crates/memstead-cli/src/commands/workspace.rs:735`<br>`crates/memstead-cli/src/setup.rs:40`<br>`crates/memstead-mcp/src/server.rs:4078` |
