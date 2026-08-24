#!/usr/bin/env sh
# memstead unified installer: wraps cargo-dist's per-crate installers so
# `curl -sSf <url> | sh` lands both `memstead` and `memstead-mcp` in one call.
#
# One project, three origins: memstead.ai serves the live graph,
# memstead.io hosts this installer and the registry, and the
# source of both binaries is github.com/memstead/memstead — this script
# only downloads release artifacts from that repository.
#
# cargo-dist publishes two installers per release:
#
#   * `memstead-cli-installer.sh` — installs the `memstead` binary
#   * `memstead-mcp-installer.sh` — installs the `memstead-mcp` binary
#
# This wrapper fetches and runs both in sequence, delivering the single
# `curl -sSf <url> | sh` install path the docs advertise.
#
# The served copy lives at `https://memstead.io/install.sh`; this file is
# its source.
#
# Usage:
#
#   curl -sSf https://memstead.io/install.sh | sh
#   curl -sSf https://memstead.io/install.sh | sh -s -- --version v0.10.0
#   MEMSTEAD_VERSION=v0.10.0 sh -c 'curl -sSf https://memstead.io/install.sh | sh'
#
# Defaults: latest tag, ~/.cargo/bin install dir (cargo-dist's default).
# `--version <tag>` (or `--version=<tag>`, or the MEMSTEAD_VERSION
# variable) picks the release; this script consumes it, because the
# cargo-dist child installers do not know the flag and refuse it. Every
# other flag is forwarded to both child installers; consult
# `memstead-cli-installer.sh --help` for the full list.
set -eu

REPO="${MEMSTEAD_REPO:-memstead/memstead}"
RELEASE="${MEMSTEAD_VERSION:-latest}"

# Consume `--version <tag>` / `--version=<tag>`; keep everything else
# for the child installers (rebuilt positionally, POSIX sh has no arrays).
forward=""
while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            if [ $# -lt 2 ]; then
                echo "--version needs a tag, e.g. --version v0.10.0" >&2
                exit 1
            fi
            RELEASE="$2"
            shift 2
            ;;
        --version=*)
            RELEASE="${1#--version=}"
            shift
            ;;
        *)
            forward="$forward $1"
            shift
            ;;
    esac
done
# shellcheck disable=SC2086
set -- $forward

# Resolve "latest" to the actual tag once so both child installers
# pull from the same release. Avoids a race window where a new release
# lands between the two fetches.
if [ "$RELEASE" = "latest" ]; then
    api="https://api.github.com/repos/${REPO}/releases/latest"
    RELEASE=$(curl -sSf "$api" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
    if [ -z "$RELEASE" ]; then
        echo "could not resolve latest release from $api" >&2
        exit 1
    fi
fi

base="https://github.com/${REPO}/releases/download/${RELEASE}"

echo "==> memstead unified installer (${RELEASE})"

for component in memstead-cli memstead-mcp; do
    url="${base}/${component}-installer.sh"
    echo "==> running ${component}-installer.sh"
    # Forward all positional args (e.g. `--quiet`, `--target-dir`) to
    # each child installer. The child scripts are cargo-dist-generated
    # and accept the same flag set.
    if ! curl -sSfL "$url" | sh -s -- "$@"; then
        echo "${component} install failed" >&2
        exit 1
    fi
done

echo ""
echo "==> memstead installed (${RELEASE})"
echo "    Run 'memstead --version' to verify."
echo ""
echo "    Next: 'memstead quickstart' bootstraps a workspace here —"
echo "          or 'memstead quickstart --repo .' inside a repository you"
echo "          already have, which also binds it as a source."
echo ""
echo "    Claude Code users can add the plugin for a guided setup:"
echo "      claude plugin marketplace add memstead/memstead"
echo "      claude plugin install memstead@memstead"
echo "    then restart Claude Code and run '/setup'."
