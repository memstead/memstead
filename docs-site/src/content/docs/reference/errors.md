---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 203

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1759` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:349`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:423`<br>`crates/memstead-cli/src/commands/publish.rs:176` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:276` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:269`<br>`crates/memstead-cli/src/commands/publish.rs:529` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1748` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:92`<br>`crates/memstead-mcp/src/server.rs:2939` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1145`<br>`crates/memstead-mcp/src/server.rs:845` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1943` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1147`<br>`crates/memstead-mcp/src/server.rs:945` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:113`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1778` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1176`<br>`crates/memstead-mcp/src/server.rs:1086` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1151`<br>`crates/memstead-base/src/ops/mod.rs:1740` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1160` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1158`<br>`crates/memstead-mcp/src/filesystem_server.rs:462` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1686` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1159`<br>`crates/memstead-mcp/src/filesystem_server.rs:471` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:1749` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1198`<br>`crates/memstead-base/src/ops/mod.rs:1761`<br>`crates/memstead-mcp/src/server.rs:1476` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:292` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:316` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1135`<br>`crates/memstead-mcp/src/server.rs:1581` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1690` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1741` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1163`<br>`crates/memstead-mcp/src/server.rs:1649` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:1744` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1150`<br>`crates/memstead-mcp/src/server.rs:766` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1154`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:58`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:721`<br>`crates/memstead-cli/src/commands/update.rs:744`<br>`crates/memstead-mcp/src/filesystem_server.rs:373`<br>`crates/memstead-mcp/src/filesystem_server.rs:1044`<br>`crates/memstead-mcp/src/filesystem_server.rs:1853`<br>`crates/memstead-mcp/src/server.rs:756`<br>`crates/memstead-mcp/src/server.rs:1861`<br>`crates/memstead-mcp/src/server.rs:2425` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1716` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1732` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1713` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1717` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:50` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1756` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:282` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1155`<br>`crates/memstead-mcp/src/server.rs:787` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1156` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:1176` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1737` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1685` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1757`<br>`crates/memstead-mcp/src/filesystem_server.rs:1817` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:67` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1194`<br>`crates/memstead-mcp/src/server.rs:1664` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1173`<br>`crates/memstead-mcp/src/server.rs:311`<br>`crates/memstead-mcp/src/server.rs:326`<br>`crates/memstead-mcp/src/server.rs:1294` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1718`<br>`crates/memstead-base/src/runtime_validator.rs:197` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:204` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1192`<br>`crates/memstead-base/src/engine/error.rs:1193`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:121`<br>`crates/memstead-cli/src/commands/batch.rs:128`<br>`crates/memstead-cli/src/commands/batch.rs:145`<br>`crates/memstead-cli/src/commands/batch.rs:162`<br>`crates/memstead-cli/src/commands/batch.rs:177`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:204`<br>`crates/memstead-cli/src/commands/batch_relate.rs:84`<br>`crates/memstead-cli/src/commands/batch_update.rs:209`<br>`crates/memstead-cli/src/commands/batch_update.rs:220`<br>`crates/memstead-cli/src/commands/batch_update.rs:348`<br>`crates/memstead-cli/src/commands/create.rs:165`<br>`crates/memstead-cli/src/commands/create.rs:172`<br>`crates/memstead-cli/src/commands/create.rs:188`<br>`crates/memstead-cli/src/commands/create.rs:195`<br>`crates/memstead-cli/src/commands/create.rs:235`<br>`crates/memstead-cli/src/commands/create.rs:364`<br>`crates/memstead-cli/src/commands/create.rs:372`<br>`crates/memstead-cli/src/commands/create.rs:443`<br>`crates/memstead-cli/src/commands/create.rs:466`<br>`crates/memstead-cli/src/commands/create.rs:481`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/export.rs:91`<br>`crates/memstead-cli/src/commands/export.rs:122`<br>`crates/memstead-cli/src/commands/export.rs:543`<br>`crates/memstead-cli/src/commands/export.rs:551`<br>`crates/memstead-cli/src/commands/install.rs:61`<br>`crates/memstead-cli/src/commands/mem.rs:1118`<br>`crates/memstead-cli/src/commands/mod.rs:112`<br>`crates/memstead-cli/src/commands/mod.rs:119`<br>`crates/memstead-cli/src/commands/publish.rs:113`<br>`crates/memstead-cli/src/commands/publish.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:123`<br>`crates/memstead-cli/src/commands/quickstart.rs:338`<br>`crates/memstead-cli/src/commands/quickstart.rs:363`<br>`crates/memstead-cli/src/commands/quickstart.rs:371`<br>`crates/memstead-cli/src/commands/quickstart.rs:441`<br>`crates/memstead-cli/src/commands/quickstart.rs:605`<br>`crates/memstead-cli/src/commands/quickstart.rs:615`<br>`crates/memstead-cli/src/commands/quickstart.rs:627`<br>`crates/memstead-cli/src/commands/quickstart.rs:664`<br>`crates/memstead-cli/src/commands/relate.rs:85`<br>`crates/memstead-cli/src/commands/relate.rs:90`<br>`crates/memstead-cli/src/commands/schema.rs:112`<br>`crates/memstead-cli/src/commands/schema.rs:800`<br>`crates/memstead-cli/src/commands/schema.rs:832`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:166`<br>`crates/memstead-cli/src/commands/update.rs:277`<br>`crates/memstead-cli/src/commands/update.rs:290`<br>`crates/memstead-cli/src/commands/update.rs:306`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:334`<br>`crates/memstead-cli/src/commands/update.rs:373`<br>`crates/memstead-cli/src/commands/update.rs:508`<br>`crates/memstead-cli/src/commands/update.rs:516`<br>`crates/memstead-cli/src/commands/update.rs:524`<br>`crates/memstead-cli/src/commands/update.rs:780`<br>`crates/memstead-cli/src/commands/update.rs:787`<br>`crates/memstead-cli/src/commands/update.rs:809`<br>`crates/memstead-cli/src/commands/update.rs:828`<br>`crates/memstead-cli/src/commands/update.rs:835`<br>`crates/memstead-cli/src/commands/update.rs:842`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:1391`<br>`crates/memstead-mcp/src/filesystem_server.rs:1708`<br>`crates/memstead-mcp/src/filesystem_server.rs:1833`<br>`crates/memstead-mcp/src/filesystem_server.rs:1868`<br>`crates/memstead-mcp/src/filesystem_server.rs:2112`<br>`crates/memstead-mcp/src/server.rs:362`<br>`crates/memstead-mcp/src/server.rs:415`<br>`crates/memstead-mcp/src/server.rs:1418`<br>`crates/memstead-mcp/src/server.rs:1431`<br>`crates/memstead-mcp/src/server.rs:2084`<br>`crates/memstead-mcp/src/server.rs:2256`<br>`crates/memstead-mcp/src/server.rs:2302`<br>`crates/memstead-mcp/src/server.rs:2340`<br>`crates/memstead-mcp/src/server.rs:2356`<br>`crates/memstead-mcp/src/server.rs:2469`<br>`crates/memstead-mcp/src/server.rs:2782`<br>`crates/memstead-mcp/src/server.rs:3389`<br>`crates/memstead-mcp/src/server.rs:3535`<br>`crates/memstead-mcp/src/server.rs:3631`<br>`crates/memstead-mcp/src/server.rs:3689`<br>`crates/memstead-mcp/src/server.rs:3789`<br>`crates/memstead-mcp/src/server.rs:3828`<br>`crates/memstead-mcp/src/server.rs:3857` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1175`<br>`crates/memstead-mcp/src/server.rs:1328`<br>`crates/memstead-mcp/src/server.rs:1732` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:201` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/server.rs:192` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:522` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1149`<br>`crates/memstead-cli/src/commands/batch_create.rs:192`<br>`crates/memstead-cli/src/commands/create.rs:226`<br>`crates/memstead-mcp/src/server.rs:1261` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:41`<br>`crates/memstead-mcp/src/server.rs:3157` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:129` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1174`<br>`crates/memstead-mcp/src/server.rs:1309` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:579`<br>`crates/memstead-cli/src/commands/schema.rs:148`<br>`crates/memstead-cli/src/commands/schema.rs:157`<br>`crates/memstead-cli/src/commands/schema.rs:182`<br>`crates/memstead-cli/src/commands/schema.rs:194`<br>`crates/memstead-cli/src/commands/schema.rs:911`<br>`crates/memstead-cli/src/commands/schema.rs:920` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:161` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1693` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1140`<br>`crates/memstead-mcp/src/server.rs:884` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1142`<br>`crates/memstead-mcp/src/server.rs:906` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:451` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1204`<br>`crates/memstead-mcp/src/server.rs:1636` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1196`<br>`crates/memstead-mcp/src/server.rs:1447` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1186`<br>`crates/memstead-base/src/engine/error.rs:1190`<br>`crates/memstead-cli/src/commands/workspace.rs:762`<br>`crates/memstead-cli/src/commands/workspace.rs:769`<br>`crates/memstead-mcp/src/filesystem_server.rs:838`<br>`crates/memstead-mcp/src/server.rs:1409`<br>`crates/memstead-mcp/src/server.rs:1611` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1753` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1157` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1191`<br>`crates/memstead-mcp/src/server.rs:1367` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:48` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1727` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1137`<br>`crates/memstead-mcp/src/server.rs:819` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1754` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1766` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1742` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:701` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1749` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1794` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1197`<br>`crates/memstead-base/src/ops/mod.rs:1760`<br>`crates/memstead-mcp/src/server.rs:1493` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1688` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1152`<br>`crates/memstead-base/src/ops/mod.rs:1739` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1180`<br>`crates/memstead-base/src/ops/mod.rs:1687`<br>`crates/memstead-mcp/src/server.rs:1180` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1719` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:534`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1141`<br>`crates/memstead-mcp/src/server.rs:893` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1736` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:216`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1691` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:599` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:179`<br>`crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:504`<br>`crates/memstead-cli/src/commands/export.rs:342` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1738` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1751` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1184`<br>`crates/memstead-base/src/engine/error.rs:1185`<br>`crates/memstead-mcp/src/filesystem_server.rs:840`<br>`crates/memstead-mcp/src/filesystem_server.rs:842`<br>`crates/memstead-mcp/src/server.rs:1593`<br>`crates/memstead-mcp/src/server.rs:1602` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1182`<br>`crates/memstead-mcp/src/server.rs:1217` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1181`<br>`crates/memstead-mcp/src/filesystem_server.rs:814`<br>`crates/memstead-mcp/src/server.rs:1204` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1461`<br>`crates/memstead-cli/src/commands/projection.rs:1506`<br>`crates/memstead-cli/src/commands/projection.rs:1541` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1496` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:460` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:408` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1416`<br>`crates/memstead-cli/src/commands/projection.rs:1823` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1284` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1637`<br>`crates/memstead-cli/src/commands/projection.rs:1671` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:1632` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:637` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:414` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:586` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1527` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1658` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:422`<br>`crates/memstead-cli/src/commands/projection.rs:611`<br>`crates/memstead-cli/src/commands/projection.rs:1267`<br>`crates/memstead-cli/src/commands/projection.rs:1459`<br>`crates/memstead-cli/src/commands/projection.rs:1474`<br>`crates/memstead-cli/src/commands/projection.rs:1627` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:402`<br>`crates/memstead-cli/src/commands/projection.rs:500` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:832` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:858`<br>`crates/memstead-cli/src/commands/projection.rs:1049`<br>`crates/memstead-cli/src/commands/projection.rs:1161`<br>`crates/memstead-cli/src/commands/projection.rs:1170`<br>`crates/memstead-cli/src/commands/projection.rs:1180` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:1101` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:825` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:837` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:820` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:419`<br>`crates/memstead-cli/src/commands/projection.rs:942`<br>`crates/memstead-cli/src/commands/projection.rs:1322` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1357` |
| `PROJECTION_QUARANTINED` | CLI | `crates/memstead-cli/src/commands/projection.rs:926` |
| `PROJECTION_STORE_LEGACY` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1558` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1855` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1882` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1771`<br>`crates/memstead-cli/src/commands/projection.rs:1834` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1144`<br>`crates/memstead-mcp/src/server.rs:862` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1721` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1729` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:1755` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:225` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1146`<br>`crates/memstead-mcp/src/server.rs:936` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:544`<br>`crates/memstead-cli/src/commands/unpublish.rs:100` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:539`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1177`<br>`crates/memstead-mcp/src/server.rs:1104` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1168`<br>`crates/memstead-mcp/src/server.rs:1346` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1200`<br>`crates/memstead-mcp/src/server.rs:1511` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1165`<br>`crates/memstead-mcp/src/server.rs:1551` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1162`<br>`crates/memstead-mcp/src/filesystem_server.rs:513`<br>`crates/memstead-mcp/src/server.rs:1525` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1167`<br>`crates/memstead-mcp/src/server.rs:1568` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1161`<br>`crates/memstead-mcp/src/server.rs:1077` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1179`<br>`crates/memstead-mcp/src/server.rs:1146` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1752` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1195`<br>`crates/memstead-mcp/src/server.rs:1675` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1758` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1757` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:1745` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1746` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1187`<br>`crates/memstead-cli/src/commands/schema.rs:773`<br>`crates/memstead-cli/src/commands/schema.rs:807`<br>`crates/memstead-cli/src/commands/schema.rs:823`<br>`crates/memstead-mcp/src/server.rs:1379` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:132` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1743` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1189`<br>`crates/memstead-mcp/src/server.rs:1400` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1188`<br>`crates/memstead-cli/src/commands/schema.rs:559`<br>`crates/memstead-cli/src/commands/schema.rs:584`<br>`crates/memstead-cli/src/commands/schema.rs:729`<br>`crates/memstead-cli/src/commands/schema.rs:741`<br>`crates/memstead-mcp/src/server.rs:1388` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1143`<br>`crates/memstead-mcp/src/server.rs:923` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1733` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1720` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1202`<br>`crates/memstead-mcp/src/server.rs:1620` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:202`<br>`crates/memstead-base/src/runtime_validator.rs:203`<br>`crates/memstead-base/src/section_format.rs:523` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:520` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1747` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1750` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1178`<br>`crates/memstead-mcp/src/server.rs:1195` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:1803` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1170`<br>`crates/memstead-mcp/src/server.rs:1267` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1697` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1172`<br>`crates/memstead-mcp/src/server.rs:1285` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1171`<br>`crates/memstead-mcp/src/server.rs:1276` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1735` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:165`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:1695` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1694` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1734` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:256` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1689` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1148`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:312`<br>`crates/memstead-mcp/src/server.rs:959` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1711` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1692` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1136`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:350`<br>`crates/memstead-cli/src/commands/export.rs:140`<br>`crates/memstead-cli/src/commands/export.rs:260`<br>`crates/memstead-cli/src/commands/export.rs:382`<br>`crates/memstead-cli/src/commands/uninstall.rs:36`<br>`crates/memstead-mcp/src/filesystem_server.rs:1841`<br>`crates/memstead-mcp/src/server.rs:805`<br>`crates/memstead-mcp/src/server.rs:2278`<br>`crates/memstead-mcp/src/server.rs:2385`<br>`crates/memstead-mcp/src/server.rs:3371` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:196` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1727` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1138`<br>`crates/memstead-mcp/src/server.rs:832` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1139`<br>`crates/memstead-mcp/src/server.rs:875` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:827` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1696` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1169`<br>`crates/memstead-mcp/src/server.rs:1460` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:270` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:2018`<br>`crates/memstead-base/src/workspace_store.rs:157`<br>`crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/export.rs:403`<br>`crates/memstead-cli/src/commands/publish.rs:390`<br>`crates/memstead-cli/src/commands/workspace.rs:736`<br>`crates/memstead-cli/src/setup.rs:41`<br>`crates/memstead-mcp/src/server.rs:4282` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:160` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:158` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:159` |
