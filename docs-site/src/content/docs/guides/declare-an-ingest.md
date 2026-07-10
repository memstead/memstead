---
title: Declare an ingest
description: "Hand-author the four-file declaration that tells memstead ingest what source to read and where to write it — the current pre-v1 format, with a worked codebase example."
sidebar:
  order: 5
---

:::caution[Pre-v1 format]
This is the **current** declaration format. It will be replaced by a single
versioned *binding* file before 1.0. When that lands, a provided command
(`memstead projection migrate`) will convert existing declarations for you — so
declarations you write today keep working, they just get folded into the new
shape automatically. Nothing here is a stable contract yet.
:::

The `/ingest` skill grows a mem from a body of source material — a codebase, a
folder of documents, another mem. Before it can run, you declare **what it
reads and where it writes**. Today that declaration is four small JSON files in
the workspace store. This guide walks through writing them for a codebase, then
running the ingest.

The four primitives — *medium*, *facet*, *projection*, *ingest* — have precise
meanings defined in the [Glossary](../../glossary/#pipeline-medium--facet--projection--ingest);
that page is the normative reference. This guide is operational: where the files
go, what each field is for, and a minimal example you can copy.

## Where the files live

All four declarations live under the `.memstead/` workspace store, next to your
mem:

```
.memstead/
├── mediums/<mem>/<name>.json       ← what body of information exists
├── facets/<mem>/<name>.json        ← which slice of it, engaged how
├── projections/<mem>/<name>.json   ← what feeds which destination mem
└── ingests/<name>.json             ← how and when to run it
```

`mediums`, `facets`, and `projections` are grouped by the mem that owns them (a
`<mem>/` subdirectory); `ingests` is a flat folder. In every case the file
**stem is the declaration's name** — `mediums/myapp/code.json` declares a medium
named `code`, and the `name` field inside must match the stem.

## The four primitives, one at a time

A **medium** ([glossary](../../glossary/#pipeline-medium--facet--projection--ingest))
names a body of information the mem treats as its territory — a codebase, a
filesystem, another mem. It is passive: it only points at what's out there, it
does not filter or transform.

A **facet** names a specific slice of a medium and how a projection engages with
it — an allow/deny scope over paths, and (for non-text sources) an optional
preparation step. One medium can back several facets.

A **projection** is the obligation: it maps one or more facets into a
**destination mem**, with an `intent` string that tells the ingest agent how to
read the source and what to write. It may also list `reference_mems` — other
mems the agent may read for context but must not write.

An **ingest** is the operational layer: which projection to run, in what `mode`,
on what `trigger`, and in what `batch_size`. It is the thing you name when you
run `/ingest`.

## Worked example: a codebase → a mem

Say you have a mem named `myapp` and a Rust codebase at `../myapp-src` you want
to model. Four files:

**`.memstead/mediums/myapp/code.json`** — point at the codebase:

```json
{
  "name": "code",
  "type": "codebase",
  "pointer": "../myapp-src"
}
```

**`.memstead/facets/myapp/code.json`** — read the Rust sources and manifests,
skip build output:

```json
{
  "name": "code",
  "medium": "code",
  "scope": [
    { "path": "../myapp-src/**/*.rs", "mode": "allow" },
    { "path": "../myapp-src/**/Cargo.toml", "mode": "allow" },
    { "path": "../myapp-src/target/**", "mode": "deny" }
  ]
}
```

The facet's `medium` field references the medium by its name (`code`).

**`.memstead/projections/myapp/graph.json`** — map the facet into the `myapp`
mem, and tell the agent how to read:

```json
{
  "intent": "Rust application source. Read the code as the source of truth; treat docstrings as claims to verify. Model modules, their responsibilities, and the relationships between them.",
  "source_facets": ["code"],
  "reference_mems": [],
  "destination_mem": "myapp"
}
```

`source_facets` lists facet names; `destination_mem` is the mem this projection
grows. Leave `reference_mems` empty unless the agent needs to read another mem
for context.

**`.memstead/ingests/myapp-code.json`** — how and when to run it:

```json
{
  "projection": "myapp/graph",
  "mode": "discovery",
  "trigger": "manual",
  "batch_size": 20,
  "deny_paths": []
}
```

`projection` references the projection as `<mem>/<name>` — here the
`projections/myapp/graph.json` file, so `myapp/graph`. `mode` is `discovery` for
a first full pass; `batch_size` bounds how much source one run works through;
`deny_paths` is an extra exclusion list layered on top of the facet's scope.

## Run it

The ingest name is the `ingests/` file stem — here `myapp-code`. Render its
run-brief (the prompt the ingest agent consumes) and confirm the declaration
resolves:

```bash
memstead ingest brief myapp-code
```

If the four files are consistent, this prints the Markdown brief for the run. In
a Claude Code session, the `/ingest myapp-code` skill renders the same brief and
carries out the pass, writing entities into the `myapp` mem one batch at a time.

:::note[Non-text sources need preparation]
A facet over a PDF, DOCX, or audio medium declares a *preparation* step
(document→markdown, audio→transcript). No preparation implementation ships
today, so an ingest whose facet declares one is reported as unsupported rather
than run. Codebase and filesystem mediums are text and need no preparation.
:::

## Next

- [Glossary — Pipeline](../../glossary/#pipeline-medium--facet--projection--ingest)
  for the normative definitions of every term used here.
- [Getting started](../getting-started/) if you don't yet have a mem to point an
  ingest at.
