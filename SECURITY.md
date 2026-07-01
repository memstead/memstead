# Security Policy

## Supported versions

Memstead is currently in **pre-release** development. Until a stable 1.0 release is tagged, only the latest commit on `main` is supported. Earlier commits, branches, and tagged pre-release builds do not receive security fixes.

| Version | Supported |
|---|---|
| `main` (HEAD) | yes |
| Tagged pre-release builds | no |
| Forks | by the fork maintainer |

## Reporting a vulnerability

If you believe you have found a security issue in Memstead — the engine, the CLI, the MCP server, the registry service, or the Memstead macOS application — please **do not open a public GitHub issue**.

Instead, send the report by email to:

**dasboe@me.com**

Subject line: `[Memstead security] <short description>`

Please include:

- A description of the issue and its potential impact
- Steps to reproduce, or a proof-of-concept if available
- Affected components (engine crate, CLI subcommand, MCP tool name, HTTP route, etc.)
- The Memstead commit SHA, version, or release tag against which the issue was observed
- Your preferred contact channel for follow-up
- Whether you would like to be credited in the resulting fix (optional)

## What you can expect

- **Acknowledgement** within 7 days of your initial email.
- **Triage and severity assessment** within 14 days.
- **Coordinated disclosure**: we will work with you to agree on a disclosure timeline, typically 30–90 days from triage depending on severity. Critical issues with active exploitation move faster; complex multi-component issues may take longer.
- **Credit** in the changelog and (if you wish) the commit message of the fix, unless you prefer to remain anonymous.

We do not currently operate a paid bug bounty program. Reports are appreciated and credited; monetary rewards are not available at this stage.

## Out of scope

The following are not considered security vulnerabilities under this policy:

- Issues in third-party dependencies that have not been triggered by Memstead's specific use. Report those upstream.
- Issues affecting only local development environments (e.g., running tests with elevated privileges).
- Theoretical attacks without a working proof-of-concept.
- Documentation issues, typos, or wording that could be misinterpreted but does not affect runtime behaviour.
- Performance degradation that is not denial-of-service in nature.

## Vulnerability disclosure principles

- We will not pursue legal action against good-faith security researchers who report issues responsibly through this channel.
- We will not publicly identify reporters without their consent.
- We will publish a brief post-mortem in the changelog or repository discussions for fixed issues at moderate or higher severity.
