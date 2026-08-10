# obligation@0.1.0 — the document-and-deadline pattern

The first non-software built-in: a small, domain-neutral vocabulary
for holdings whose subject is authoritative documents plus deadlines —
contracts and renewal windows, compliance and filing duties, permits,
grants, maintenance cycles.

Four types, kept deliberately a vocabulary rather than a census:

- **obligation** — a dated duty with a stated consequence if missed.
  The defining property: it *forfeits* (a right, a fee, a standing),
  where a milestone merely *slips*. Declares the `due:` axis, so
  `memstead due` answers "what is due next" over any mem pinned here.
- **party** — any counterparty: person, organization, authority.
- **commitment** — the modelled agreement obligations arise from,
  distinct from the artifact file that evidences it.
- **decision** — the why, with its rejected alternatives and date.

Recurring duties are one entity whose `due_date` the maintaining
agent advances after each completed occurrence — the engine never
advances a date; the agent loop is the runtime.

**This is a fork target.** Nothing in it names a domain or
jurisdiction. When your domain needs its own surrounding types
(assets, cases, correspondence, units), fork the package
(`memstead schema install obligation@0.1.0`, copy, rename, extend)
and keep this core.
