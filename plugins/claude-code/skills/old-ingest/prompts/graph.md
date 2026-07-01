## Destination: Knowledge Graph

- before creating anything, search the graph for existing coverage — not just exact duplicates but entities that already capture the same concepts under a different name. update the existing entity instead of creating a new one
- when reading the graph, look for repeated content across entities. if the same knowledge lives in multiple places, consolidate it into the right entity and remove it from the others — without losing information
- wire relationships as you go — entities without connections are orphans that nobody will find
- no cross-mem wiki-links or relationships — note cross-mem dependencies in Constraints as plain text
- the graph is a network, not a list. every entity should be reachable from related entities

## Entity Lifecycle

- before touching an entity: verify its source still exists. if the source was deleted, delete the entity. if it was renamed, rename the entity — never create a duplicate. if the entity is empty or a stub, delete it
- while updating: rewrite to match current reality. do not preserve claims the code no longer supports
- after updating: if the change was substantial, re-read the entity with `memstead_entity include_relations:true` and walk the incoming edges. if other entities reference what changed, verify their claims still hold. flag inconsistencies in Constraints
- when a source reveals a concern spanning beyond itself (data flow, auth chain, lifecycle continuing in other components), note it in Constraints as: cross-cutting: <name> — <which components are involved>

## Granularity

- one entity per concept, not per file — but each entity must have bounded, coherent scope
- if you're mapping 30+ source files to one entity, it's almost certainly too broad — split into sub-concepts
- a codebase with hundreds of source files needs dozens of entities, not a handful
- when unsure, err toward more focused entities — they can always be merged later, but oversized entities hide gaps
