---
type: decision
created_date: 2026-09-03T12:47:32Z
last_modified: 2026-09-03T12:47:32Z
status: accepted
decided_on: 2026-09-03
deciders: backlog-repairs bundle, C1 executing session
scope: subsystem
tags: markdown, parser, fuzzing
---

# Close every open block context at a concatenation site, HTML blocks like fences

## Decision
We will treat every place the engine concatenates content pieces it did not parse together (the catch-all merge of non-schema sections, the generator's part assembly) as a context boundary that must be closed: after each piece the CommonMark referee is asked whether the running text ends inside an open fence or inside an HTML block of the kinds no blank line ends, and the matching closer is appended once. The closer is derived from the open construct's own start condition and verified by probe; no second model of CommonMark exists beside the parser.

## Context
The long-tier fuzzer found an input whose second parse-generate pass dropped two merged headings. The plan framed it as a control-byte finding; measurement showed the control bytes were incidental and the cause was an HTML block declaration left open at the end of one merged piece, hiding the fence of the next. The fence-only context close that earlier fuzz findings established was the right mechanism with too narrow a notion of context.

## Consequences
- The parse-generate fixpoint holds for the pinned artifact and the whole shared corpus.
- A stored section that ended inside an unclosed HTML block gains one closer line on its first re-save, the same one-round normalisation fence closers already perform.
- What the parser refuses is unchanged.
- Any future concatenation site in the engine must call the combined context oracle, not the fence oracle alone.

## Options

- Ask the referee for fences AND unterminated HTML blocks at every concatenation site: CHOSEN.
- Keep the fence-only oracle: rejected by the finding itself, which is an HTML block rather than a fence.
- Model the contexts independently of the parser: rejected; a second model of CommonMark is a second thing to be wrong.
