---
title: "Error Code Index"
---

# Error Code Index

Every typed error code emitted by the engine, the CLI (`memstead-cli`), and the MCP server (`memstead-mcp`). Each row lists the code, the surfaces that emit it, and the source locations the static scan found.

**Distinct codes:** 117

| Code | Surfaces | Source locations |
|------|----------|------------------|
| `AMBIGUOUS_DESCRIPTION_DELIMITER` | engine | `crates/memstead-base/src/ops/mod.rs:1450` |
| `ARCHIVE_ASSEMBLY_FAILED` | CLI | `crates/memstead-cli/src/commands/export.rs:287` |
| `ARCHIVE_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/publish.rs:278` |
| `ARCHIVE_VALIDATION_FAILED` | CLI | `crates/memstead-cli/src/lib.rs:54` |
| `AUTO_STUB_CREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1442` |
| `BATCH_REFUSED` | CLI | `crates/memstead-cli/src/commands/batch_update.rs:292` |
| `CHANGELOG_ERROR` | MCP | `crates/memstead-mcp/src/filesystem_server.rs:1544` |
| `CHUNK_OUT_OF_RANGE` | CLI | `crates/memstead-cli/src/commands/context.rs:44`<br>`crates/memstead-cli/src/commands/entity.rs:74`<br>`crates/memstead-cli/src/commands/overview.rs:143`<br>`crates/memstead-cli/src/commands/overview.rs:231`<br>`crates/memstead-cli/src/lib.rs:42` |
| `CONFLICTING_SECTION_MODES` | engine | `crates/memstead-base/src/engine/error.rs:1015` |
| `CROSS_MEM_EDGE_NOT_DECLARED` | engine | `crates/memstead-base/src/engine/error.rs:999` |
| `CROSS_MEM_LINK_NOT_ALLOWED` | engine | `crates/memstead-base/src/engine/error.rs:997` |
| `CROSS_MEM_TARGET_MEM_UNCREATED` | engine | `crates/memstead-base/src/ops/mod.rs:1392` |
| `CROSS_MEM_TARGET_NOT_FOUND` | engine | `crates/memstead-base/src/engine/error.rs:998` |
| `DESCRIPTION_NOT_PERMITTED` | engine | `crates/memstead-base/src/engine/error.rs:1035`<br>`crates/memstead-base/src/ops/mod.rs:1452` |
| `DOMAIN_KEYGEN_FAILED` | CLI | `crates/memstead-cli/src/commands/domain.rs:73` |
| `DOMAIN_KEY_NOT_FOUND` | CLI | `crates/memstead-cli/src/commands/domain.rs:81`<br>`crates/memstead-cli/src/commands/publish.rs:296` |
| `DUPLICATE_MEM` | engine | `crates/memstead-base/src/engine/error.rs:980` |
| `DUPLICATE_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1396` |
| `DUPLICATE_SECTION_HEADING` | engine | `crates/memstead-base/src/ops/mod.rs:1439` |
| `EMPTY_UPDATE` | engine | `crates/memstead-base/src/engine/error.rs:1002` |
| `ENTITY_ALREADY_EXISTS` | engine, MCP | `crates/memstead-base/src/engine/error.rs:992`<br>`crates/memstead-mcp/src/filesystem_server.rs:326` |
| `ENTITY_NOT_FOUND` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:993`<br>`crates/memstead-cli/src/commands/relations.rs:71`<br>`crates/memstead-mcp/src/filesystem_server.rs:329`<br>`crates/memstead-mcp/src/filesystem_server.rs:946` |
| `FIELD_NOT_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1418` |
| `FIELD_NOT_RANGE_FILTERABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1431` |
| `FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1415` |
| `FILTER_VALUE_MULTI_MEMBER` | engine | `crates/memstead-base/src/ops/mod.rs:1419` |
| `HASH_FLAG_REQUIRED` | CLI | `crates/memstead-cli/src/lib.rs:33` |
| `HASH_MISMATCH` | engine | `crates/memstead-base/src/engine/error.rs:994` |
| `HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:995` |
| `IGNORED_READONLY_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1436` |
| `INLINE_WIKI_LINK_AUTO_STUBBED` | engine | `crates/memstead-base/src/ops/mod.rs:1391` |
| `INTERNAL` | CLI | `crates/memstead-cli/src/lib.rs:28` |
| `INVALID_CURSOR` | engine | `crates/memstead-base/src/engine/error.rs:1032` |
| `INVALID_ENTITY_ID` | engine | `crates/memstead-base/src/engine/error.rs:1012` |
| `INVALID_ENUM_VALUE` | engine | `crates/memstead-base/src/ops/mod.rs:1420`<br>`crates/memstead-base/src/runtime_validator.rs:197` |
| `INVALID_FIELD_VALUE` | engine | `crates/memstead-base/src/runtime_validator.rs:204` |
| `INVALID_INPUT` | engine, CLI, MCP | `crates/memstead-base/src/engine/error.rs:1030`<br>`crates/memstead-base/src/engine/error.rs:1031`<br>`crates/memstead-cli/src/commands/batch_update.rs:462`<br>`crates/memstead-mcp/src/filesystem_server.rs:1633`<br>`crates/memstead-mcp/src/server.rs:380`<br>`crates/memstead-mcp/src/server.rs:1900`<br>`crates/memstead-mcp/src/server.rs:2108` |
| `INVALID_MEM_NAME` | engine | `crates/memstead-base/src/engine/error.rs:1014` |
| `INVALID_REL_SHAPE` | engine | `crates/memstead-base/src/runtime_validator.rs:201` |
| `INVALID_REL_TYPE` | engine | `crates/memstead-base/src/runtime_validator.rs:200` |
| `INVALID_TITLE` | engine | `crates/memstead-base/src/engine/error.rs:991` |
| `INVALID_WIKI_LINK_TARGET` | engine | `crates/memstead-base/src/engine/error.rs:1013` |
| `LIMIT_CLAMPED` | engine | `crates/memstead-base/src/ops/mod.rs:1399` |
| `LOCAL_DIVERGENCE` | engine | `crates/memstead-base/src/engine/error.rs:984` |
| `LOCAL_INVALID_STATE` | engine | `crates/memstead-base/src/engine/error.rs:986` |
| `MARKDOWN_EXPORT_UNSUPPORTED_BACKEND` | engine | `crates/memstead-base/src/engine/error.rs:1041` |
| `MEM_CONFIG_INCOMPLETE` | engine | `crates/memstead-base/src/engine/error.rs:1033` |
| `MEM_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1025`<br>`crates/memstead-base/src/engine/error.rs:1028`<br>`crates/memstead-mcp/src/filesystem_server.rs:772` |
| `MEM_FILES_NOT_DELETED` | engine | `crates/memstead-base/src/ops/mod.rs:1448` |
| `MEM_HAS_INCOMING_REFS` | engine | `crates/memstead-base/src/engine/error.rs:996` |
| `MEM_NAME_COLLISION` | engine | `crates/memstead-base/src/engine/error.rs:1029` |
| `MEM_REATTACHED_AFTER_UNREGISTER` | engine | `crates/memstead-base/src/ops/mod.rs:1449` |
| `MEM_RELOADED` | engine | `crates/memstead-base/src/ops/mod.rs:1440` |
| `MISSING_REQUIRED_DESCRIPTION` | engine | `crates/memstead-base/src/engine/error.rs:1034`<br>`crates/memstead-base/src/ops/mod.rs:1451` |
| `MISSING_REQUIRED_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1394` |
| `MISSING_REQUIRED_OUTGOING` | engine | `crates/memstead-base/src/ops/mod.rs:1438` |
| `MISSING_REQUIRED_SECTION` | engine | `crates/memstead-base/src/engine/error.rs:1019`<br>`crates/memstead-base/src/ops/mod.rs:1393` |
| `NEIGHBOURHOOD_CAPPED` | engine | `crates/memstead-base/src/ops/mod.rs:1421` |
| `NETWORK_ERROR` | CLI | `crates/memstead-cli/src/commands/admin.rs:175` |
| `NON_FAST_FORWARD` | engine | `crates/memstead-base/src/engine/error.rs:985` |
| `NOTE_MISSING` | engine | `crates/memstead-base/src/ops/mod.rs:1435` |
| `NO_SUCH_RELATIONSHIP` | engine | `crates/memstead-base/src/ops/mod.rs:1397` |
| `OUTER_REPO_NOT_IGNORING_MEM_REPO` | engine | `crates/memstead-base/src/ops/mod.rs:1437` |
| `PARSED_RELATION_INVALID` | engine | `crates/memstead-base/src/ops/mod.rs:1444` |
| `PARSE_ERROR` | engine, MCP | `crates/memstead-base/src/engine/error.rs:1023`<br>`crates/memstead-base/src/engine/error.rs:1024`<br>`crates/memstead-mcp/src/filesystem_server.rs:774`<br>`crates/memstead-mcp/src/filesystem_server.rs:776` |
| `PATCH_OLD_NOT_FOUND` | engine | `crates/memstead-base/src/engine/error.rs:1021` |
| `PATCH_SECTION_EMPTY` | engine | `crates/memstead-base/src/engine/error.rs:1020` |
| `PUSHED_COMMITS_PROTECTED` | engine | `crates/memstead-base/src/engine/error.rs:988` |
| `RANGE_FILTER_KEY_MALFORMED` | engine | `crates/memstead-base/src/ops/mod.rs:1423` |
| `RANGE_FILTER_TYPE_SCOPED` | engine | `crates/memstead-base/src/ops/mod.rs:1428` |
| `READ_MEM_SHADOWS_WRITABLE` | CLI | `crates/memstead-cli/src/commands/install.rs:442` |
| `READ_ONLY_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:198` |
| `READ_ONLY_MOUNT` | engine | `crates/memstead-base/src/engine/error.rs:989` |
| `RELATIONSHIP_CYCLE` | engine | `crates/memstead-base/src/engine/error.rs:1016` |
| `RELATION_HAS_BODY_LINKS` | engine | `crates/memstead-base/src/engine/error.rs:1007` |
| `RELATION_MANUAL_AUTHORING_FORBIDDEN` | engine | `crates/memstead-base/src/engine/error.rs:1037` |
| `RENAME_BLOCKED_BY_CROSS_MEM_POLICY` | engine | `crates/memstead-base/src/engine/error.rs:1004` |
| `RENAME_NO_OP` | engine | `crates/memstead-base/src/engine/error.rs:1001` |
| `RENAME_PARTIAL_FAILURE` | engine | `crates/memstead-base/src/engine/error.rs:1006` |
| `REPAIR_NOT_NEEDED` | engine | `crates/memstead-base/src/engine/error.rs:1000` |
| `REQUIRED_FIELD_UNSET` | engine | `crates/memstead-base/src/engine/error.rs:1018` |
| `RESIDUAL_STUB_FOR_READONLY_REFERRERS` | engine | `crates/memstead-base/src/ops/mod.rs:1446` |
| `SCHEMA_NOT_FOUND` | engine | `crates/memstead-base/src/engine/error.rs:1026` |
| `SCHEMA_PIN_MISMATCH` | engine | `crates/memstead-base/src/ops/mod.rs:1441` |
| `SCHEMA_RESOLVER_INIT_FAILED` | engine | `crates/memstead-base/src/engine/error.rs:1027` |
| `SCHEMA_VIOLATION_IN_FETCH` | engine | `crates/memstead-base/src/engine/error.rs:987` |
| `SEARCH_MEM_INDEX_UNAVAILABLE` | engine | `crates/memstead-base/src/ops/mod.rs:1432` |
| `SEARCH_RESULTS_TRUNCATED` | engine | `crates/memstead-base/src/ops/mod.rs:1422` |
| `SEARCH_UNAVAILABLE_IN_WASM` | engine | `crates/memstead-base/src/engine/error.rs:1039` |
| `SECTION_CONTENT_INVALID` | engine | `crates/memstead-base/src/runtime_validator.rs:202`<br>`crates/memstead-base/src/runtime_validator.rs:203` |
| `SECTION_NOT_UPDATABLE` | engine | `crates/memstead-base/src/runtime_validator.rs:199` |
| `SELF_LINK_IGNORED` | engine | `crates/memstead-base/src/ops/mod.rs:1443` |
| `SET_AND_UNSET_CONFLICT` | engine | `crates/memstead-base/src/engine/error.rs:1017` |
| `STUB_CANNOT_RELATE` | engine | `crates/memstead-base/src/engine/error.rs:1009` |
| `STUB_FILTER_EXCLUDES_ALL` | engine | `crates/memstead-base/src/ops/mod.rs:1402` |
| `STUB_NOT_RENAMABLE` | engine | `crates/memstead-base/src/engine/error.rs:1011` |
| `STUB_NOT_UPDATABLE` | engine | `crates/memstead-base/src/engine/error.rs:1010` |
| `SUSPICIOUS_NESTED_PREFIX` | engine | `crates/memstead-base/src/ops/mod.rs:1434` |
| `TARGET_NOT_EMPTY` | CLI | `crates/memstead-cli/src/lib.rs:38` |
| `TITLE_NORMALIZED_TO_SLUG_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1400` |
| `TITLE_TRIMMED` | engine | `crates/memstead-base/src/ops/mod.rs:1433` |
| `UNDECLARED_RELATIONSHIP_OPEN` | engine | `crates/memstead-base/src/ops/mod.rs:1395` |
| `UNKNOWN_ENTITY_TYPE` | engine | `crates/memstead-base/src/engine/error.rs:990` |
| `UNKNOWN_FILTER_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1413` |
| `UNKNOWN_INCLUDE_KEY` | engine | `crates/memstead-base/src/ops/mod.rs:1398` |
| `UNKNOWN_MEM` | engine | `crates/memstead-base/src/engine/error.rs:981` |
| `UNKNOWN_METADATA_FIELD` | engine | `crates/memstead-base/src/runtime_validator.rs:196` |
| `UNKNOWN_RANGE_FILTER_FIELD` | engine | `crates/memstead-base/src/ops/mod.rs:1426` |
| `UNKNOWN_REF` | engine | `crates/memstead-base/src/engine/error.rs:982` |
| `UNKNOWN_REMOTE` | engine | `crates/memstead-base/src/engine/error.rs:983` |
| `UNKNOWN_SECTION` | engine | `crates/memstead-base/src/runtime_validator.rs:195` |
| `UPDATE_NOOP` | engine | `crates/memstead-base/src/ops/mod.rs:1401` |
| `WIKILINK_WITHOUT_RELATION` | engine | `crates/memstead-base/src/engine/error.rs:1008` |
| `WORKSPACE_ALREADY_EXISTS_ABOVE` | CLI | `crates/memstead-cli/src/lib.rs:49` |
| `WORKSPACE_CONFIG_INVALID` | CLI | `crates/memstead-cli/src/commands/install.rs:273`<br>`crates/memstead-cli/src/commands/install.rs:280`<br>`crates/memstead-cli/src/commands/install.rs:322`<br>`crates/memstead-cli/src/commands/install.rs:329` |
| `WORKSPACE_CONFIG_READ_FAILED` | CLI | `crates/memstead-cli/src/commands/install.rs:270` |
| `WORKSPACE_NOT_INITIALISED` | CLI | `crates/memstead-cli/src/setup.rs:40` |
