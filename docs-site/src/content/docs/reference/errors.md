---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 217

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1916` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:463`<br>`crates/memstead-cli/src/commands/publish.rs:232`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ANCHORS_SIDECAR_UNREADABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2081` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:257`<br>`crates/memstead-cli/src/commands/publish.rs:331` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:374` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:367`<br>`crates/memstead-cli/src/commands/publish.rs:627` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:425`<br>`crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1904` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:117`<br>`crates/memstead-mcp/src/filesystem_server.rs:1583`<br>`crates/memstead-mcp/src/server.rs:3102` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1231`<br>`crates/memstead-mcp/src/server.rs:844` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:2096` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1233`<br>`crates/memstead-mcp/src/server.rs:944` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:183`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1820` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1262`<br>`crates/memstead-mcp/src/filesystem_server.rs:696`<br>`crates/memstead-mcp/src/server.rs:1085` |
| `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND` | engine | `crates/memstead-base/src/engine/error.rs:1281` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1237`<br>`crates/memstead-base/src/ops/mod.rs:1895` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1246` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1244`<br>`crates/memstead-mcp/src/filesystem_server.rs:459` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1840` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1245`<br>`crates/memstead-mcp/src/filesystem_server.rs:468` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:1905` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1289`<br>`crates/memstead-base/src/ops/mod.rs:1918`<br>`crates/memstead-mcp/src/filesystem_server.rs:901`<br>`crates/memstead-mcp/src/server.rs:1519` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:390` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:414` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1221`<br>`crates/memstead-mcp/src/server.rs:1624` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1844` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1896` |
| `EMBEDDED_SCHEMA_INVALID` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1274`<br>`crates/memstead-cli/src/commands/install.rs:238`<br>`crates/memstead-mcp/src/server.rs:1408` |
| `EMPTY_UNDECLARED_HEADING` | engine | `crates/memstead-base/src/runtime_validator.rs:224` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1249`<br>`crates/memstead-mcp/src/filesystem_server.rs:1003`<br>`crates/memstead-mcp/src/server.rs:1692` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:1900` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1236`<br>`crates/memstead-mcp/src/filesystem_server.rs:355`<br>`crates/memstead-mcp/src/server.rs:765` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1240`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:58`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:745`<br>`crates/memstead-cli/src/commands/update.rs:769`<br>`crates/memstead-mcp/src/filesystem_server.rs:370`<br>`crates/memstead-mcp/src/filesystem_server.rs:1071`<br>`crates/memstead-mcp/src/filesystem_server.rs:1962`<br>`crates/memstead-mcp/src/server.rs:755`<br>`crates/memstead-mcp/src/server.rs:1903`<br>`crates/memstead-mcp/src/server.rs:2517` |
| `EXPECTED_HASH_REQUIRED` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1355`<br>`crates/memstead-mcp/src/server.rs:2850` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1870` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1886` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1867` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1871` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:69` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1912` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:645` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1241`<br>`crates/memstead-mcp/src/server.rs:786` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1242` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:1466` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1891` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1839` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:28`<br>`crates/memstead-mcp/src/filesystem_server.rs:1857`<br>`crates/memstead-mcp/src/filesystem_server.rs:1926` |
| `INTERNAL_IO_ERROR` | CLI | `crates/memstead-cli/src/commands/install.rs:81`<br>`crates/memstead-cli/src/commands/quickstart.rs:230`<br>`crates/memstead-cli/src/commands/quickstart.rs:370`<br>`crates/memstead-cli/src/commands/quickstart.rs:674`<br>`crates/memstead-cli/src/commands/quickstart.rs:857`<br>`crates/memstead-cli/src/commands/quickstart.rs:986`<br>`crates/memstead-cli/src/commands/quickstart.rs:1096`<br>`crates/memstead-cli/src/commands/quickstart.rs:1108`<br>`crates/memstead-cli/src/setup.rs:706` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:67` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1285`<br>`crates/memstead-mcp/src/filesystem_server.rs:1018`<br>`crates/memstead-mcp/src/server.rs:1706` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1259`<br>`crates/memstead-mcp/src/filesystem_server.rs:647`<br>`crates/memstead-mcp/src/server.rs:310`<br>`crates/memstead-mcp/src/server.rs:325`<br>`crates/memstead-mcp/src/server.rs:1318` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1872`<br>`crates/memstead-base/src/runtime_validator.rs:219` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:227` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1279`<br>`crates/memstead-base/src/engine/error.rs:1284`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:146`<br>`crates/memstead-cli/src/commands/batch.rs:153`<br>`crates/memstead-cli/src/commands/batch.rs:170`<br>`crates/memstead-cli/src/commands/batch.rs:187`<br>`crates/memstead-cli/src/commands/batch.rs:202`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:208`<br>`crates/memstead-cli/src/commands/batch_relate.rs:84`<br>`crates/memstead-cli/src/commands/batch_update.rs:235`<br>`crates/memstead-cli/src/commands/batch_update.rs:246`<br>`crates/memstead-cli/src/commands/batch_update.rs:383`<br>`crates/memstead-cli/src/commands/conflicts.rs:100`<br>`crates/memstead-cli/src/commands/create.rs:166`<br>`crates/memstead-cli/src/commands/create.rs:173`<br>`crates/memstead-cli/src/commands/create.rs:189`<br>`crates/memstead-cli/src/commands/create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:236`<br>`crates/memstead-cli/src/commands/create.rs:374`<br>`crates/memstead-cli/src/commands/create.rs:453`<br>`crates/memstead-cli/src/commands/create.rs:476`<br>`crates/memstead-cli/src/commands/create.rs:491`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/export.rs:124`<br>`crates/memstead-cli/src/commands/export.rs:156`<br>`crates/memstead-cli/src/commands/export.rs:698`<br>`crates/memstead-cli/src/commands/export.rs:703`<br>`crates/memstead-cli/src/commands/export.rs:735`<br>`crates/memstead-cli/src/commands/export.rs:743`<br>`crates/memstead-cli/src/commands/install.rs:61`<br>`crates/memstead-cli/src/commands/mem.rs:1130`<br>`crates/memstead-cli/src/commands/mod.rs:113`<br>`crates/memstead-cli/src/commands/mod.rs:120`<br>`crates/memstead-cli/src/commands/projection.rs:1711`<br>`crates/memstead-cli/src/commands/publish.rs:128`<br>`crates/memstead-cli/src/commands/publish.rs:136`<br>`crates/memstead-cli/src/commands/publish.rs:158`<br>`crates/memstead-cli/src/commands/quickstart.rs:192`<br>`crates/memstead-cli/src/commands/quickstart.rs:212`<br>`crates/memstead-cli/src/commands/quickstart.rs:726`<br>`crates/memstead-cli/src/commands/quickstart.rs:750`<br>`crates/memstead-cli/src/commands/quickstart.rs:758`<br>`crates/memstead-cli/src/commands/quickstart.rs:829`<br>`crates/memstead-cli/src/commands/quickstart.rs:993`<br>`crates/memstead-cli/src/commands/quickstart.rs:1003`<br>`crates/memstead-cli/src/commands/quickstart.rs:1015`<br>`crates/memstead-cli/src/commands/quickstart.rs:1066`<br>`crates/memstead-cli/src/commands/relate.rs:85`<br>`crates/memstead-cli/src/commands/relate.rs:90`<br>`crates/memstead-cli/src/commands/schema.rs:147`<br>`crates/memstead-cli/src/commands/schema.rs:240`<br>`crates/memstead-cli/src/commands/schema.rs:1009`<br>`crates/memstead-cli/src/commands/schema.rs:1041`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:284`<br>`crates/memstead-cli/src/commands/update.rs:297`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:320`<br>`crates/memstead-cli/src/commands/update.rs:341`<br>`crates/memstead-cli/src/commands/update.rs:380`<br>`crates/memstead-cli/src/commands/update.rs:527`<br>`crates/memstead-cli/src/commands/update.rs:535`<br>`crates/memstead-cli/src/commands/update.rs:543`<br>`crates/memstead-cli/src/commands/update.rs:827`<br>`crates/memstead-cli/src/commands/update.rs:834`<br>`crates/memstead-cli/src/commands/update.rs:856`<br>`crates/memstead-cli/src/commands/update.rs:875`<br>`crates/memstead-cli/src/commands/update.rs:882`<br>`crates/memstead-cli/src/commands/update.rs:889`<br>`crates/memstead-cli/src/commands/workspace.rs:647`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:951`<br>`crates/memstead-mcp/src/filesystem_server.rs:1462`<br>`crates/memstead-mcp/src/filesystem_server.rs:1788`<br>`crates/memstead-mcp/src/filesystem_server.rs:1942`<br>`crates/memstead-mcp/src/filesystem_server.rs:1977`<br>`crates/memstead-mcp/src/filesystem_server.rs:2292`<br>`crates/memstead-mcp/src/server.rs:361`<br>`crates/memstead-mcp/src/server.rs:414`<br>`crates/memstead-mcp/src/server.rs:1451`<br>`crates/memstead-mcp/src/server.rs:1474`<br>`crates/memstead-mcp/src/server.rs:2165`<br>`crates/memstead-mcp/src/server.rs:2349`<br>`crates/memstead-mcp/src/server.rs:2395`<br>`crates/memstead-mcp/src/server.rs:2432`<br>`crates/memstead-mcp/src/server.rs:2448`<br>`crates/memstead-mcp/src/server.rs:2561`<br>`crates/memstead-mcp/src/server.rs:2945`<br>`crates/memstead-mcp/src/server.rs:3560`<br>`crates/memstead-mcp/src/server.rs:3703`<br>`crates/memstead-mcp/src/server.rs:3796`<br>`crates/memstead-mcp/src/server.rs:3853`<br>`crates/memstead-mcp/src/server.rs:3952`<br>`crates/memstead-mcp/src/server.rs:3991`<br>`crates/memstead-mcp/src/server.rs:4020` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1261`<br>`crates/memstead-mcp/src/filesystem_server.rs:683`<br>`crates/memstead-mcp/src/server.rs:1352`<br>`crates/memstead-mcp/src/server.rs:1774` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:223` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:222` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/filesystem_server.rs:247`<br>`crates/memstead-mcp/src/server.rs:191` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:523` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1235`<br>`crates/memstead-cli/src/commands/batch_create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:227`<br>`crates/memstead-mcp/src/filesystem_server.rs:349`<br>`crates/memstead-mcp/src/server.rs:1285` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:41`<br>`crates/memstead-mcp/src/filesystem_server.rs:2212`<br>`crates/memstead-mcp/src/server.rs:3319` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:144` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1260`<br>`crates/memstead-mcp/src/filesystem_server.rs:663`<br>`crates/memstead-mcp/src/server.rs:1333` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:657`<br>`crates/memstead-cli/src/commands/export.rs:771`<br>`crates/memstead-cli/src/commands/schema.rs:276`<br>`crates/memstead-cli/src/commands/schema.rs:285`<br>`crates/memstead-cli/src/commands/schema.rs:310`<br>`crates/memstead-cli/src/commands/schema.rs:322`<br>`crates/memstead-cli/src/commands/schema.rs:1121`<br>`crates/memstead-cli/src/commands/schema.rs:1130` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:161` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1847` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1226`<br>`crates/memstead-mcp/src/server.rs:883` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1228`<br>`crates/memstead-mcp/src/server.rs:905` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:549` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1295`<br>`crates/memstead-mcp/src/filesystem_server.rs:994`<br>`crates/memstead-mcp/src/server.rs:1679` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1287`<br>`crates/memstead-mcp/src/filesystem_server.rs:970`<br>`crates/memstead-mcp/src/server.rs:1490` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1272`<br>`crates/memstead-base/src/engine/error.rs:1277`<br>`crates/memstead-cli/src/commands/workspace.rs:767`<br>`crates/memstead-cli/src/commands/workspace.rs:774`<br>`crates/memstead-mcp/src/filesystem_server.rs:864`<br>`crates/memstead-mcp/src/server.rs:1442`<br>`crates/memstead-mcp/src/server.rs:1654` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1909` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1243` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1278`<br>`crates/memstead-mcp/src/server.rs:1391` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:48` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1769` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1223`<br>`crates/memstead-mcp/src/server.rs:818` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1910` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1808` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1897` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:903` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1791` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1836` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1288`<br>`crates/memstead-base/src/ops/mod.rs:1917`<br>`crates/memstead-mcp/src/filesystem_server.rs:888`<br>`crates/memstead-mcp/src/server.rs:1536` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1842` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1238`<br>`crates/memstead-base/src/ops/mod.rs:1893` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1266`<br>`crates/memstead-base/src/ops/mod.rs:1841`<br>`crates/memstead-mcp/src/filesystem_server.rs:837`<br>`crates/memstead-mcp/src/server.rs:1213` |
| `MOUNT_UNBACKED` | engine | `crates/memstead-base/src/ops/mod.rs:1899` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1873` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:632`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1227`<br>`crates/memstead-mcp/src/server.rs:892` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1890` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:298`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NOT_CONFLICTED` | engine | `crates/memstead-base/src/engine/error.rs:1283` |
| `NO_ACTIVE_BINDING` | CLI | `crates/memstead-cli/src/commands/projection.rs:1652` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1845` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:801` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:183`<br>`crates/memstead-cli/src/commands/changes.rs:65`<br>`crates/memstead-cli/src/commands/create.rs:514`<br>`crates/memstead-cli/src/commands/export.rs:456` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1892` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1907` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1270`<br>`crates/memstead-base/src/engine/error.rs:1271`<br>`crates/memstead-mcp/src/filesystem_server.rs:866`<br>`crates/memstead-mcp/src/filesystem_server.rs:868`<br>`crates/memstead-mcp/src/server.rs:1636`<br>`crates/memstead-mcp/src/server.rs:1645` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1268`<br>`crates/memstead-mcp/src/filesystem_server.rs:850`<br>`crates/memstead-mcp/src/server.rs:1241` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1267`<br>`crates/memstead-mcp/src/filesystem_server.rs:840`<br>`crates/memstead-mcp/src/server.rs:1228` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1733`<br>`crates/memstead-cli/src/commands/projection.rs:1778`<br>`crates/memstead-cli/src/commands/projection.rs:1813` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1768` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:637` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:577` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:540`<br>`crates/memstead-cli/src/commands/projection.rs:1565`<br>`crates/memstead-cli/src/commands/projection.rs:2131` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1433` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1910`<br>`crates/memstead-cli/src/commands/projection.rs:1944` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:1905` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:870` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:583` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:819`<br>`crates/memstead-cli/src/commands/quickstart.rs:596` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1799` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1931` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:591`<br>`crates/memstead-cli/src/commands/projection.rs:844`<br>`crates/memstead-cli/src/commands/projection.rs:1416`<br>`crates/memstead-cli/src/commands/projection.rs:1726`<br>`crates/memstead-cli/src/commands/projection.rs:1746`<br>`crates/memstead-cli/src/commands/projection.rs:1900` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:571`<br>`crates/memstead-cli/src/commands/projection.rs:654`<br>`crates/memstead-cli/src/commands/projection.rs:700`<br>`crates/memstead-cli/src/commands/projection.rs:1662` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:981` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1007`<br>`crates/memstead-cli/src/commands/projection.rs:1198`<br>`crates/memstead-cli/src/commands/projection.rs:1310`<br>`crates/memstead-cli/src/commands/projection.rs:1319`<br>`crates/memstead-cli/src/commands/projection.rs:1329` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:1250` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:974` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:986` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:969` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:588`<br>`crates/memstead-cli/src/commands/projection.rs:1091`<br>`crates/memstead-cli/src/commands/projection.rs:1471` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1506` |
| `PROJECTION_QUARANTINED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1075` |
| `PROJECTION_SCOPE_UNINTERPRETABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:598`<br>`crates/memstead-cli/src/commands/projection.rs:1730` |
| `PROJECTION_STORE_LEGACY` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:550` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2265` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2277` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2051`<br>`crates/memstead-cli/src/commands/projection.rs:2142` |
| `PROJECTION_VERIFY_FINDINGS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2298` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1230`<br>`crates/memstead-mcp/src/server.rs:861` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1875` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1883` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:1911` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:250` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:220` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1232`<br>`crates/memstead-mcp/src/server.rs:935` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:642`<br>`crates/memstead-cli/src/commands/unpublish.rs:100`<br>`crates/memstead-cli/src/registry/mod.rs:92` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:637`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1263`<br>`crates/memstead-mcp/src/filesystem_server.rs:734`<br>`crates/memstead-mcp/src/server.rs:1120` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1254`<br>`crates/memstead-mcp/src/server.rs:1370` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1291`<br>`crates/memstead-mcp/src/filesystem_server.rs:915`<br>`crates/memstead-mcp/src/server.rs:1554` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1251`<br>`crates/memstead-mcp/src/server.rs:1594` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1248`<br>`crates/memstead-mcp/src/filesystem_server.rs:510`<br>`crates/memstead-mcp/src/server.rs:1568` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1253`<br>`crates/memstead-mcp/src/server.rs:1611` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1247`<br>`crates/memstead-mcp/src/server.rs:1076` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1265`<br>`crates/memstead-mcp/src/filesystem_server.rs:779`<br>`crates/memstead-mcp/src/server.rs:1152` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1908` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1286`<br>`crates/memstead-mcp/src/filesystem_server.rs:1025`<br>`crates/memstead-mcp/src/server.rs:1717` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1914` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1913` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:1901` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1902` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1273`<br>`crates/memstead-cli/src/commands/schema.rs:166`<br>`crates/memstead-cli/src/commands/schema.rs:215`<br>`crates/memstead-cli/src/commands/schema.rs:982`<br>`crates/memstead-cli/src/commands/schema.rs:1016`<br>`crates/memstead-cli/src/commands/schema.rs:1032`<br>`crates/memstead-mcp/src/server.rs:1403` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:260` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1898` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1276`<br>`crates/memstead-mcp/src/server.rs:1433` |
| `SCHEMA_UNSTAMPED_SOURCE_ROT` | engine | `crates/memstead-base/src/ops/mod.rs:1915` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1275`<br>`crates/memstead-cli/src/commands/schema.rs:733`<br>`crates/memstead-cli/src/commands/schema.rs:761`<br>`crates/memstead-cli/src/commands/schema.rs:786`<br>`crates/memstead-cli/src/commands/schema.rs:931`<br>`crates/memstead-cli/src/commands/schema.rs:943`<br>`crates/memstead-mcp/src/server.rs:1421` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1229`<br>`crates/memstead-mcp/src/server.rs:922` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1887` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1874` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1293`<br>`crates/memstead-mcp/src/filesystem_server.rs:980`<br>`crates/memstead-mcp/src/server.rs:1663` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:225`<br>`crates/memstead-base/src/runtime_validator.rs:226`<br>`crates/memstead-base/src/section_format.rs:524` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1903` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:522` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:221` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1906` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1264`<br>`crates/memstead-mcp/src/filesystem_server.rs:739`<br>`crates/memstead-mcp/src/server.rs:1219` |
| `SIGNAL_THRESHOLD_CROSSED` | engine | `crates/memstead-base/src/ops/mod.rs:1894` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2110` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1256`<br>`crates/memstead-mcp/src/server.rs:1291` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1851` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1258`<br>`crates/memstead-mcp/src/server.rs:1309` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1257`<br>`crates/memstead-mcp/src/server.rs:1300` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1889` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:293`<br>`crates/memstead-cli/src/lib.rs:38` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:1849` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1848` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1888` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:255` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1843` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1234`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:309`<br>`crates/memstead-mcp/src/filesystem_server.rs:2012`<br>`crates/memstead-mcp/src/server.rs:958`<br>`crates/memstead-mcp/src/server.rs:2603` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1865` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1846` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1222`<br>`crates/memstead-cli/src/commands/changes.rs:225`<br>`crates/memstead-cli/src/commands/create.rs:351`<br>`crates/memstead-cli/src/commands/export.rs:174`<br>`crates/memstead-cli/src/commands/export.rs:305`<br>`crates/memstead-cli/src/commands/export.rs:496`<br>`crates/memstead-cli/src/commands/uninstall.rs:36`<br>`crates/memstead-mcp/src/filesystem_server.rs:1950`<br>`crates/memstead-mcp/src/filesystem_server.rs:2304`<br>`crates/memstead-mcp/src/server.rs:804`<br>`crates/memstead-mcp/src/server.rs:2371`<br>`crates/memstead-mcp/src/server.rs:2477`<br>`crates/memstead-mcp/src/server.rs:3542` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:218` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1881` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1224`<br>`crates/memstead-mcp/src/server.rs:831` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1225`<br>`crates/memstead-mcp/src/server.rs:874` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:217` |
| `UNSUPPORTED_PARAM` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:273` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:861` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1850` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1255`<br>`crates/memstead-mcp/src/server.rs:1503` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:633` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:469` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI | `crates/memstead-base/src/engine/error.rs:2187`<br>`crates/memstead-base/src/workspace_store.rs:157`<br>`crates/memstead-cli/src/commands/changes.rs:246`<br>`crates/memstead-cli/src/commands/publish.rs:488`<br>`crates/memstead-cli/src/setup.rs:41` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:160` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:158` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:159` |
