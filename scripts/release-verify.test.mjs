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
# fake curl: serve a fixture file by URL pattern; the network probe succeeds
FX="${join(dir, "fx")}"
url=""
for a in "$@"; do case "$a" in http*) url="$a" ;; esac; done
case "$url" in
  https://api.github.com) exit 0 ;;
  */releases/latest) cat "$FX/latest.json" ;;
  */releases/tags/*) cat "$FX/release-tag.json" ;;
  */Formula/memstead-cli.rb) cat "$FX/memstead-cli.rb" ;;
  */Formula/memstead-mcp.rb) cat "$FX/memstead-mcp.rb" ;;
  */plugin.json) cat "$FX/plugin.json" ;;
  */marketplace.json) cat "$FX/marketplace.json" ;;
  *crates.io*) cat "$FX/crates.json" ;;
  *registry.npmjs.org*) cat "$FX/npm.json" ;;
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

test("an unknown option refuses with exit 2", () => {
  const dir = scratch({ treeVersion: "0.10.0", fx: fixtures({ v: "0.10.0", jobs: ALL_SUCCESS }) });
  const r = run(dir, ["--bogus"]);
  assert.equal(r.status, 2, r.out);
});
