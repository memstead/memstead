---
type: principle
created_date: 2026-08-15T03:19:33Z
last_modified: 2026-08-15T03:19:37Z
authority: accepted
universality: domain-wide
tags: cli, output, shell-quoting, surfaces, instructions, onboarding
---

# A command a surface prints is built, never formatted

## Statement
When a surface prints a command for someone to run, that command is CONSTRUCTED by a builder that owns the three things every such command needs — a program resolved to something the reader can actually invoke, every interpolated value shell-quoted, and a working directory when the command only works somewhere else — and rendered from it. It is never assembled with string formatting at the call site.

The test that guards this does not enumerate the commands. It EXTRACTS every command the surface printed, from every output surface, and runs each one verbatim in a shell, from the directory the reader would be standing in, under the environment they invoked from.

## Scope
Every message that tells a reader or an agent to run something: command receipts, refusal recovery hints, disclosure blocks, `--help` prose that names a literal invocation, and the machine-readable twin of any of those. It covers the value being interpolated as much as the verb — a directory name, an installed binary path, an entity id.

It does NOT govern prose that merely mentions a command's name ("the `memstead` MCP server"), which is a noun rather than an instruction. The distinguishing question is whether a reader could reasonably copy the span and press enter.

## Relationships
- **REFERENCES**: [[advertised-front-door-commands-serve-a-fresh-non-maintainer-workspace]]
- **GOVERNS**: [[every-workspace-creating-command-discloses-the-shape-it-just-made]]

## Justification

String formatting puts the correctness of an instruction in the hands of whoever writes the next `format!`, and the failure is invisible to the author because the string looks right. Over four rounds of adversarial verification on one command's output, four separate printed commands were found unrunnable — an unquoted directory (`cd My Graph`), an unquoted binary path in a wiring command, a directory named `-graph` whose `cd` needed the `--` terminator, and a hardcoded bare `memstead` for a reader who has none on `PATH`. Each round fixed the instances it knew about and shipped the next one, because the shape of the code made "miss one" the default outcome.

The enumerating test is the second half of the same failure. Three successive versions of the guard passed while something printed was broken, because each sampled: one output surface, one agent target, one kind of awkward path, an environment that always had the binary on `PATH`. A guard that extracts and runs cannot be outrun by a command it was never told about.

This is the concrete form of [[engineering--advertised-front-door-commands-serve-a-fresh-non-maintainer-workspace]] one level down: it is not enough that the front door works: what the front door TELLS you to do next must work too, for the reader who is standing where they are, with the paths they actually have.

## Exceptions

Placeholder invocations that teach a shape rather than name a run (`memstead install <scope>/<name>`) are documentation, not instructions, and are exempt from the runnable test — the angle brackets are the marker.

A command may also be named while explicitly disowned: the lean build points at `memstead mem-repo init` while stating in the same sentence that this build does not carry it. That is honest, and the guard recognises the disowning.

## Consequences

A new printed command inherits the treatment by construction, and is covered by the extract-and-run guard the day it is added — neither requires the author to remember.

The program-resolution half has a visible cost: a reader whose binary is not on `PATH` sees absolute paths in their receipt rather than a tidy `memstead`. That is the correct trade — a tidy command that fails is worse than a long one that works — and the resolution collapses to the bare name whenever `PATH` genuinely carries this binary.

Where a surface prints a command it did NOT itself configure (quickstart leaving an existing `.mcp.json` entry untouched), the printed check must describe what is actually wired rather than what would have been — the same rule applied to the claim rather than the syntax.
