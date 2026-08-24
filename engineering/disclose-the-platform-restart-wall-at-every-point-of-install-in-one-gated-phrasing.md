---
type: decision
created_date: 2026-08-24T19:37:54Z
last_modified: 2026-08-24T21:05:53Z
status: accepted
decided_on: 2026-08-24
deciders: dasboe
scope: system
tags: install, onboarding, claims, plugin, mcp, disclosure
---

# Disclose the platform restart wall at every point of install, in one gated phrasing

## Decision
We will state the agent-platform restart wall wherever a surface teaches an install, in one shared phrasing duplicated verbatim rather than paraphrased per surface, and we will build no reload mechanism of our own. The phrasing is two sentences because the platform is asymmetric: plugin skills reach a running Claude Code session after `/reload-plugins` or a restart, while an MCP server added during a session is not attached until the session restarts. The public copies are machine-gated by `scripts/check-restart-disclosure.sh` in `run-tests.sh`, which fails the suite when an install-teaching surface loses the sentence, so the phrasing changes in the guard first and everywhere second. Surfaces additionally name the before-launch path for a session that cannot restart at all: wire it before the agent starts (`quickstart` writes `.mcp.json` first; the platform's own `--mcp-config` and `--plugin-dir` load both at startup).

## Context
The 2026-08-22 sealed newcomer gate installed the plugin, saw six skills confirmed, ran the documented next step and got `Unknown skill: memstead:setup` (finding F7, `Blocked: yes`). Nothing was broken. The wall is platform-owned, but no surface that taught the install said so, and the silence was ours: [[plugin--setup-skill]] disclosed the restart for the MCP server it wires, while the marketplace entry, the READMEs, the install script and the flagship guide taught install-then-`/setup` with no step in between. Verified against the platform's own documentation on 2026-08-24: `/reload-plugins` exists and activates a plugin installed mid-session; a newly added MCP server is not picked up by a running session (`/mcp reconnect` only revives a server the session already knows); headless launches pre-load both via `--mcp-config` / `--plugin-dir`, project `.mcp.json`, and `enabledPlugins` in settings. That asymmetry is why one sentence would have been false in one direction or the other.

## Consequences
- A newcomer following any documented install path is told the restart requirement at the point of install, in the same words each time.
- The claim is machine-held for the open tree: an install-teaching surface that drops the sentence fails the suite, not a future review.
- The guard is half discovery, half list, and the split is where its honesty lives. Discovery sweeps every tracked reader-facing file in the open repository for quoted install commands and demands the applicable sentence, so a NEW surface that quotes a command is covered the day it is written. That half exists because a purely hand-kept list lost three grading rounds in a row, twice on a published crate readme. But a surface can teach an install without quoting anything (a catalogue description, a page generator, a doc comment), and no pattern reaches that shape, so those are held BY NAME and the list is load-bearing rather than a supplement: it holds four of the five hardest surfaces. Grades proved both halves by mutation, destroying the sentence in a scratch copy and checking the guard fails. Files that legitimately teach no install are exemptions carrying their reason, reviewed in the guard rather than forgotten elsewhere.
- Residual, known and named: teaching-without-a-quoted-command in a file nobody added to the list stays invisible to the machine. That is the class a human still owns.
- The guard still reaches only the public repository. The private copies (the hosted-endpoint HTML and runbook in `serve/`, memstead.com's brochure and `llms.txt`, the `.ai` site's connect dialog and agent-surface steps, the `.io` twin of `install.sh`, the launch-kit drafts and their CTA spec, the `flagship` mem's connect and bootstrap guides, the handbook chapter) carry the same sentences by hand and can drift; the handbook names them so the next editor knows where they are.
- The disclosure is scoped to the platform behaviour it describes, so if a reload path for MCP servers ships, this retires with a doc change rather than a rewrite. That is the cheap direction, and it is deliberate.
- Cost accepted: the sentences are long, they appear in a marketplace description and an installer's terminal output where every character is read, and each new install surface inherits an obligation.

## Relationships
- **MOTIVATED_BY**: [[a-surfaces-claim-about-itself-is-derived-or-absent]]
- **INFORMED_BY**: [[a-test-gate-that-exists-must-gate]]
- **REFERENCES**: [[plugin:setup-skill]]

## Options

- **Build a reload helper** (spawn a fresh authenticated session and proxy the skill call): rejected. The gate journey proved the container shape where it fails (no credentials reachable from a subprocess), and a sometimes-working workaround is a worse promise than an honest restart line.
- **Disclose in one place only** (the plugin README): rejected. The journey reached install through the marketplace without ever opening that README; the disclosure has to live where the install command is taught, which is several surfaces.
- **Treat it as the platform's bug and do nothing**: rejected. The wall is platform-owned, the silence about it was ours, and the silence is what the gate measured as `Blocked: yes`.
- **Let each surface phrase it in its own voice**: rejected. Five paraphrases drift, and drift is how a wall stays effectively undocumented while every surface looks covered. One phrasing, gated, chosen.

## Notes

Revisit when the platform ships an in-session attach path for MCP servers: at that point the second sentence becomes false, and the guard is the single place to change it first. The plan that landed this is flywheel 10-first-session-residue/02; the F6 sibling from the same gate run was the export-layout seam.
