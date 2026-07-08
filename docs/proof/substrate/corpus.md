# Mem-repo: mutation, trust, and coherence — engineering notes

These are working notes on how Memstead keeps a mem-repo correct while many
writers touch it. They are the *source corpus* for the substrate eval: both
substrates (free-form notes and schema-forced typed entities) are captured from
this same text, and neither is allowed to see it in a more favourable form than
the other.

## The engine is the only writer

Nothing outside the engine mutates a mem-repo. No raw `git` commands against it,
no hand-edited `.md` files under the mem's directories. Every create, update,
relate, rename, and delete routes through the engine — over MCP for agents, over
UniFFI for the native app, or over the CLI. Reads may use any surface; writes may
not.

The reason is not that a raw write is impossible — the files are plain markdown
on disk, and any process could edit them. The boundary runs by *discipline*, not
by capability. A raw write would bypass the two things the engine exists to
guarantee: schema validation at the moment of the write, and the provenance
record of why the write happened. Route around the engine and you route around
both.

## Optimistic content-hash locking

Many actors share one mem-repo at once: a long-lived engine process, out-of-band
CLI invocations, a human, several agents. To stop two of them from silently
clobbering the same entity, every update is guarded by an optimistic
content-hash check. The caller must present the expected hash of the entity's
current state. If the on-disk state has moved since the caller last read it, the
hashes disagree and the write is refused rather than applied. The loser re-reads
the new state and retries. No write is ever silently lost.

## Reload before operation

Content-hash locking protects a single write, but a long-lived engine process
also has to notice when a *sibling* process has changed the branch underneath it.
Memstead's coherence rule is reload-before-operation: a polling file-watcher
detects when the mem-repo's ref has moved, a `mem_changed` notice marks the
in-memory store stale, and the store reloads from the branch tip before it serves
the next operation. Without this, the engine would answer from a snapshot the
disk had already left behind. One known failure mode this guards against: a failed
writer check-and-set can leave the in-memory store ahead of committed disk, so
reads must reconcile against the branch, not trust memory.

## Never silently admit unvalidated content

The runtime validator gates every ingress into the graph. A write that violates
the schema — an unknown entity type, a required field left unset, a relationship
the vocabulary does not allow — is refused rather than coerced into something
that validates. Crucially, the refusal is not a bare error: it carries a typed
recovery payload naming what was wrong and how to fix it (the declared field
list, the allowed values, a nearest-match suggestion), so the agent that tripped
it can correct and resubmit without guessing.

## Provenance: the why-layer lives in git

Every mutation is a git commit. Beyond the auto-emitted `Tool:` / `Actor:` /
`Client:` trailer block, the workspace's `require_notes` policy nudges each
mutating commit to carry a one-sentence prose rationale — *why* this change, not
just what. A direct count over the live mem-repo put adherence near 97% of
mutating commits. The effect is that the project's decision history is readable
with `git log` alone, with no engine and no database, years after the fact.

## Portability by construction

Markdown plus git is the only authoritative store. Every index — the search
index, the in-memory graph — is a *derived projection* that can be rebuilt from
the markdown at any time. Nothing of value lives only in an index. The practical
consequence: to move a project's entire knowledge base to another machine you
copy the `.git` directory and the data goes with it; the indexes rebuild on first
load. There is no export step and no vendor format to escape.
