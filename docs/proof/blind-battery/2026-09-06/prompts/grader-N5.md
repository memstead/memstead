You are grading two anonymous answers, A and B, to one question about Memstead. You do not know who wrote them and must not guess; ignore any stray hint of where an answer came from and judge content only.

Verify every checkable claim in both answers against the engine's Rust source under `<source>/crates` and, where a claim is about behaviour, by running the release binary `<memstead>` in a scratch workspace of your own under `<run>/grader-N5` (create it with `<memstead> init --name probe --schema default@1.3.0 <run>/grader-N5/probe` or `<memstead> quickstart <run>/grader-N5/probe`; never touch any other workspace or repository).

Return ONLY a JSON object with exactly these keys:
- "winner": "A", "B" or "tie"
- "margin": 1, 2 or 3 (1 close, 3 decisive; 0 for a tie)
- "scores": {"A": {"correctness": 0-5, "completeness": 0-5, "usefulness": 0-5, "honesty": 0-5}, "B": {same}}
- "errors_A": a list of strings, each one false or unverifiable claim in A with what the source or the binary shows instead; empty if none
- "errors_B": the same for B
- "rationale": under 120 words

Correctness is truth against the source and the binary; completeness is whether the question's parts are all answered; usefulness is whether a developer could act on it; honesty is whether the answer hedges what it could not establish and invents nothing.

Question:

What are the hard limits on a published Memstead mem archive (size, entries, entity count, file types, anything else), and why were they set that way? How big a mem is the system actually designed for?

# Answer A

The hard limits sit in one place: the ingress validator every `.mem` archive passes before any byte reaches a cache, a running engine, or the registry. Its default limits are 2 MiB compressed and 20 MiB uncompressed for the whole archive, 1 MiB per entry, 64 KiB for `.memstead/config.json`, at most 10,000 files, a 512-character path length, and a directory depth of 16. Custom limits exist for non-default callers, but the registry, the CLI install path and export all use the same function, so in practice the defaults are what a publisher meets. The registry answers an oversized upload with 413, which the CLI maps to `ARCHIVE_TOO_LARGE`, and the upload client's 30-second timeout is sized for that 2 MB cap.

File types are a whitelist, not a size question: entity markdown (`.md`) at mem-relative paths, `.memstead/config.json`, the pinned schema's source under `.memstead/schema/`, and optional `provenance.json` and `anchors.json` sidecars. Symlinks, duplicate paths, non-UTF-8 content and zip-slip paths are refused; entity ids must be unique after Unicode normalization; an unterminated code fence in any entity refuses the whole archive; and cross-mem edges refuse at install unless exported `--self-contained`.

There is no explicit entity-count cap beyond the 10,000-file ceiling. The stated reason for the limits is trust-boundary defence: an archive is the unit the registry distributes and installs, and a malformed, oversized or id-colliding one would corrupt a graph silently, so rejection happens once, in one pure function, for every surface.

Design size: a mem is meant for one coherent subject of roughly 1,000 to 5,000 entities. Beyond about 10,000 the intended model is several small mems federated by cross-mem edges; the engine does not hard-cap open mems, but community detection, schema validation at boot and search-index build all degrade non-linearly past that range. I could not verify whether the 2 MiB compressed cap comfortably holds a 5,000-entity mem; that depends on entity size and is not stated anywhere I can confirm.

# Answer B

A published mem is a zip archive, and one validator enforces a fixed ladder of caps on every archive that enters a cache, the registry, or a byte-hydrated engine. The defaults are: 2 MiB compressed archive, 20 MiB total uncompressed, 1 MiB per uncompressed entry, 64 KiB for `.memstead/config.json`, 10,000 zip entries, 512-character paths, and 16 path segments deep. The registry answers an oversized upload with HTTP 413, which the CLI reports as "archive exceeds the 2 MB publisher cap". The caps are a struct, so a self-hosted or enterprise registry can construct different values; the defaults are described as matching the public registry.

There is no entity-count limit as such. Entities are bounded only indirectly by the 10,000-entry and 20 MiB ceilings; entity IDs must additionally be unique after NFC normalization.

File types are a whitelist: `.md` entity files outside `.memstead/`, `.memstead/config.json`, the schema package under `.memstead/schema/` (`schema.yaml`, the format marker, and `types/<name>.yaml`), `.memstead/provenance.json`, and `.memstead/anchors.json`. Any other file is rejected, as are symlinks, absolute or drive-letter paths, `..` traversal, duplicate paths, non-UTF-8 markdown, and a `.md` file inside `.memstead/`. Unknown non-markdown files at the top of `.memstead/` are tolerated and dropped for forward compatibility. The `format` field must be one of the accepted generations.

The stated rationale is security rather than capacity: every read stops at cap+1 bytes so decompression is never sized by an attacker-declared header, and tests explicitly exercise deflate bombs. I found no recorded justification for the specific numbers.

As for intended size, the code repeatedly assumes a mem of up to roughly 10k entities: the search-hit ceiling per mem is 10,000 and named the "typical mem" perf budget, the index writer heap is sized for corpora up to ~10k entities, and the store notes a re-bucketing only if a workspace passes 10k. Practically, the 2 MiB compressed cap will bind first for prose-heavy mems, likely in the low thousands of entities.
