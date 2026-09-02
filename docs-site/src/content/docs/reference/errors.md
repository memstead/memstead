---
title: "Error Code Index"
---

# Error Code Index

Typed error codes the static scan finds in the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations. Not indexed here: the registry-relayed codes the CLI maps from memstead.io HTTP statuses during publish/install (`REGISTRY_VALIDATION_FAILED`, `NOT_AUTHENTICATED`, `FORBIDDEN`, `REGISTRY_NOT_FOUND`, `GONE`, `ARCHIVE_TOO_LARGE`, `RATE_LIMITED`, `REGISTRY_ERROR` — see the publish guide and `memstead-cli/src/commands/publish.rs`).

**Distinct codes:** 242

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:2058` |
| `AMBIGUOUS_MEM` | CLI | `crates/memstead-cli/src/commands/export.rs:706`<br>`crates/memstead-cli/src/commands/publish.rs:242`<br>`crates/memstead-cli/src/commands/type_cmd.rs:226` |
| `AMBIGUOUS_QUERY` | CLI | `crates/memstead-cli/src/commands/context.rs:67` |
| `ANCHORS_SIDECAR_UNREADABLE` | CLI | `crates/memstead-cli/src/commands/anchors.rs:67`<br>`crates/memstead-cli/src/commands/projection.rs:2261`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:154` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:267`<br>`crates/memstead-cli/src/commands/publish.rs:341` |
| `ARCHIVE_INVALID` | CLI | `crates/memstead-cli/src/commands/publish.rs:384` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:377`<br>`crates/memstead-cli/src/commands/publish.rs:636` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:665`<br>`crates/memstead-cli/src/lib.rs:55` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:2045` |
| `BATCH_REFUSED` | CLI, MCP | `crates/memstead-cli/src/commands/batch.rs:117`<br>`crates/memstead-cli/src/commands/check.rs:344`<br>`crates/memstead-mcp/src/filesystem_server.rs:1674`<br>`crates/memstead-mcp/src/server.rs:3241` |
| `BRANCH_RESET_HEAD_MOVED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1330`<br>`crates/memstead-mcp/src/server.rs:929` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:2224` |
| `CHECK_NOT_RECORDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1332`<br>`crates/memstead-mcp/src/server.rs:1029` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:194`<br>`crates/memstead-cli/src/commands/overview.rs:148`<br>`crates/memstead-cli/src/commands/overview.rs:234`<br>`crates/memstead-cli/src/lib.rs:43` |
| `CONFIG_ERROR` | MCP | `crates/memstead-mcp/src/server.rs:1920` |
| `CONFIG_WRITE_INTERVENED` | engine | `crates/memstead-base/src/ops/mod.rs:2037` |
| `CONFLICTING_SECTION_MODES` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1378`<br>`crates/memstead-mcp/src/filesystem_server.rs:755`<br>`crates/memstead-mcp/src/server.rs:1170` |
| `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND` | engine | `crates/memstead-base/src/engine/error.rs:1400` |
| `CONSTRAINT_UNSATISFIED` | engine | `crates/memstead-base/src/engine/error.rs:1336`<br>`crates/memstead-base/src/engine/outcomes.rs:643`<br>`crates/memstead-base/src/ops/mod.rs:2033` |
| `CONTEXT_NOT_COMPUTABLE` | CLI | `crates/memstead-cli/src/commands/context.rs:54` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:1345` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1343`<br>`crates/memstead-mcp/src/filesystem_server.rs:518` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1978` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1344`<br>`crates/memstead-mcp/src/filesystem_server.rs:527` |
| `CROSS_SCHEMA_LINK_UNDECLARED` | engine | `crates/memstead-base/src/ops/mod.rs:2048` |
| `DANGLING_LINK_NOT_RELATED` | engine | `crates/memstead-base/src/ops/mod.rs:3659` |
| `DANGLING_LINK_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3658` |
| `DANGLING_RELATION_TARGET_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:3660` |
| `DERIVATION_BASELINE_REFRESHED` | engine | `crates/memstead-base/src/ops/mod.rs:2046` |
| `DESCRIPTION_NOT_PERMITTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1409`<br>`crates/memstead-base/src/ops/mod.rs:2060`<br>`crates/memstead-mcp/src/filesystem_server.rs:962`<br>`crates/memstead-mcp/src/server.rs:1606` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:80`<br>`crates/memstead-cli/src/commands/publish.rs:400` |
| `DOMAIN_PUBLISH_UNAVAILABLE` | CLI | `crates/memstead-cli/src/commands/publish.rs:424` |
| `DUPLICATE_MEM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1319`<br>`crates/memstead-mcp/src/server.rs:1722` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1982` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:2034` |
| `EMBEDDED_SCHEMA_INVALID` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1393`<br>`crates/memstead-cli/src/commands/install.rs:273`<br>`crates/memstead-mcp/src/server.rs:1495` |
| `EMPTY_UNDECLARED_HEADING` | engine | `crates/memstead-base/src/runtime_validator.rs:247` |
| `EMPTY_UPDATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1365`<br>`crates/memstead-mcp/src/filesystem_server.rs:1065`<br>`crates/memstead-mcp/src/server.rs:1790` |
| `ENGINE_LOCK_POISONED` | MCP | `crates/memstead-mcp/src/error_envelopes.rs:70` |
| `ENGINE_VERSION_SKEW` | engine | `crates/memstead-base/src/ops/mod.rs:2041` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1335`<br>`crates/memstead-mcp/src/filesystem_server.rs:414`<br>`crates/memstead-mcp/src/server.rs:830` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1339`<br>`crates/memstead-cli/src/commands/context.rs:60`<br>`crates/memstead-cli/src/commands/delete.rs:55`<br>`crates/memstead-cli/src/commands/delete.rs:84`<br>`crates/memstead-cli/src/commands/delete.rs:127`<br>`crates/memstead-cli/src/commands/delete.rs:151`<br>`crates/memstead-cli/src/commands/entity.rs:60`<br>`crates/memstead-cli/src/commands/relations.rs:72`<br>`crates/memstead-cli/src/commands/rename.rs:142`<br>`crates/memstead-cli/src/commands/rename.rs:176`<br>`crates/memstead-cli/src/commands/retype.rs:228`<br>`crates/memstead-cli/src/commands/update.rs:828`<br>`crates/memstead-cli/src/commands/update.rs:852`<br>`crates/memstead-mcp/src/filesystem_server.rs:429`<br>`crates/memstead-mcp/src/filesystem_server.rs:1134`<br>`crates/memstead-mcp/src/filesystem_server.rs:2090`<br>`crates/memstead-mcp/src/server.rs:820`<br>`crates/memstead-mcp/src/server.rs:2003`<br>`crates/memstead-mcp/src/server.rs:2637` |
| `EXPECTED_HASH_REQUIRED` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1432`<br>`crates/memstead-mcp/src/server.rs:2984` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:2008` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:2024` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:2005` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:2009` |
| `FINDINGS_STORE_ERROR` | CLI | `crates/memstead-cli/src/commands/verify_anchors.rs:228` |
| `FOLDER_MEM_PROVENANCE` | engine | `crates/memstead-base/src/ops/mod.rs:2054` |
| `FOREIGN_MEMSTEAD_DIR` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:645` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:34` |
| `HASH_MISMATCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1340`<br>`crates/memstead-mcp/src/server.rs:862` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1341` |
| `HEALTH_STRICT_VIOLATIONS` | CLI | `crates/memstead-cli/src/commands/health.rs:1795` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:2029` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1977` |
| `INTERNAL` | CLI, MCP | `crates/memstead-cli/src/lib.rs:29`<br>`crates/memstead-mcp/src/filesystem_server.rs:1952`<br>`crates/memstead-mcp/src/filesystem_server.rs:2024`<br>`crates/memstead-mcp/src/filesystem_server.rs:2054` |
| `INTERNAL_IO_ERROR` | CLI | `crates/memstead-cli/src/commands/install.rs:121`<br>`crates/memstead-cli/src/commands/quickstart.rs:230`<br>`crates/memstead-cli/src/commands/quickstart.rs:370`<br>`crates/memstead-cli/src/commands/quickstart.rs:674`<br>`crates/memstead-cli/src/commands/quickstart.rs:857`<br>`crates/memstead-cli/src/commands/quickstart.rs:986`<br>`crates/memstead-cli/src/commands/quickstart.rs:1096`<br>`crates/memstead-cli/src/commands/quickstart.rs:1108`<br>`crates/memstead-cli/src/setup.rs:715` |
| `INVALID_ANCHOR` | engine | `crates/memstead-base/src/anchor.rs:73` |
| `INVALID_CHECK_FINDING` | engine | `crates/memstead-base/src/check.rs:65` |
| `INVALID_CHECK_KIND` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:109`<br>`crates/memstead-mcp/src/filesystem_server.rs:2444`<br>`crates/memstead-mcp/src/server.rs:3484` |
| `INVALID_CURSOR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1404`<br>`crates/memstead-base/src/engine/error.rs:1405`<br>`crates/memstead-mcp/src/filesystem_server.rs:1082`<br>`crates/memstead-mcp/src/filesystem_server.rs:2249`<br>`crates/memstead-mcp/src/server.rs:1805` |
| `INVALID_DOMAIN` | CLI | `crates/memstead-cli/src/commands/domain.rs:148` |
| `INVALID_ENTITY_ID` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1375`<br>`crates/memstead-mcp/src/filesystem_server.rs:706`<br>`crates/memstead-mcp/src/server.rs:361`<br>`crates/memstead-mcp/src/server.rs:376`<br>`crates/memstead-mcp/src/server.rs:1405` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:2010`<br>`crates/memstead-base/src/runtime_validator.rs:242` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:251` |
| `INVALID_IDENTITY` | CLI, MCP | `crates/memstead-cli/src/main.rs:131`<br>`crates/memstead-mcp/src/filesystem_server.rs:273`<br>`crates/memstead-mcp/src/server.rs:210` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1398`<br>`crates/memstead-base/src/engine/error.rs:1403`<br>`crates/memstead-cli/src/commands/admin.rs:78`<br>`crates/memstead-cli/src/commands/admin.rs:85`<br>`crates/memstead-cli/src/commands/admin.rs:123`<br>`crates/memstead-cli/src/commands/anchors.rs:39`<br>`crates/memstead-cli/src/commands/batch.rs:146`<br>`crates/memstead-cli/src/commands/batch.rs:153`<br>`crates/memstead-cli/src/commands/batch.rs:170`<br>`crates/memstead-cli/src/commands/batch.rs:187`<br>`crates/memstead-cli/src/commands/batch.rs:202`<br>`crates/memstead-cli/src/commands/batch_create.rs:110`<br>`crates/memstead-cli/src/commands/batch_create.rs:208`<br>`crates/memstead-cli/src/commands/batch_relate.rs:83`<br>`crates/memstead-cli/src/commands/batch_update.rs:262`<br>`crates/memstead-cli/src/commands/batch_update.rs:273`<br>`crates/memstead-cli/src/commands/batch_update.rs:292`<br>`crates/memstead-cli/src/commands/batch_update.rs:429`<br>`crates/memstead-cli/src/commands/check.rs:252`<br>`crates/memstead-cli/src/commands/check.rs:259`<br>`crates/memstead-cli/src/commands/check.rs:269`<br>`crates/memstead-cli/src/commands/conflicts.rs:100`<br>`crates/memstead-cli/src/commands/create.rs:168`<br>`crates/memstead-cli/src/commands/create.rs:175`<br>`crates/memstead-cli/src/commands/create.rs:191`<br>`crates/memstead-cli/src/commands/create.rs:198`<br>`crates/memstead-cli/src/commands/create.rs:238`<br>`crates/memstead-cli/src/commands/create.rs:376`<br>`crates/memstead-cli/src/commands/create.rs:460`<br>`crates/memstead-cli/src/commands/create.rs:483`<br>`crates/memstead-cli/src/commands/create.rs:498`<br>`crates/memstead-cli/src/commands/due.rs:39`<br>`crates/memstead-cli/src/commands/due.rs:48`<br>`crates/memstead-cli/src/commands/export.rs:175`<br>`crates/memstead-cli/src/commands/export.rs:184`<br>`crates/memstead-cli/src/commands/export.rs:194`<br>`crates/memstead-cli/src/commands/export.rs:203`<br>`crates/memstead-cli/src/commands/export.rs:234`<br>`crates/memstead-cli/src/commands/export.rs:266`<br>`crates/memstead-cli/src/commands/export.rs:280`<br>`crates/memstead-cli/src/commands/export.rs:954`<br>`crates/memstead-cli/src/commands/export.rs:959`<br>`crates/memstead-cli/src/commands/export.rs:996`<br>`crates/memstead-cli/src/commands/export.rs:1004`<br>`crates/memstead-cli/src/commands/install.rs:69`<br>`crates/memstead-cli/src/commands/mem.rs:1147`<br>`crates/memstead-cli/src/commands/mod.rs:113`<br>`crates/memstead-cli/src/commands/mod.rs:120`<br>`crates/memstead-cli/src/commands/projection.rs:1885`<br>`crates/memstead-cli/src/commands/publish.rs:128`<br>`crates/memstead-cli/src/commands/publish.rs:136`<br>`crates/memstead-cli/src/commands/publish.rs:158`<br>`crates/memstead-cli/src/commands/quickstart.rs:192`<br>`crates/memstead-cli/src/commands/quickstart.rs:212`<br>`crates/memstead-cli/src/commands/quickstart.rs:726`<br>`crates/memstead-cli/src/commands/quickstart.rs:750`<br>`crates/memstead-cli/src/commands/quickstart.rs:758`<br>`crates/memstead-cli/src/commands/quickstart.rs:829`<br>`crates/memstead-cli/src/commands/quickstart.rs:993`<br>`crates/memstead-cli/src/commands/quickstart.rs:1003`<br>`crates/memstead-cli/src/commands/quickstart.rs:1015`<br>`crates/memstead-cli/src/commands/quickstart.rs:1066`<br>`crates/memstead-cli/src/commands/relate.rs:85`<br>`crates/memstead-cli/src/commands/relate.rs:90`<br>`crates/memstead-cli/src/commands/retype.rs:90`<br>`crates/memstead-cli/src/commands/retype.rs:99`<br>`crates/memstead-cli/src/commands/schema.rs:192`<br>`crates/memstead-cli/src/commands/schema.rs:327`<br>`crates/memstead-cli/src/commands/schema.rs:365`<br>`crates/memstead-cli/src/commands/schema.rs:1299`<br>`crates/memstead-cli/src/commands/schema.rs:1331`<br>`crates/memstead-cli/src/commands/unpublish.rs:39`<br>`crates/memstead-cli/src/commands/update.rs:184`<br>`crates/memstead-cli/src/commands/update.rs:346`<br>`crates/memstead-cli/src/commands/update.rs:359`<br>`crates/memstead-cli/src/commands/update.rs:375`<br>`crates/memstead-cli/src/commands/update.rs:382`<br>`crates/memstead-cli/src/commands/update.rs:403`<br>`crates/memstead-cli/src/commands/update.rs:446`<br>`crates/memstead-cli/src/commands/update.rs:601`<br>`crates/memstead-cli/src/commands/update.rs:609`<br>`crates/memstead-cli/src/commands/update.rs:617`<br>`crates/memstead-cli/src/commands/update.rs:910`<br>`crates/memstead-cli/src/commands/update.rs:917`<br>`crates/memstead-cli/src/commands/update.rs:939`<br>`crates/memstead-cli/src/commands/update.rs:959`<br>`crates/memstead-cli/src/commands/update.rs:966`<br>`crates/memstead-cli/src/commands/update.rs:977`<br>`crates/memstead-cli/src/commands/workspace.rs:717`<br>`crates/memstead-cli/src/main.rs:94`<br>`crates/memstead-mcp/src/filesystem_server.rs:1013`<br>`crates/memstead-mcp/src/filesystem_server.rs:1547`<br>`crates/memstead-mcp/src/filesystem_server.rs:1879`<br>`crates/memstead-mcp/src/filesystem_server.rs:2070`<br>`crates/memstead-mcp/src/filesystem_server.rs:2105`<br>`crates/memstead-mcp/src/filesystem_server.rs:2372`<br>`crates/memstead-mcp/src/filesystem_server.rs:2561`<br>`crates/memstead-mcp/src/server.rs:412`<br>`crates/memstead-mcp/src/server.rs:465`<br>`crates/memstead-mcp/src/server.rs:1538`<br>`crates/memstead-mcp/src/server.rs:1561`<br>`crates/memstead-mcp/src/server.rs:2285`<br>`crates/memstead-mcp/src/server.rs:2469`<br>`crates/memstead-mcp/src/server.rs:2515`<br>`crates/memstead-mcp/src/server.rs:2552`<br>`crates/memstead-mcp/src/server.rs:2568`<br>`crates/memstead-mcp/src/server.rs:2681`<br>`crates/memstead-mcp/src/server.rs:3083`<br>`crates/memstead-mcp/src/server.rs:3676`<br>`crates/memstead-mcp/src/server.rs:3854`<br>`crates/memstead-mcp/src/server.rs:3997`<br>`crates/memstead-mcp/src/server.rs:4090`<br>`crates/memstead-mcp/src/server.rs:4147`<br>`crates/memstead-mcp/src/server.rs:4246`<br>`crates/memstead-mcp/src/server.rs:4285`<br>`crates/memstead-mcp/src/server.rs:4314` |
| `INVALID_MEM_NAME` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1377`<br>`crates/memstead-mcp/src/filesystem_server.rs:742`<br>`crates/memstead-mcp/src/server.rs:1439`<br>`crates/memstead-mcp/src/server.rs:1874` |
| `INVALID_OBSERVATION` | engine, CLI | `crates/memstead-base/src/anchor.rs:1151`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:89`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:96`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:108`<br>`crates/memstead-cli/src/commands/verify_anchors.rs:122` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/engine/outcomes.rs:641`<br>`crates/memstead-base/src/runtime_validator.rs:246` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:245` |
| `INVALID_ROLE` | CLI, MCP | `crates/memstead-cli/src/main.rs:107`<br>`crates/memstead-mcp/src/filesystem_server.rs:294`<br>`crates/memstead-mcp/src/server.rs:242` |
| `INVALID_TABLE_COLUMNS` | engine | `crates/memstead-base/src/section_format.rs:523` |
| `INVALID_TITLE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1334`<br>`crates/memstead-cli/src/commands/batch_create.rs:196`<br>`crates/memstead-cli/src/commands/create.rs:229`<br>`crates/memstead-mcp/src/filesystem_server.rs:408`<br>`crates/memstead-mcp/src/server.rs:1372` |
| `INVALID_VERDICT` | CLI, MCP | `crates/memstead-cli/src/commands/check.rs:154`<br>`crates/memstead-mcp/src/filesystem_server.rs:2427`<br>`crates/memstead-mcp/src/server.rs:3463` |
| `INVALID_VERSION` | CLI | `crates/memstead-cli/src/commands/publish.rs:144` |
| `INVALID_WIKI_LINK_TARGET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1376`<br>`crates/memstead-mcp/src/filesystem_server.rs:722`<br>`crates/memstead-mcp/src/server.rs:1420` |
| `IO_ERROR` | CLI | `crates/memstead-cli/src/commands/export.rs:913`<br>`crates/memstead-cli/src/commands/export.rs:1040`<br>`crates/memstead-cli/src/commands/schema.rs:401`<br>`crates/memstead-cli/src/commands/schema.rs:410`<br>`crates/memstead-cli/src/commands/schema.rs:435`<br>`crates/memstead-cli/src/commands/schema.rs:447`<br>`crates/memstead-cli/src/commands/schema.rs:1411`<br>`crates/memstead-cli/src/commands/schema.rs:1420` |
| `LEGACY_WORKSPACE_LAYOUT` | engine | `crates/memstead-base/src/workspace_store.rs:165` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1985` |
| `LOCAL_DIVERGENCE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1325`<br>`crates/memstead-mcp/src/server.rs:968` |
| `LOCAL_INVALID_STATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1327`<br>`crates/memstead-mcp/src/server.rs:990` |
| `LOGIN_FAILED` | CLI | `crates/memstead-cli/src/commands/login.rs:40`<br>`crates/memstead-cli/src/commands/publish.rs:558` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1415`<br>`crates/memstead-mcp/src/filesystem_server.rs:1056`<br>`crates/memstead-mcp/src/server.rs:1777` |
| `MEM_CONFIG_INCOMPLETE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1407`<br>`crates/memstead-mcp/src/filesystem_server.rs:1032`<br>`crates/memstead-mcp/src/server.rs:1577` |
| `MEM_ERROR` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1391`<br>`crates/memstead-base/src/engine/error.rs:1396`<br>`crates/memstead-cli/src/commands/workspace.rs:900`<br>`crates/memstead-cli/src/commands/workspace.rs:907`<br>`crates/memstead-mcp/src/filesystem_server.rs:925`<br>`crates/memstead-mcp/src/server.rs:1529`<br>`crates/memstead-mcp/src/server.rs:1752` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:2051` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:1342` |
| `MEM_NAME_COLLISION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1397`<br>`crates/memstead-mcp/src/server.rs:1478` |
| `MEM_NOT_READ_ONLY` | CLI | `crates/memstead-cli/src/commands/uninstall.rs:53` |
| `MEM_PATH_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1869` |
| `MEM_QUARANTINED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1322`<br>`crates/memstead-mcp/src/server.rs:903` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:2052` |
| `MEM_REFERENCED_BY_POLICY` | MCP | `crates/memstead-mcp/src/server.rs:1908` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:2035` |
| `MEM_REPO_NOT_SUPPORTED` | CLI | `crates/memstead-cli/src/commands/schema.rs:1175` |
| `MEM_ROSTER_CHANGED` | engine | `crates/memstead-base/src/ops/mod.rs:2036` |
| `MEM_SCHEMA_NOT_ALLOWED` | MCP | `crates/memstead-mcp/src/server.rs:1891` |
| `MEM_STORAGE_RESIDUE_DETECTED` | MCP | `crates/memstead-mcp/src/server.rs:1936` |
| `MEM_UNMOUNTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1321`<br>`crates/memstead-mcp/src/server.rs:890` |
| `MISSING_REQUIRED_DESCRIPTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1408`<br>`crates/memstead-base/src/ops/mod.rs:2059`<br>`crates/memstead-mcp/src/filesystem_server.rs:949`<br>`crates/memstead-mcp/src/server.rs:1623` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1980` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/engine/error.rs:1337`<br>`crates/memstead-base/src/engine/outcomes.rs:642`<br>`crates/memstead-base/src/ops/mod.rs:2031` |
| `MISSING_REQUIRED_SECTION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1382`<br>`crates/memstead-base/src/engine/outcomes.rs:636`<br>`crates/memstead-base/src/ops/mod.rs:1979`<br>`crates/memstead-mcp/src/filesystem_server.rs:896`<br>`crates/memstead-mcp/src/server.rs:1298` |
| `MOUNT_UNBACKED` | engine | `crates/memstead-base/src/ops/mod.rs:2040` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:2011` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:176`<br>`crates/memstead-cli/src/commands/publish.rs:641`<br>`crates/memstead-cli/src/commands/unpublish.rs:90` |
| `NON_FAST_FORWARD` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1326`<br>`crates/memstead-mcp/src/server.rs:977` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:2028` |
| `NOT_AUTHENTICATED` | CLI | `crates/memstead-cli/src/commands/admin.rs:161`<br>`crates/memstead-cli/src/commands/publish.rs:308`<br>`crates/memstead-cli/src/commands/unpublish.rs:53` |
| `NOT_CONFLICTED` | engine | `crates/memstead-base/src/engine/error.rs:1402` |
| `NO_ACTIVE_BINDING` | CLI | `crates/memstead-cli/src/commands/projection.rs:1826` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1983` |
| `NO_WORKSPACE` | CLI | `crates/memstead-cli/src/commands/schema.rs:1070` |
| `NO_WRITABLE_MEM` | CLI | `crates/memstead-cli/src/commands/batch_create.rs:183`<br>`crates/memstead-cli/src/commands/changes.rs:72`<br>`crates/memstead-cli/src/commands/create.rs:521`<br>`crates/memstead-cli/src/commands/export.rs:699` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:2030` |
| `OUT_OF_BAND_EDITS_UNDETECTED` | engine | `crates/memstead-base/src/ops/mod.rs:2038` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:2049` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1389`<br>`crates/memstead-base/src/engine/error.rs:1390`<br>`crates/memstead-mcp/src/filesystem_server.rs:927`<br>`crates/memstead-mcp/src/filesystem_server.rs:929`<br>`crates/memstead-mcp/src/server.rs:1734`<br>`crates/memstead-mcp/src/server.rs:1743` |
| `PATCH_OLD_NOT_FOUND` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1384`<br>`crates/memstead-mcp/src/filesystem_server.rs:910`<br>`crates/memstead-mcp/src/server.rs:1327` |
| `PATCH_SECTION_EMPTY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1383`<br>`crates/memstead-mcp/src/filesystem_server.rs:899`<br>`crates/memstead-mcp/src/server.rs:1313` |
| `PROJECTION_ADVANCE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:1907`<br>`crates/memstead-cli/src/commands/projection.rs:1952`<br>`crates/memstead-cli/src/commands/projection.rs:1987` |
| `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT` | CLI | `crates/memstead-cli/src/commands/projection.rs:1942` |
| `PROJECTION_BRIEF_BINDING_REQUIRED` | CLI | `crates/memstead-cli/src/commands/projection.rs:689` |
| `PROJECTION_BUILD_NOT_ENABLED` | CLI | `crates/memstead-cli/src/commands/projection.rs:629` |
| `PROJECTION_CAPABILITY_UNSUPPORTED` | CLI | `crates/memstead-cli/src/commands/projection.rs:592`<br>`crates/memstead-cli/src/commands/projection.rs:1621`<br>`crates/memstead-cli/src/commands/projection.rs:2312` |
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
| `PROJECTION_VERIFY_BACKFILL_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2461` |
| `PROJECTION_VERIFY_BASELINE_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2473` |
| `PROJECTION_VERIFY_FAILED` | CLI | `crates/memstead-cli/src/commands/projection.rs:2231`<br>`crates/memstead-cli/src/commands/projection.rs:2323` |
| `PROJECTION_VERIFY_FINDINGS` | CLI | `crates/memstead-cli/src/commands/projection.rs:2494` |
| `PROJECTION_VERIFY_INCONCLUSIVE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2521` |
| `PUSHED_COMMITS_PROTECTED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1329`<br>`crates/memstead-mcp/src/server.rs:946` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:2013` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:2021` |
| `READ_MEMS_MIGRATED_TO_MOUNTS` | engine | `crates/memstead-base/src/ops/mod.rs:2053` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:285` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:243` |
| `READ_ONLY_MOUNT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1331`<br>`crates/memstead-mcp/src/server.rs:1020` |
| `REGISTRY_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:186`<br>`crates/memstead-cli/src/commands/publish.rs:651`<br>`crates/memstead-cli/src/commands/unpublish.rs:100`<br>`crates/memstead-cli/src/registry/mod.rs:92` |
| `REGISTRY_MALFORMED_RESPONSE` | CLI | `crates/memstead-cli/src/commands/admin.rs:181`<br>`crates/memstead-cli/src/commands/publish.rs:646`<br>`crates/memstead-cli/src/commands/unpublish.rs:95` |
| `RELATIONSHIP_CYCLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1379`<br>`crates/memstead-mcp/src/filesystem_server.rs:793`<br>`crates/memstead-mcp/src/server.rs:1205` |
| `RELATION_HAS_BODY_LINKS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1370`<br>`crates/memstead-mcp/src/server.rs:1457` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1411`<br>`crates/memstead-mcp/src/filesystem_server.rs:976`<br>`crates/memstead-mcp/src/server.rs:1641` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1367`<br>`crates/memstead-mcp/src/server.rs:1692` |
| `RENAME_NO_OP` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1347`<br>`crates/memstead-mcp/src/filesystem_server.rs:569`<br>`crates/memstead-mcp/src/server.rs:1666` |
| `RENAME_PARTIAL_FAILURE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1369`<br>`crates/memstead-mcp/src/server.rs:1709` |
| `REPAIR_NOT_NEEDED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1346`<br>`crates/memstead-mcp/src/server.rs:1161` |
| `REQUIRED_FIELD_UNSET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1381`<br>`crates/memstead-base/src/engine/outcomes.rs:637`<br>`crates/memstead-mcp/src/filesystem_server.rs:838`<br>`crates/memstead-mcp/src/server.rs:1237` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:2050` |
| `RETYPE_NO_OP` | engine | `crates/memstead-base/src/engine/error.rs:1362` |
| `RETYPE_REFERRER_UNPROBEABLE` | engine | `crates/memstead-base/src/engine/error.rs:1364` |
| `RETYPE_REFUSED` | engine | `crates/memstead-base/src/engine/error.rs:1359` |
| `REVIEW_MARK_NOT_SET` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1406`<br>`crates/memstead-mcp/src/filesystem_server.rs:1088`<br>`crates/memstead-mcp/src/server.rs:1817` |
| `SCHEMA_AUTHORING_SOURCE_DIVERGED` | engine | `crates/memstead-base/src/ops/mod.rs:2056` |
| `SCHEMA_AUTHORING_SOURCE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:2055` |
| `SCHEMA_GENERATIONS_BEHIND` | engine | `crates/memstead-base/src/ops/mod.rs:2042` |
| `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` | engine | `crates/memstead-base/src/ops/mod.rs:2043` |
| `SCHEMA_MIGRATE_FAILED` | CLI | `crates/memstead-cli/src/commands/schema.rs:937` |
| `SCHEMA_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1392`<br>`crates/memstead-cli/src/commands/schema.rs:238`<br>`crates/memstead-cli/src/commands/schema.rs:340`<br>`crates/memstead-cli/src/commands/schema.rs:1272`<br>`crates/memstead-cli/src/commands/schema.rs:1306`<br>`crates/memstead-cli/src/commands/schema.rs:1322`<br>`crates/memstead-mcp/src/server.rs:1490` |
| `SCHEMA_PACKAGE_EXISTS` | CLI | `crates/memstead-cli/src/commands/schema.rs:385` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:2039` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1395`<br>`crates/memstead-mcp/src/server.rs:1520` |
| `SCHEMA_UNSTAMPED_SOURCE_ROT` | engine | `crates/memstead-base/src/ops/mod.rs:2057` |
| `SCHEMA_VALIDATION_FAILED` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1394`<br>`crates/memstead-cli/src/commands/schema.rs:858`<br>`crates/memstead-cli/src/commands/schema.rs:886`<br>`crates/memstead-cli/src/commands/schema.rs:911`<br>`crates/memstead-cli/src/commands/schema.rs:1209`<br>`crates/memstead-cli/src/commands/schema.rs:1221`<br>`crates/memstead-mcp/src/server.rs:1508` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1328`<br>`crates/memstead-mcp/src/server.rs:1007` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:2025` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:2012` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1413`<br>`crates/memstead-mcp/src/filesystem_server.rs:1042`<br>`crates/memstead-mcp/src/server.rs:1761` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:249`<br>`crates/memstead-base/src/runtime_validator.rs:250`<br>`crates/memstead-base/src/section_format.rs:524` |
| `SECTION_CONTENT_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:521` |
| `SECTION_HEADING_DIVERGENCE` | engine | `crates/memstead-base/src/ops/mod.rs:2044` |
| `SECTION_ITEM_PATTERN_MISMATCH` | engine | `crates/memstead-base/src/section_format.rs:522` |
| `SECTION_MAP_COLLISION` | engine | `crates/memstead-base/src/engine/outcomes.rs:640` |
| `SECTION_MAP_SOURCE_MISSING` | engine | `crates/memstead-base/src/engine/outcomes.rs:639` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:244` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:2047` |
| `SET_AND_UNSET_CONFLICT` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1380`<br>`crates/memstead-mcp/src/filesystem_server.rs:798`<br>`crates/memstead-mcp/src/server.rs:1304` |
| `SIGNAL_THRESHOLD_CROSSED` | engine | `crates/memstead-base/src/ops/mod.rs:2032` |
| `SOURCE_UNREACHABLE` | CLI | `crates/memstead-cli/src/commands/projection.rs:2291` |
| `STUB_CANNOT_RELATE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1372`<br>`crates/memstead-mcp/src/server.rs:1378` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1989` |
| `STUB_NOT_RENAMABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1374`<br>`crates/memstead-mcp/src/server.rs:1396` |
| `STUB_NOT_UPDATABLE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1373`<br>`crates/memstead-mcp/src/server.rs:1387` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:2027` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/commands/schema.rs:418`<br>`crates/memstead-cli/src/lib.rs:39` |
| `TITLE_CHARS_DROPPED_FROM_SLUG` | engine | `crates/memstead-base/src/ops/mod.rs:1987` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1986` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:2026` |
| `TOOL_DISABLED` | MCP | `crates/memstead-mcp/src/server.rs:306` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1981` |
| `UNKNOWN_BINDING_VERSION` | engine | `crates/memstead-base/src/workspace_store.rs:167` |
| `UNKNOWN_ENTITY_TYPE` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1333`<br>`crates/memstead-cli/src/commands/type_cmd.rs:79`<br>`crates/memstead-mcp/src/filesystem_server.rs:368`<br>`crates/memstead-mcp/src/filesystem_server.rs:2140`<br>`crates/memstead-mcp/src/server.rs:1043`<br>`crates/memstead-mcp/src/server.rs:2723` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:2003` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1984` |
| `UNKNOWN_MEM` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1320`<br>`crates/memstead-cli/src/commands/changes.rs:232`<br>`crates/memstead-cli/src/commands/create.rs:353`<br>`crates/memstead-cli/src/commands/export.rs:322`<br>`crates/memstead-cli/src/commands/export.rs:526`<br>`crates/memstead-cli/src/commands/export.rs:739`<br>`crates/memstead-cli/src/commands/uninstall.rs:41`<br>`crates/memstead-mcp/src/filesystem_server.rs:2078`<br>`crates/memstead-mcp/src/filesystem_server.rs:2573`<br>`crates/memstead-mcp/src/server.rs:880`<br>`crates/memstead-mcp/src/server.rs:2491`<br>`crates/memstead-mcp/src/server.rs:2597`<br>`crates/memstead-mcp/src/server.rs:3836` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:241` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:2019` |
| `UNKNOWN_REF` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1323`<br>`crates/memstead-mcp/src/server.rs:916` |
| `UNKNOWN_REMOTE` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1324`<br>`crates/memstead-mcp/src/server.rs:959` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/engine/outcomes.rs:635`<br>`crates/memstead-base/src/runtime_validator.rs:240` |
| `UNSUPPORTED_PARAM` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:320` |
| `UNSUPPORTED_WORKSPACE_SHAPE` | engine | `crates/memstead-base/src/workspace_store.rs:1023` |
| `UNTERMINATED_FENCE` | engine | `crates/memstead-base/src/runtime_validator.rs:248` |
| `UNTERMINATED_FENCE_IN_STORED_BODY` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1386`<br>`crates/memstead-mcp/src/filesystem_server.rs:353`<br>`crates/memstead-mcp/src/server.rs:842` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1988` |
| `WIKILINK_WITHOUT_RELATION` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1371`<br>`crates/memstead-mcp/src/server.rs:1590` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:50` |
| `WORKSPACE_ALREADY_INITIALISED` | CLI | `crates/memstead-cli/src/commands/quickstart.rs:633` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/workspace.rs:539` |
| `WORKSPACE_NOT_INITIALISED` | engine, CLI | `crates/memstead-base/src/engine/error.rs:2383`<br>`crates/memstead-base/src/workspace_store.rs:161`<br>`crates/memstead-cli/src/commands/changes.rs:253`<br>`crates/memstead-cli/src/commands/publish.rs:497`<br>`crates/memstead-cli/src/setup.rs:41` |
| `WORKSPACE_STORE_ERROR` | engine | `crates/memstead-base/src/workspace_store.rs:168` |
| `WORKSPACE_STORE_FORMAT_MISMATCH` | engine | `crates/memstead-base/src/workspace_store.rs:164` |
| `WORKSPACE_STORE_IO` | engine | `crates/memstead-base/src/workspace_store.rs:162` |
| `WORKSPACE_STORE_PARSE` | engine | `crates/memstead-base/src/workspace_store.rs:163` |
