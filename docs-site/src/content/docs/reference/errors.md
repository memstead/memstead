---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 225

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1972` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:503`<br>`crates/memstead-cli/src/commands/publish.rs:242`<br>`crates/memstead-cli/src/commands/type_cmd.rs:152` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ANCHORS_SIDECAR_UNREADABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2085` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:267`<br>`crates/memstead-cli/src/commands/publish.rs:341` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:384` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:377`<br>`crates/memstead-cli/src/commands/publish.rs:637` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:462`<br>`crates/memstead-cli/src/lib.rs:55` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1960` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:117`<br>`crates/memstead-mcp/src/filesystem_server.rs:1613`<br>`crates/memstead-mcp/src/server.rs:3127` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1261`<br>`crates/memstead-mcp/src/server.rs:869` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:2152` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1263`<br>`crates/memstead-mcp/src/server.rs:969` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:183`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:43` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1845` |
| `CONFIG_WRITE_INTERVENED` | engine | `crates/memstead-base/src/ops/mod.rs:1952` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1292`<br>`crates/memstead-mcp/src/filesystem_server.rs:718`<br>`crates/memstead-mcp/src/server.rs:1110` |
| `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND` | engine | `crates/memstead-base/src/engine/error.rs:1314` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1267`<br>`crates/memstead-base/src/ops/mod.rs:1949` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1276` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1274`<br>`crates/memstead-mcp/src/filesystem_server.rs:481` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1894` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1275`<br>`crates/memstead-mcp/src/filesystem_server.rs:490` |
| `DANGLING_LINK_NOT_RELATED` | engine | `crates/memstead-base/src/ops/mod.rs:3533` |
| `DANGLING_LINK_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3532` |
| `DANGLING_RELATION_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3534` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:1961` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1322`<br>`crates/memstead-base/src/ops/mod.rs:1974`<br>`crates/memstead-mcp/src/filesystem_server.rs:923`<br>`crates/memstead-mcp/src/server.rs:1544` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:400` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:424` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1251`<br>`crates/memstead-mcp/src/server.rs:1649` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1898` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1950` |
| `EMBEDDED_SCHEMA_INVALID` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1307`<br>`crates/memstead-cli/src/commands/install.rs:238`<br>`crates/memstead-mcp/src/server.rs:1433` |
| `EMPTY_UNDECLARED_HEADING` | engine | `crates/memstead-base/src/runtime_validator.rs:247` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1279`<br>`crates/memstead-mcp/src/filesystem_server.rs:1025`<br>`crates/memstead-mcp/src/server.rs:1717` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:1956` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1266`<br>`crates/memstead-mcp/src/filesystem_server.rs:377`<br>`crates/memstead-mcp/src/server.rs:779` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1270`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:58`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:139`<br>`crates/memstead-cli/src/commands/rename.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:745`<br>`crates/memstead-cli/src/commands/update.rs:769`<br>`crates/memstead-mcp/src/filesystem_server.rs:392`<br>`crates/memstead-mcp/src/filesystem_server.rs:1093`<br>`crates/memstead-mcp/src/filesystem_server.rs:2018`<br>`crates/memstead-mcp/src/server.rs:769`<br>`crates/memstead-mcp/src/server.rs:1928`<br>`crates/memstead-mcp/src/server.rs:2542` |
| `EXPECTED_HASH_REQUIRED` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1379`<br>`crates/memstead-mcp/src/server.rs:2875` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1924` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1940` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1921` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1925` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:69` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:1968` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:645` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:34` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1271`<br>`crates/memstead-mcp/src/server.rs:811` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1272` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:1583` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1945` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1893` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:29`<br>`crates/memstead-mcp/src/filesystem_server.rs:1889`<br>`crates/memstead-mcp/src/filesystem_server.rs:1952`<br>`crates/memstead-mcp/src/filesystem_server.rs:1982` |
| `INTERNAL_IO_ERROR` | CLI | `crates/memstead-cli/src/commands/install.rs:81`<br>`crates/memstead-cli/src/commands/quickstart.rs:230`<br>`crates/memstead-cli/src/commands/quickstart.rs:370`<br>`crates/memstead-cli/src/commands/quickstart.rs:674`<br>`crates/memstead-cli/src/commands/quickstart.rs:857`<br>`crates/memstead-cli/src/commands/quickstart.rs:986`<br>`crates/memstead-cli/src/commands/quickstart.rs:1096`<br>`crates/memstead-cli/src/commands/quickstart.rs:1108`<br>`crates/memstead-cli/src/setup.rs:706` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:67` |
| `INVALID_CHECK_KIND` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:65`<br>`crates/memstead-mcp/src/filesystem_server.rs:2285`<br>`crates/memstead-mcp/src/server.rs:3363` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1318`<br>`crates/memstead-mcp/src/filesystem_server.rs:1040`<br>`crates/memstead-mcp/src/server.rs:1731` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1289`<br>`crates/memstead-mcp/src/filesystem_server.rs:669`<br>`crates/memstead-mcp/src/server.rs:310`<br>`crates/memstead-mcp/src/server.rs:325`<br>`crates/memstead-mcp/src/server.rs:1343` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1926`<br>`crates/memstead-base/src/runtime_validator.rs:242` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:251` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1312`<br>`crates/memstead-base/src/engine/error.rs:1317`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:146`<br>`crates/memstead-cli/src/commands/batch.rs:153`<br>`crates/memstead-cli/src/commands/batch.rs:170`<br>`crates/memstead-cli/src/commands/batch.rs:187`<br>`crates/memstead-cli/src/commands/batch.rs:202`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:208`<br>`crates/memstead-cli/src/commands/batch_relate.rs:84`<br>`crates/memstead-cli/src/commands/batch_update.rs:235`<br>`crates/memstead-cli/src/commands/batch_update.rs:246`<br>`crates/memstead-cli/src/commands/batch_update.rs:383`<br>`crates/memstead-cli/src/commands/conflicts.rs:100`<br>`crates/memstead-cli/src/commands/create.rs:166`<br>`crates/memstead-cli/src/commands/create.rs:173`<br>`crates/memstead-cli/src/commands/create.rs:189`<br>`crates/memstead-cli/src/commands/create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:236`<br>`crates/memstead-cli/src/commands/create.rs:374`<br>`crates/memstead-cli/src/commands/create.rs:453`<br>`crates/memstead-cli/src/commands/create.rs:476`<br>`crates/memstead-cli/src/commands/create.rs:491`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/export.rs:124`<br>`crates/memstead-cli/src/commands/export.rs:156`<br>`crates/memstead-cli/src/commands/export.rs:738`<br>`crates/memstead-cli/src/commands/export.rs:743`<br>`crates/memstead-cli/src/commands/export.rs:775`<br>`crates/memstead-cli/src/commands/export.rs:783`<br>`crates/memstead-cli/src/commands/install.rs:61`<br>`crates/memstead-cli/src/commands/mem.rs:1147`<br>`crates/memstead-cli/src/commands/mod.rs:113`<br>`crates/memstead-cli/src/commands/mod.rs:120`<br>`crates/memstead-cli/src/commands/projection.rs:1715`<br>`crates/memstead-cli/src/commands/publish.rs:128`<br>`crates/memstead-cli/src/commands/publish.rs:136`<br>`crates/memstead-cli/src/commands/publish.rs:158`<br>`crates/memstead-cli/src/commands/quickstart.rs:192`<br>`crates/memstead-cli/src/commands/quickstart.rs:212`<br>`crates/memstead-cli/src/commands/quickstart.rs:726`<br>`crates/memstead-cli/src/commands/quickstart.rs:750`<br>`crates/memstead-cli/src/commands/quickstart.rs:758`<br>`crates/memstead-cli/src/commands/quickstart.rs:829`<br>`crates/memstead-cli/src/commands/quickstart.rs:993`<br>`crates/memstead-cli/src/commands/quickstart.rs:1003`<br>`crates/memstead-cli/src/commands/quickstart.rs:1015`<br>`crates/memstead-cli/src/commands/quickstart.rs:1066`<br>`crates/memstead-cli/src/commands/relate.rs:85`<br>`crates/memstead-cli/src/commands/relate.rs:90`<br>`crates/memstead-cli/src/commands/schema.rs:147`<br>`crates/memstead-cli/src/commands/schema.rs:240`<br>`crates/memstead-cli/src/commands/schema.rs:1009`<br>`crates/memstead-cli/src/commands/schema.rs:1041`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:173`<br>`crates/memstead-cli/src/commands/update.rs:284`<br>`crates/memstead-cli/src/commands/update.rs:297`<br>`crates/memstead-cli/src/commands/update.rs:313`<br>`crates/memstead-cli/src/commands/update.rs:320`<br>`crates/memstead-cli/src/commands/update.rs:341`<br>`crates/memstead-cli/src/commands/update.rs:380`<br>`crates/memstead-cli/src/commands/update.rs:527`<br>`crates/memstead-cli/src/commands/update.rs:535`<br>`crates/memstead-cli/src/commands/update.rs:543`<br>`crates/memstead-cli/src/commands/update.rs:827`<br>`crates/memstead-cli/src/commands/update.rs:834`<br>`crates/memstead-cli/src/commands/update.rs:856`<br>`crates/memstead-cli/src/commands/update.rs:875`<br>`crates/memstead-cli/src/commands/update.rs:882`<br>`crates/memstead-cli/src/commands/update.rs:889`<br>`crates/memstead-cli/src/commands/workspace.rs:717`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:973`<br>`crates/memstead-mcp/src/filesystem_server.rs:1490`<br>`crates/memstead-mcp/src/filesystem_server.rs:1818`<br>`crates/memstead-mcp/src/filesystem_server.rs:1998`<br>`crates/memstead-mcp/src/filesystem_server.rs:2033`<br>`crates/memstead-mcp/src/filesystem_server.rs:2379`<br>`crates/memstead-mcp/src/server.rs:361`<br>`crates/memstead-mcp/src/server.rs:414`<br>`crates/memstead-mcp/src/server.rs:1476`<br>`crates/memstead-mcp/src/server.rs:1499`<br>`crates/memstead-mcp/src/server.rs:2190`<br>`crates/memstead-mcp/src/server.rs:2374`<br>`crates/memstead-mcp/src/server.rs:2420`<br>`crates/memstead-mcp/src/server.rs:2457`<br>`crates/memstead-mcp/src/server.rs:2473`<br>`crates/memstead-mcp/src/server.rs:2586`<br>`crates/memstead-mcp/src/server.rs:2970`<br>`crates/memstead-mcp/src/server.rs:3618`<br>`crates/memstead-mcp/src/server.rs:3761`<br>`crates/memstead-mcp/src/server.rs:3854`<br>`crates/memstead-mcp/src/server.rs:3911`<br>`crates/memstead-mcp/src/server.rs:4010`<br>`crates/memstead-mcp/src/server.rs:4049`<br>`crates/memstead-mcp/src/server.rs:4078` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1291`<br>`crates/memstead-mcp/src/filesystem_server.rs:705`<br>`crates/memstead-mcp/src/server.rs:1377`<br>`crates/memstead-mcp/src/server.rs:1799` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:246` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:245` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/filesystem_server.rs:263`<br>`crates/memstead-mcp/src/server.rs:191` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:523` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1265`<br>`crates/memstead-cli/src/commands/batch_create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:227`<br>`crates/memstead-mcp/src/filesystem_server.rs:371`<br>`crates/memstead-mcp/src/server.rs:1310` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:51`<br>`crates/memstead-mcp/src/filesystem_server.rs:2270`<br>`crates/memstead-mcp/src/server.rs:3344` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:144` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1290`<br>`crates/memstead-mcp/src/filesystem_server.rs:685`<br>`crates/memstead-mcp/src/server.rs:1358` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:697`<br>`crates/memstead-cli/src/commands/export.rs:811`<br>`crates/memstead-cli/src/commands/schema.rs:276`<br>`crates/memstead-cli/src/commands/schema.rs:285`<br>`crates/memstead-cli/src/commands/schema.rs:310`<br>`crates/memstead-cli/src/commands/schema.rs:322`<br>`crates/memstead-cli/src/commands/schema.rs:1121`<br>`crates/memstead-cli/src/commands/schema.rs:1130` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:161` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1901` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1256`<br>`crates/memstead-mcp/src/server.rs:908` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1258`<br>`crates/memstead-mcp/src/server.rs:930` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:559` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1328`<br>`crates/memstead-mcp/src/filesystem_server.rs:1016`<br>`crates/memstead-mcp/src/server.rs:1704` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1320`<br>`crates/memstead-mcp/src/filesystem_server.rs:992`<br>`crates/memstead-mcp/src/server.rs:1515` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1305`<br>`crates/memstead-base/src/engine/error.rs:1310`<br>`crates/memstead-cli/src/commands/workspace.rs:899`<br>`crates/memstead-cli/src/commands/workspace.rs:906`<br>`crates/memstead-mcp/src/filesystem_server.rs:886`<br>`crates/memstead-mcp/src/server.rs:1467`<br>`crates/memstead-mcp/src/server.rs:1679` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1965` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1273` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1311`<br>`crates/memstead-mcp/src/server.rs:1416` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:48` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1794` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1253`<br>`crates/memstead-mcp/src/server.rs:843` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1966` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1833` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1951` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:903` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1816` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1861` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1321`<br>`crates/memstead-base/src/ops/mod.rs:1973`<br>`crates/memstead-mcp/src/filesystem_server.rs:910`<br>`crates/memstead-mcp/src/server.rs:1561` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1896` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1268`<br>`crates/memstead-base/src/ops/mod.rs:1947` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1296`<br>`crates/memstead-base/src/ops/mod.rs:1895`<br>`crates/memstead-mcp/src/filesystem_server.rs:859`<br>`crates/memstead-mcp/src/server.rs:1238` |
| `MOUNT_UNBACKED` | engine | `crates/memstead-base/src/ops/mod.rs:1955` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1927` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:642`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1257`<br>`crates/memstead-mcp/src/server.rs:917` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1944` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:308`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NOT_CONFLICTED` | engine | `crates/memstead-base/src/engine/error.rs:1316` |
| `NO_ACTIVE_BINDING` | CLI | `crates/memstead-cli/src/commands/projection.rs:1656` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1899` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:801` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:183`<br>`crates/memstead-cli/src/commands/changes.rs:69`<br>`crates/memstead-cli/src/commands/create.rs:514`<br>`crates/memstead-cli/src/commands/export.rs:496` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1946` |
| `OUT_OF_BAND_EDITS_UNDETECTED` | engine | `crates/memstead-base/src/ops/mod.rs:1953` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1963` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1303`<br>`crates/memstead-base/src/engine/error.rs:1304`<br>`crates/memstead-mcp/src/filesystem_server.rs:888`<br>`crates/memstead-mcp/src/filesystem_server.rs:890`<br>`crates/memstead-mcp/src/server.rs:1661`<br>`crates/memstead-mcp/src/server.rs:1670` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1298`<br>`crates/memstead-mcp/src/filesystem_server.rs:872`<br>`crates/memstead-mcp/src/server.rs:1266` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1297`<br>`crates/memstead-mcp/src/filesystem_server.rs:862`<br>`crates/memstead-mcp/src/server.rs:1253` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1737`<br>`crates/memstead-cli/src/commands/projection.rs:1782`<br>`crates/memstead-cli/src/commands/projection.rs:1817` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1772` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:637` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:577` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:540`<br>`crates/memstead-cli/src/commands/projection.rs:1569`<br>`crates/memstead-cli/src/commands/projection.rs:2135` |
| `PROJECTION_ENABLE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1437` |
| `PROJECTION_EXCLUDE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1914`<br>`crates/memstead-cli/src/commands/projection.rs:1948` |
| `PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER` | CLI | `crates/memstead-cli/src/commands/projection.rs:1909` |
| `PROJECTION_EXISTS` | CLI | `crates/memstead-cli/src/commands/projection.rs:870` |
| `PROJECTION_FINDINGS_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:583` |
| `PROJECTION_INIT_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:819`<br>`crates/memstead-cli/src/commands/quickstart.rs:596` |
| `PROJECTION_INVALID_DISPOSITIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1803` |
| `PROJECTION_INVALID_EXCLUSIONS` | CLI | `crates/memstead-cli/src/commands/projection.rs:1935` |
| `PROJECTION_INVALID_NAME` | CLI | `crates/memstead-cli/src/commands/projection.rs:591`<br>`crates/memstead-cli/src/commands/projection.rs:844`<br>`crates/memstead-cli/src/commands/projection.rs:1420`<br>`crates/memstead-cli/src/commands/projection.rs:1730`<br>`crates/memstead-cli/src/commands/projection.rs:1750`<br>`crates/memstead-cli/src/commands/projection.rs:1904` |
| `PROJECTION_LOAD_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:571`<br>`crates/memstead-cli/src/commands/projection.rs:654`<br>`crates/memstead-cli/src/commands/projection.rs:700`<br>`crates/memstead-cli/src/commands/projection.rs:1666` |
| `PROJECTION_MIGRATE_DANGLING_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:981` |
| `PROJECTION_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1007`<br>`crates/memstead-cli/src/commands/projection.rs:1202`<br>`crates/memstead-cli/src/commands/projection.rs:1314`<br>`crates/memstead-cli/src/commands/projection.rs:1323`<br>`crates/memstead-cli/src/commands/projection.rs:1333` |
| `PROJECTION_MIGRATE_INERT_PROJECTION` | CLI | `crates/memstead-cli/src/commands/projection.rs:1254` |
| `PROJECTION_MIGRATE_MALFORMED_REF` | CLI | `crates/memstead-cli/src/commands/projection.rs:974` |
| `PROJECTION_MIGRATE_ORPHAN_RECORDS` | CLI | `crates/memstead-cli/src/commands/projection.rs:986` |
| `PROJECTION_MIGRATE_REFINEMENT` | CLI | `crates/memstead-cli/src/commands/projection.rs:969` |
| `PROJECTION_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/projection.rs:588`<br>`crates/memstead-cli/src/commands/projection.rs:1091`<br>`crates/memstead-cli/src/commands/projection.rs:1475` |
| `PROJECTION_OP_ALREADY_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1510` |
| `PROJECTION_QUARANTINED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1075` |
| `PROJECTION_SCOPE_UNINTERPRETABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:598`<br>`crates/memstead-cli/src/commands/projection.rs:1734` |
| `PROJECTION_STORE_LEGACY` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `PROJECTION_SYNC_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:550` |
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2276` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2288` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2055`<br>`crates/memstead-cli/src/commands/projection.rs:2146` |
| `PROJECTION_VERIFY_FINDINGS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2309` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1260`<br>`crates/memstead-mcp/src/server.rs:886` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1929` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1937` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:1967` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:250` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:243` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1262`<br>`crates/memstead-mcp/src/server.rs:960` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:652`<br>`crates/memstead-cli/src/commands/unpublish.rs:100`<br>`crates/memstead-cli/src/registry/mod.rs:92` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:647`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1293`<br>`crates/memstead-mcp/src/filesystem_server.rs:756`<br>`crates/memstead-mcp/src/server.rs:1145` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1284`<br>`crates/memstead-mcp/src/server.rs:1395` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1324`<br>`crates/memstead-mcp/src/filesystem_server.rs:937`<br>`crates/memstead-mcp/src/server.rs:1579` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1281`<br>`crates/memstead-mcp/src/server.rs:1619` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1278`<br>`crates/memstead-mcp/src/filesystem_server.rs:532`<br>`crates/memstead-mcp/src/server.rs:1593` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1283`<br>`crates/memstead-mcp/src/server.rs:1636` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1277`<br>`crates/memstead-mcp/src/server.rs:1101` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1295`<br>`crates/memstead-mcp/src/filesystem_server.rs:801`<br>`crates/memstead-mcp/src/server.rs:1177` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1964` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1319`<br>`crates/memstead-mcp/src/filesystem_server.rs:1047`<br>`crates/memstead-mcp/src/server.rs:1742` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:1970` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1969` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:1957` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:1958` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1306`<br>`crates/memstead-cli/src/commands/schema.rs:166`<br>`crates/memstead-cli/src/commands/schema.rs:215`<br>`crates/memstead-cli/src/commands/schema.rs:982`<br>`crates/memstead-cli/src/commands/schema.rs:1016`<br>`crates/memstead-cli/src/commands/schema.rs:1032`<br>`crates/memstead-mcp/src/server.rs:1428` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:260` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1954` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1309`<br>`crates/memstead-mcp/src/server.rs:1458` |
| `SCHEMA_UNSTAMPED_SOURCE_ROT` | engine | `crates/memstead-base/src/ops/mod.rs:1971` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1308`<br>`crates/memstead-cli/src/commands/schema.rs:733`<br>`crates/memstead-cli/src/commands/schema.rs:761`<br>`crates/memstead-cli/src/commands/schema.rs:786`<br>`crates/memstead-cli/src/commands/schema.rs:931`<br>`crates/memstead-cli/src/commands/schema.rs:943`<br>`crates/memstead-mcp/src/server.rs:1446` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1259`<br>`crates/memstead-mcp/src/server.rs:947` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1941` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1928` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1326`<br>`crates/memstead-mcp/src/filesystem_server.rs:1002`<br>`crates/memstead-mcp/src/server.rs:1688` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:249`<br>`crates/memstead-base/src/runtime_validator.rs:250`<br>`crates/memstead-base/src/section_format.rs:524` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:1959` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:522` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:244` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1962` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1294`<br>`crates/memstead-mcp/src/filesystem_server.rs:761`<br>`crates/memstead-mcp/src/server.rs:1244` |
| `SIGNAL_THRESHOLD_CROSSED` | engine | `crates/memstead-base/src/ops/mod.rs:1948` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2114` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1286`<br>`crates/memstead-mcp/src/server.rs:1316` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1905` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1288`<br>`crates/memstead-mcp/src/server.rs:1334` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1287`<br>`crates/memstead-mcp/src/server.rs:1325` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1943` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:293`<br>`crates/memstead-cli/src/lib.rs:39` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:1903` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1902` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1942` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:255` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1897` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1264`<br>`crates/memstead-cli/src/commands/type_cmd.rs:54`<br>`crates/memstead-mcp/src/filesystem_server.rs:331`<br>`crates/memstead-mcp/src/filesystem_server.rs:2068`<br>`crates/memstead-mcp/src/server.rs:983`<br>`crates/memstead-mcp/src/server.rs:2628` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1919` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1900` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1252`<br>`crates/memstead-cli/src/commands/changes.rs:229`<br>`crates/memstead-cli/src/commands/create.rs:351`<br>`crates/memstead-cli/src/commands/export.rs:174`<br>`crates/memstead-cli/src/commands/export.rs:324`<br>`crates/memstead-cli/src/commands/export.rs:536`<br>`crates/memstead-cli/src/commands/uninstall.rs:36`<br>`crates/memstead-mcp/src/filesystem_server.rs:2006`<br>`crates/memstead-mcp/src/filesystem_server.rs:2391`<br>`crates/memstead-mcp/src/server.rs:829`<br>`crates/memstead-mcp/src/server.rs:2396`<br>`crates/memstead-mcp/src/server.rs:2502`<br>`crates/memstead-mcp/src/server.rs:3600` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:241` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1935` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1254`<br>`crates/memstead-mcp/src/server.rs:856` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1255`<br>`crates/memstead-mcp/src/server.rs:899` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:240` |
| `UNSUPPORTED_PARAM` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:289` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:861` |
| `UNTERMINATED_FENCE` | engine | `crates/memstead-base/src/runtime_validator.rs:248` |
| `UNTERMINATED_FENCE_IN_STORED_BODY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1300`<br>`crates/memstead-mcp/src/filesystem_server.rs:316`<br>`crates/memstead-mcp/src/server.rs:791` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1904` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1285`<br>`crates/memstead-mcp/src/server.rs:1528` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:50` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:633` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:539` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI | `crates/memstead-base/src/engine/error.rs:2257`<br>`crates/memstead-base/src/workspace_store.rs:157`<br>`crates/memstead-cli/src/commands/changes.rs:250`<br>`crates/memstead-cli/src/commands/publish.rs:498`<br>`crates/memstead-cli/src/setup.rs:41` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:160` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:158` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:159` |
