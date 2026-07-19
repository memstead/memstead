---
type: spec
created_date: 2026-07-15T07:29:23Z
last_modified: 2026-07-15T07:34:37Z
level: M0
stability: evolving
tags: stdlib, collections
---

# Collections

## Identity
Kāra's built-in collection types — Vec[T], VecDeque[T], Map[K,V], Set[T], SortedSet[T] — with their literal syntax, methods, Display, hashing, and drop behavior across interpreter and codegen.

## Purpose
To provide the everyday container vocabulary with full-fidelity semantics in both execution backends.

## Relationships
- **REFERENCES**: [[standard-library]]
- **REFERENCES**: [[monomorphized-collection-codegen]]
- **PART_OF**: [[standard-library]]
- **INFORMED_BY**: [[fxhash-for-hash-codegen]]

## Realization

- runtime/stdlib/vec.kara, vec_deque.kara, map.kara, set.kara, sorted_set.kara; runtime/src/map.rs (KaracMap)
- Codegen: src/codegen/maps.rs, vec_method.rs, collections.rs, entry_chains.rs
- Interpreter: src/interpreter/method_call_map.rs, method_call_set.rs, method_call_seq.rs

## Specifies

- Vec[T]: new/push/pop(→Option[T])/filled(n,val)/from_slice/sort_by/sort_by_key, indexed read+write, method dispatch on indexed receivers.
- VecDeque[T]: push/pop_*/iter/len/is_empty (lowered onto Vec's {ptr,len,cap}).
- Map[K,V]: m[k] read/write, keys/values/entries, clear, entry(k) chains (or_insert / or_insert_with / and_modify), Map[k: v] prefix-literal form.
- Set[T]: union/intersection/difference; SortedSet.
- Per-type synthesized Display, hash, eq, and recursive drop for keys/elements.

## Constraints

- Map method catch-all is hardened from silent-0 to Err.
- Element/key types must supply hash+eq for Map/Set use.

## Rationale

Core [[standard-library]] surface. Hot paths get [[monomorphized-collection-codegen]].
