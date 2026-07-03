# `agent-program@0.1.0` — knowledge-graph schema for executable agent programs

A copy-paste-ready Memstead schema for **executable agent programs** —
graphs an LLM agent reads as instructions and runs over autonomously,
with declared tools and constraints. Four types model the executable
surface:

| Concern | Type |
|---|---|
| What runs, when, what counts as done | `program` |
| Each unit of work the agent satisfies | `node` |
| A callable capability the program may use | `tool` |
| A rule the agent must obey | `constraint` |

The agent itself is the runtime. There is no external orchestrator
walking nodes and prompting the agent at each step. The graph is
**content the agent reads as instructions** — not control flow imposed
on the agent. The graph constrains the action space; the agent picks
words and tool calls within it.

This directory is a **pure example**. It is not wired into any mem,
not registered in `.memstead/workspace.toml`, not published. To adopt it,
copy the directory into a program mem and pin it.

## How to use

### Per program (or program family), a mem

Create a dedicated mem for each agent program (or for a family of
related programs):

```
<workspace>/programs/<program-name>/
```

1. Install this example package into the workspace's local schema
   storage, then create the program mem pinned to it:

   ```
   memstead schema install examples/schemas/agent-program
   memstead mem init <workspace>/programs/<program-name> --schema agent-program@0.1.0
   ```

   `install` copies the package under `<workspace>/.memstead/schemas/agent-program@0.1.0/`;
   the next engine boot resolves the pin.

2. Author. Start with a `program` entity. Author its `tool` catalog
   (one tool per MCP tool / shell command the program calls). Author
   `constraint` entities for the rules. Then build the node graph
   from the entry node outward, linking branches_to and falls_back_to
   as you go.

### Anatomy of a program

```
                  program  ─entry─→  node (kind=action)
                     │                  │ branches_to
                     │ governed_by      ↓
                     ↓               node (kind=decision)
                  constraint            │ branches_to / falls_back_to
                                        ↓                        ↓
                                      node                     node
                                     (kind=action)            (kind=terminal,
                                        │ uses                  outcome=escalated)
                                        ↓
                                      tool
```

The agent enters at the `entry` node, satisfies its intent, checks
success, and follows a branches_to edge. On error it follows a
falls_back_to edge instead. Loops are allowed (refinement, retry,
polling) — branches_to is not acyclic. Fallback chains terminate —
falls_back_to is acyclic. The program ends at a terminal node.

## Types

### Root — what runs

| Type | Purpose |
|---|---|
| `program` | The runnable unit. Declares purpose, entry signature, exit criteria, and operating mode (autonomous / interactive / loop). |

### Execution — the work

| Type | Purpose | Key test |
|---|---|---|
| `node` | One unit of work the agent satisfies before transitioning. | Does it have intent + success_check + transitions, and is its kind consistent with its outgoing edges? |

### Catalog — capabilities

| Type | Purpose | Key test |
|---|---|---|
| `tool` | A callable capability with declared inputs, outputs, error codes. | Does its `tool_name` match the actual MCP tool / shell command the agent calls? |
| `constraint` | A rule the agent must obey. | Is enforcement (advisory or blocking) and detection both stated? |

## Relationship vocabulary

Strict mode. Five program-specific edges plus the relevant defaults:

| Edge | From → To | Purpose |
|---|---|---|
| `entry` | program → node | Single starting node per program. |
| `branches_to` | node → node | Possible next steps. Loops allowed. The `transitions` section in the source node's body explains when to take which branch. |
| `falls_back_to` | node → node | Error / escalation path. Acyclic — fallback chains terminate. |
| `uses` | node → tool | The agent may call the tool at this node. Declarative; not prescriptive. |
| `governed_by` | program → constraint, node → constraint | The rule applies. Program-attached = whole program; node-attached = local. |

Common defaults in use: `PART_OF` (node belongs to program; tools and
constraints belong to a catalog mem), `MOTIVATED_BY` (program →
planning.decision in another mem), `IMPLEMENTS` (program → spec in a
software mem), `SUPERSEDES` (a v2 program replaces a v1).

## Why this graph runs

Three properties hold by construction once a program validates:

1. **Bounded action space.** A node can only transition to nodes it
   has edges to. The agent doesn't fabricate transitions — they are
   declared. The intent and tool calls remain free; the *next state*
   is enumerated.

2. **Typed error handling.** Tools declare the error codes they emit.
   Nodes attach falls_back_to edges keyed against those codes via
   their `error_handling` section. Unknown errors propagate to the
   program's `failure_modes` section and the closest falls_back_to.

3. **Auditable rule application.** Constraints attach via governed_by.
   At any node the agent can ask "what rules apply here?" and receive
   the union of program-wide and node-local constraints. After
   execution, an auditor can query "where did rule X apply?" and get
   the answer from the graph.

## Evolving the schema

Bump `version` in `schema.yaml` on any shape change. Programs pin
exact versions (`agent-program@0.1.0`), so an active program keeps
working against the pinned version until explicitly updated. Ship new
versions alongside old ones.

Likely v0.2 expansions (not in v0.1):

- `prescribed_tool_call` section on `node` for fully-deterministic
  steps (tool name + literal params + output bindings).
- `condition` metadata on `branches_to` edges (requires engine
  support for edge metadata).
- Sub-program composition: a node whose action is "run sub-program X
  and bind its outcome to my success_check".
- `goal` type if exit criteria need to be queryable as standalone
  entities.

These are deliberately deferred. The v0.1 surface aims to be the
smallest schema that lets a useful program run.

## Reference

- Built-in schema this one extends: [crates/memstead-schema/builtins/schemas/default/](../../../crates/memstead-schema/builtins/schemas/default/)
- Sister built-in schemas: [planning](../../../crates/memstead-schema/builtins/schemas/planning/) (deliberation), [software](../../../crates/memstead-schema/builtins/schemas/software/) (code-bound knowledge)
