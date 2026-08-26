// Fixture tests for scripts/release-verify.sh: the four exit codes, the
// channel-mismatch and skipped-publish failures, the prerelease allowance
// and the report-only tree-vs-tag line, with the network replaced by a
// fake `curl` and the run readout by a fake `gh` on PATH.
//
// Node built-ins only (node --test). The script under test is copied into
// a scratch tree so its `$ROOT/Cargo.toml` read (the tree-vs-tag line) can
// be fabricated per case.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, chmodSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPTS = dirname(fileURLToPath(import.meta.url));

// Every channel at `v`, the publish jobs as given (name\tconclusion lines).
function fixtures({ v, brewMcp = v, jobs }) {
  return {
    "latest.json": `{"tag_name": "v${v}"}`,
    "release-tag.json": `{"assets": [{"name": "memstead-cli-installer.sh"}, {"name": "memstead-mcp-installer.sh"}]}`,
    "memstead-cli.rb": `class MemsteadCli < Formula\n  version "${v}"\nend\n`,
    "memstead-mcp.rb": `class MemsteadMcp < Formula\n  version "${brewMcp}"\nend\n`,
    "plugin.json": `{"name": "memstead", "version": "${v}"}`,
    "marketplace.json": `{"version": "${v}"}`,
    "crates.json": `{"crate": {"max_version": "${v}"}}`,
    "npm.json": `{"dist-tags": {"latest": "${v}"}}`,
    "jobs.txt": jobs.join("\n") + "\n",
  };
}

// A scratch tree: scripts/release-verify.sh, Cargo.toml at `treeVersion`,
// and bin/{curl,gh} fakes that serve the fixture files.
function scratch({ treeVersion, fx }) {
  const dir = mkdtempSync(join(tmpdir(), "release-verify-"));
  mkdirSync(join(dir, "scripts"));
  mkdirSync(join(dir, "bin"));
  mkdirSync(join(dir, "fx"));
  cpSync(join(SCRIPTS, "release-verify.sh"), join(dir, "scripts/release-verify.sh"));
  chmodSync(join(dir, "scripts/release-verify.sh"), 0o755);
  writeFileSync(join(dir, "Cargo.toml"), `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${treeVersion}"\n`);
  for (const [name, body] of Object.entries(fx)) writeFileSync(join(dir, "fx", name), body);
  writeFileSync(join(dir, "bin/curl"), `#!/bin/bash
# fake curl: serve a fixture file by URL pattern, honoring -o/-D/-w the way
# the script's fetch helper uses them. FAKE_FAIL_URLS (space-separated URL
# substrings) makes matching URLs answer FAKE_FAIL_CODE (default 403) with
# FAKE_FAIL_BODY — an error body, not an absent response; when
# FAKE_RATELIMIT=1 the failure carries rate-limit headers and
# FAKE_RATELIMIT_RESET as the reset epoch.
FX="${join(dir, "fx")}"
url=""; out=""; dump=""; wfmt=""; prev=""
for a in "$@"; do
  case "$prev" in
    -o) out="$a"; prev=""; continue ;;
    -D) dump="$a"; prev=""; continue ;;
    -w) wfmt="$a"; prev=""; continue ;;
    -H) prev=""; continue ;;
  esac
  case "$a" in
    -o|-D|-w|-H) prev="$a" ;;
    http*) url="$a" ;;
  esac
done
emit() { # $1 body-file  $2 http-code  $3 extra-headers
  if [ -n "$dump" ]; then printf 'HTTP/2 %s\\r\\n%s\\r\\n' "$2" "$3" > "$dump"; fi
  if [ -n "$out" ]; then cat "$1" > "$out"; else cat "$1"; fi
  if [ -n "$wfmt" ]; then printf '%s' "$2"; fi
  exit 0
}
for bad in \${FAKE_FAIL_URLS:-}; do
  case "$url" in *"$bad"*)
    body=$(mktemp); printf '%s' "\${FAKE_FAIL_BODY:-upstream error}" > "$body"
    hdrs=""
    if [ "\${FAKE_RATELIMIT:-}" = 1 ]; then
      hdrs=$(printf 'x-ratelimit-remaining: 0\\r\\nx-ratelimit-reset: %s\\r\\n' "\${FAKE_RATELIMIT_RESET:-1756200000}")
    fi
    emit "$body" "\${FAKE_FAIL_CODE:-403}" "$hdrs" ;;
  esac
done
case "$url" in
  https://api.github.com) emit /dev/null 200 "" ;;
  */releases/latest) emit "$FX/latest.json" 200 "" ;;
  */releases/tags/*) emit "$FX/release-tag.json" 200 "" ;;
  */Formula/memstead-cli.rb) emit "$FX/memstead-cli.rb" 200 "" ;;
  */Formula/memstead-mcp.rb) emit "$FX/memstead-mcp.rb" 200 "" ;;
  */plugin.json) emit "$FX/plugin.json" 200 "" ;;
  */marketplace.json) emit "$FX/marketplace.json" 200 "" ;;
  *crates.io*) emit "$FX/crates.json" 200 "" ;;
  *registry.npmjs.org*) emit "$FX/npm.json" 200 "" ;;
  *) echo "fake curl: unmapped $url" >&2; exit 22 ;;
esac
`);
  writeFileSync(join(dir, "bin/gh"), `#!/bin/bash
# fake gh: one release run (4242) whose jobs are the fixture
FX="${join(dir, "fx")}"
case "$1 $2" in
  "run list") echo 4242 ;;
  "api repos/"*) cat "$FX/jobs.txt" ;;
  *) echo "fake gh: unsupported $*" >&2; exit 9 ;;
esac
`);
  chmodSync(join(dir, "bin/curl"), 0o755);
  chmodSync(join(dir, "bin/gh"), 0o755);
  return dir;
}

function run(dir, args, extraEnv = {}) {
  const r = spawnSync(join(dir, "scripts/release-verify.sh"), args, {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, PATH: `${join(dir, "bin")}:${process.env.PATH}`, ...extraEnv },
  });
  return { status: r.status, out: r.stdout + r.stderr };
}

const ALL_SUCCESS = [
  "plan\tsuccess",
  "publish-homebrew-formula\tsuccess",
  "custom-publish-crates / publish-crates\tsuccess",
  "custom-publish-npm / publish-npm\tsuccess",
  "announce\tsuccess",
  "custom-release-verify / verify\tin_progress",
];

test("exit 0: every channel and every publish job agree with the tag", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["v0.10.0"]);
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /every channel serves 0\.10\.0$/m);
  assert.match(r.out, /publish job custom-publish-npm \/ publish-npm\s+.*success/);
  assert.doesNotMatch(r.out, /REPORT:/);
});

test("exit 1: a channel disagrees with the tag", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", brewMcp: "0.9.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["0.10.0"]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /Homebrew memstead-mcp\s+.*0\.9\.0 \(expected 0\.10\.0\)/);
  assert.match(r.out, /1 channel\(s\) or publish job\(s\) disagree/);
});

test("exit 1: a publish job skipped on a non-prerelease", () => {
  const jobs = ALL_SUCCESS.map((j) => (j.startsWith("custom-publish-npm") ? "custom-publish-npm / publish-npm\tskipped" : j));
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs }) });
  const r = run(dir, ["v0.10.0", "--run-id", "4242"]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /publish job custom-publish-npm \/ publish-npm\s+.*SKIPPED on a non-prerelease/);
});

test("exit 0: a prerelease may skip its publish jobs", () => {
  const v = "0.11.0-prerelease.1";
  const jobs = ALL_SUCCESS.map((j) => (/^(custom-)?publish-/.test(j) ? j.replace(/\tsuccess$/, "\tskipped") : j));
  const dir = scratch({ treeVersion: v, fx: fixtures({ v, jobs }) });
  const r = run(dir, [`v${v}`]);
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /skipped \(prerelease: by design\)/);
});

test("exit 2: green with the report-only tree-vs-tag line", () => {
  const dir = scratch({ treeVersion: "0.11.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["0.10.0"]);
  assert.equal(r.status, 2, r.out);
  assert.match(r.out, /REPORT: tree is at 0\.11\.0, verified tag is v0\.10\.0/);
  assert.match(r.out, /every channel serves 0\.10\.0 \(1 report-only finding\(s\) above\)/);
});

test("exit 3: no network is a named skip, never a verdict", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const forced = run(dir, ["0.10.0"], { MEMSTEAD_VERIFY_OFFLINE: "1" });
  assert.equal(forced.status, 3, forced.out);
  assert.match(forced.out, /^SKIPPED: no network/m);
  // The probe itself failing is the same skip.
  writeFileSync(join(dir, "bin/curl"), "#!/bin/bash\nexit 7\n");
  const down = run(dir, ["0.10.0"]);
  assert.equal(down.status, 3, down.out);
  assert.match(down.out, /SKIPPED: no network \(api\.github\.com unreachable\)/);
});

// ── --prose and the changelog check ─────────────────────────────────────────
//
// The published binary is a stub in a pre-populated cache (the download
// never happens); the tags come from MEMSTEAD_VERIFY_TAGS; the prose set
// and the changelog are fixtures in the scratch tree.

function proseScratch({ treeVersion, changelog }) {
  const dir = scratch({ treeVersion, fx: fixtures({ v: treeVersion, jobs: ALL_SUCCESS }) });
  mkdirSync(join(dir, "ci"), { recursive: true });
  cpSync(join(dirname(SCRIPTS), "ci", "check_prose.py"), join(dir, "ci", "check_prose.py"));
  mkdirSync(join(dir, "xtask"), { recursive: true });
  writeFileSync(join(dir, "xtask", "docs-guard-allow.txt"), "# none\n");
  // The "published" binary: accepts `health --strict` only.
  const cache = join(dir, "cache", treeVersion);
  mkdirSync(cache, { recursive: true });
  writeFileSync(join(cache, "memstead"), `#!/bin/bash
if [ "$1" = "--version" ]; then echo "memstead ${treeVersion}+gpublished"; exit 0; fi
args="$*"
case "$args" in
  "--help") echo "Options:"; echo "      --json"; echo "      --quiet"; exit 0 ;;
  "health --help") echo "Options:"; echo "      --strict"; echo "      --json"; exit 0 ;;
  *) exit 2 ;;
esac
`);
  chmodSync(join(cache, "memstead"), 0o755);
  if (changelog !== undefined) writeFileSync(join(dir, "CHANGELOG.md"), changelog);
  return dir;
}

test("--prose: a prose set ahead of the published binary is reported (exit 2); one at the tag is not (exit 0)", () => {
  const dir = proseScratch({ treeVersion: "0.10.0" });
  const ahead = join(dir, "prose-ahead");
  mkdirSync(ahead);
  writeFileSync(join(ahead, "guide.md"), "# Guide\n\n```bash\nmemstead health --strict --consume\nmemstead frobnicate\n```\n");
  const at = join(dir, "prose-at");
  mkdirSync(at);
  writeFileSync(join(at, "guide.md"), "# Guide\n\n```bash\nmemstead health --strict\n```\n");
  const seams = { MEMSTEAD_VERIFY_TAGS: "0.8.1 0.9.0 0.10.0", MEMSTEAD_VERIFY_CACHE: join(dir, "cache") };

  const r = run(dir, ["--prose", "--prose-set", ahead], seams);
  assert.equal(r.status, 2, r.out);
  assert.match(r.out, /prose report against the published v0\.10\.0 binary \(0\.10\.0\+gpublished\)/);
  assert.match(r.out, /REPORT: prose ahead of v0\.10\.0: .*guide\.md:4: flag: `--consume` is not a flag of `memstead health`/);
  assert.match(r.out, /REPORT: prose ahead of v0\.10\.0: .*guide\.md:5: command: `memstead frobnicate`/);

  const ok = run(dir, ["--prose", "--prose-set", at], seams);
  assert.equal(ok.status, 0, ok.out);
  assert.match(ok.out, /prose at v0\.10\.0: every documented command and flag resolves/);
  assert.doesNotMatch(ok.out, /REPORT: prose/);

  // Offline: the named skip, before anything is reported.
  const offline = Object.assign({}, seams, { MEMSTEAD_VERIFY_OFFLINE: "1" });
  const off = run(dir, ["--prose", "--prose-set", at], offline);
  assert.equal(off.status, 3, off.out);
  assert.match(off.out, /^SKIPPED: no network/m);
});

test("the changelog check reports a header without tag or note and a non-resolving compare link, and is silent on a fixed one", () => {
  const broken = `# Changelog

## [Unreleased]

## [0.10.0] - 2026-08-23

- x

## [0.9.0] - 2026-08-19

- y

[Unreleased]: https://github.com/memstead/memstead/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/memstead/memstead/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/memstead/memstead/compare/v0.8.1...v0.9.0
`;
  const dir = proseScratch({ treeVersion: "0.10.0", changelog: broken });
  const seams = { MEMSTEAD_VERIFY_TAGS: "0.8.1 0.10.0" };
  const r = run(dir, ["0.10.0"], seams);
  assert.equal(r.status, 2, r.out);
  assert.match(r.out, /REPORT: changelog: `## \[0\.9\.0\]` has no tag on origin and no "never published" note/);
  assert.match(r.out, /REPORT: changelog: compare link for \[0\.10\.0\] names v0\.9\.0, which is neither a tag/);
  assert.match(r.out, /REPORT: changelog: compare link for \[0\.9\.0\] names v0\.9\.0/);

  const fixed = broken
    .replace("## [0.9.0] - 2026-08-19", "## [0.9.0] - 2026-08-19 (cut, never published)")
    .replace("compare/v0.9.0...v0.10.0", "compare/v0.8.1...v0.10.0")
    .replace("[0.9.0]: https://github.com/memstead/memstead/compare/v0.8.1...v0.9.0\n", "");
  writeFileSync(join(dir, "CHANGELOG.md"), fixed);
  const ok = run(dir, ["0.10.0"], seams);
  assert.equal(ok.status, 0, ok.out);
  assert.doesNotMatch(ok.out, /REPORT: changelog/);
});

test("an unknown option is fatal (exit 1), never the report-verdict code the CI renders green", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["--bogus"]);
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /unknown option/);
});

// ── failed reads are unmeasured, never disagreements ────────────────────────

test("a rate-limited channel reads as UNMEASURED naming the reset, exit 2, never red", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["0.10.0"], {
    FAKE_FAIL_URLS: "crates.io",
    FAKE_FAIL_BODY: '{"message":"API rate limit exceeded"}',
    FAKE_RATELIMIT: "1",
    FAKE_RATELIMIT_RESET: "1756200000",
  });
  assert.equal(r.status, 2, r.out);
  assert.match(r.out, /crates\.io\s+.*UNMEASURED: rate limited \(anonymous quota; resets .*\)/);
  assert.doesNotMatch(r.out, /crates\.io\s+.*unreadable/);
  assert.match(r.out, /1 channel\(s\) UNMEASURED/);
  assert.doesNotMatch(r.out, /channel\(s\) or publish job\(s\) disagree/);
});

test("a plain HTTP error is UNMEASURED with the status named; the other channels still read (partial failure)", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["0.10.0"], {
    FAKE_FAIL_URLS: "registry.npmjs.org",
    FAKE_FAIL_CODE: "503",
    FAKE_FAIL_BODY: "upstream unavailable",
  });
  assert.equal(r.status, 2, r.out);
  assert.match(r.out, /npm @memstead\/wasm\s+.*UNMEASURED: HTTP 503/);
  assert.match(r.out, /Homebrew memstead-cli\s+.*0\.10\.0/);
  assert.match(r.out, /GitHub Release \(Latest\)\s+.*0\.10\.0/);
});

test("a run in which every channel is unmeasured does not claim every channel serves", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["0.10.0"], {
    FAKE_FAIL_URLS: "releases/latest releases/tags Formula plugin.json marketplace.json crates.io registry.npmjs.org",
    FAKE_FAIL_CODE: "500",
    FAKE_FAIL_BODY: "boom",
  });
  assert.equal(r.status, 2, r.out);
  assert.match(r.out, /8 channel\(s\) UNMEASURED/);
  assert.doesNotMatch(r.out, /^✓ every channel serves/m);
});

test("the pre-flight probe skips on a rate-limited API root instead of proceeding", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["0.10.0"], {
    FAKE_FAIL_URLS: "https://api.github.com",
    FAKE_FAIL_BODY: '{"message":"API rate limit exceeded"}',
    FAKE_RATELIMIT: "1",
  });
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /SKIPPED: api\.github\.com is rate limited/);
  assert.doesNotMatch(r.out, /Homebrew/);
});

test("an unresolvable latest target is a named skip (exit 3), not a red run", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, [], {
    FAKE_FAIL_URLS: "releases/latest",
    FAKE_FAIL_CODE: "403",
    FAKE_FAIL_BODY: '{"message":"API rate limit exceeded"}',
    FAKE_RATELIMIT: "1",
  });
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /SKIPPED: could not resolve the latest release \(rate limited/);
});
