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
#
# Resolution follows the release host's redirect
# (github.com/<repo>/releases/latest -> .../releases/tag/<tag>), which is
# not subject to GitHub's anonymous REST quota. The REST API is
# deliberately contacted nowhere in this script: a shared address (an
# office, a campus, a CI runner) that has spent the 60-per-hour anonymous
# budget must still be able to install.
if [ "$RELEASE" = "latest" ]; then
    latest_url="https://github.com/${REPO}/releases/latest"
    if ! final_url=$(curl -sSfLI -o /dev/null -w '%{url_effective}' "$latest_url"); then
        echo "could not resolve the latest release from $latest_url" >&2
        echo "likely cause: no network, or the repository has no published release." >&2
        echo "workaround: pin a release yourself with --version <tag> or MEMSTEAD_VERSION=<tag>;" >&2
        echo "tags are listed at https://github.com/${REPO}/releases" >&2
        exit 1
    fi
    RELEASE="${final_url##*/tag/}"
    if [ -z "$RELEASE" ] || [ "$RELEASE" = "$final_url" ]; then
        echo "could not resolve the latest release from $latest_url (no tag redirect; landed on: $final_url)" >&2
        echo "workaround: pin a release yourself with --version <tag> or MEMSTEAD_VERSION=<tag>;" >&2
        echo "tags are listed at https://github.com/${REPO}/releases" >&2
        exit 1
    fi
fi

base="https://github.com/${REPO}/releases/download/${RELEASE}"

echo "==> memstead unified installer (${RELEASE})"

for component in memstead-cli memstead-mcp; do
    url="${base}/${component}-installer.sh"
    echo "==> running ${component}-installer.sh"
    # Download to a file first, then run it: piping curl into sh lets a
    # failed fetch feed sh an empty script that exits 0, and the wrapper
    # would then report success over an install that never happened.
    child=$(mktemp "${TMPDIR:-/tmp}/${component}-installer.XXXXXX")
    if ! curl -sSfL "$url" -o "$child"; then
        rm -f "$child"
        echo "${component} installer download failed from ${url}" >&2
        exit 1
    fi
    # Forward all positional args (e.g. `--quiet`, `--target-dir`) to
    # each child installer. The child scripts are cargo-dist-generated
    # and accept the same flag set.
    if ! sh "$child" "$@"; then
        rm -f "$child"
        echo "${component} install failed" >&2
        exit 1
    fi
    rm -f "$child"
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
echo "    A session that is already running picks the new skills up only after '/reload-plugins' or a restart."
echo "    Then run '/setup'."
