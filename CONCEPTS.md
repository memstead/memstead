# Core concepts

The fourteen terms Memstead's docs, tool descriptions, and error
messages assume. Each definition below is quoted verbatim from the
normative [glossary](GLOSSARY.md) — follow a term's link for its
boundaries, rationale, and worked details. Read top to bottom once
(~3 minutes) and every other page in this repo gets easier.

## [Mem](GLOSSARY.md#mem)

> A named, schema-pinned markdown entity graph about exactly one chosen subject — a **typed model of that subject**.

## [Entity](GLOSSARY.md#entity)

> An atomic, addressable element in a mem — a single markdown document conforming to one type from the mem's pinned schema.

## [Schema](GLOSSARY.md#schema)

> The type vocabulary that constrains a mem's content — what entity types exist, what sections each type has, which sections are required, what relationship types are allowed, what metadata fields are valid.

## [Subject](GLOSSARY.md#subject)

> The topical focus a [mem](#mem) is *about* — what makes one mem distinct from another that pins the same [schema](#schema).

## [Modal flavour](GLOSSARY.md#modal-flavour)

> The conceptual genre a mem inhabits — knowledge, planning, inquiry, spec, or hybrid — determined by the [schema](#schema) the mem pins.

## [Workspace](GLOSSARY.md#workspace)

> A named runtime context that lists a set of mem mounts and the policy that governs them collectively.

## [Mount](GLOSSARY.md#mount)

> The act of making one mem — together with its schema and capabilities — available to a running engine. Also the resulting record in the engine's mount registry.

## [Storage backend](GLOSSARY.md#storage-backend)

> The mechanism that holds one mem's bytes — folder of files, branch of a git repository, or `.mem` archive.

## [Graph](GLOSSARY.md#graph)

> The live, mutable form of typed models in a workspace, at any compositional level.

## [Wikilink](GLOSSARY.md#wikilink)

> A markdown reference to another [entity](#entity), of the form `[[id]]` or `[[mem:id]]`.

## [Cross-mem edge](GLOSSARY.md#cross-mem-edge)

> A relationship between an entity in one mounted mem and an entity in another mounted mem of the same workspace.

## [Workspace store](GLOSSARY.md#workspace-store)

> The persisted form of a workspace's configuration — a logical data structure containing the mount list, the cross-mem permission table, the workspace-level policy, and the workspace-level pipeline configuration. How it reaches durable storage is the responsibility of a replaceable persistence adapter.

## [Provenance / Mutation log](GLOSSARY.md#provenance--mutation-log)

> An append-only structured record of every mutation an entity in a mem undergoes — who, when, what, and optionally why.

## [Pipeline (medium · facet · projection)](GLOSSARY.md#pipeline-medium--facet--projection)

> The workspace-level mechanism that populates a mem's content from external bodies of information rather than from direct agent writes.
