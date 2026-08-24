// Tests for scripts/untagged-release.sh, scripts/untagged-release-issue.sh
// and the untagged-release arm of scripts/ci-status.sh, against scratch
// git repositories with fabricated tags and commit dates.
//
// Node built-ins only (node --test), like the other script tests here.
// Every scenario builds a bare "origin" plus a clone so the scripts read
// tags the way they do in real life: over `git ls-remote`, never from the
// local ref store.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, chmodSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPTS = dirname(fileURLToPath(import.meta.url));
const ROOT = dirname(SCRIPTS);

function sh(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { encoding: "utf8", ...opts });
  return { status: r.status, out: (r.stdout ?? "") + (r.stderr ?? "") };
}

function git(cwd, args, env = {}) {
  const r = spawnSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, ...env } });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")} failed in ${cwd}:\n${r.stderr}`);
  return r.stdout.trim();
}

// A scratch "public" checkout: bare origin + clone carrying the three
// scripts under test and a minimal workspace Cargo.toml. Returns the
// clone path; `version` is the workspace version committed at HEAD with
// the given committer date (ISO 8601), pushed to origin/main.
function scratch({ version, cutDate, tags }) {
  const dir = mkdtempSync(join(tmpdir(), "untagged-release-"));
  const origin = join(dir, "origin.git");
  const clone = join(dir, "clone");
  git(dir, ["init", "--bare", "--quiet", "--initial-branch=main", origin]);
  git(dir, ["clone", "--quiet", origin, clone]);
  git(clone, ["config", "user.email", "t@example.com"]);
  git(clone, ["config", "user.name", "t"]);
  mkdirSync(join(clone, "scripts"), { recursive: true });
  for (const f of ["untagged-release.sh", "untagged-release-issue.sh", "ci-status.sh"]) {
    cpSync(join(SCRIPTS, f), join(clone, "scripts", f));
    chmodSync(join(clone, "scripts", f), 0o755);
  }
  const cargo = (v) => `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${v}"\nedition = "2024"\n\n[workspace.dependencies]\nmemstead-base = { version = "${v}", path = "crates/memstead-base" }\n`;
  // Base commit at an older version, so the version bump is its own
  // commit the pickaxe can date.
  writeFileSync(join(clone, "Cargo.toml"), cargo("0.8.1"));
  const old = { GIT_AUTHOR_DATE: "2026-08-01T10:00:00+00:00", GIT_COMMITTER_DATE: "2026-08-01T10:00:00+00:00" };
  git(clone, ["add", "-A"], old);
  git(clone, ["commit", "--quiet", "-m", "base at 0.8.1"], old);
  for (const t of tags.filter((t) => t === "v0.8.1")) git(clone, ["tag", t]);
  writeFileSync(join(clone, "Cargo.toml"), cargo(version));
  const cut = { GIT_AUTHOR_DATE: cutDate, GIT_COMMITTER_DATE: cutDate };
  git(clone, ["add", "-A"], cut);
  git(clone, ["commit", "--quiet", "-m", `release: ${version}`], cut);
  for (const t of tags.filter((t) => t !== "v0.8.1")) git(clone, ["tag", t]);
  git(clone, ["push", "--quiet", "origin", "main"]);
  if (tags.length) git(clone, ["push", "--quiet", "origin", "--tags"]);
  return clone;
}

function isoDaysAgo(days) {
  return new Date(Date.now() - days * 86400_000).toISOString().replace(/\.\d{3}Z$/, "+00:00");
}

test("an untagged release older than a day refuses, naming sha and date", () => {
  const clone = scratch({ version: "0.10.0", cutDate: isoDaysAgo(3), tags: ["v0.8.1", "v0.9.0"] });
  const sha = git(clone, ["rev-parse", "--short", "HEAD"]);
  const r = sh(join(clone, "scripts/untagged-release.sh"), [], { cwd: clone });
  assert.equal(r.status, 1, r.out);
  assert.match(r.out, /UNTAGGED RELEASE/);
  assert.ok(r.out.includes(sha), `names the release commit ${sha}:\n${r.out}`);
  assert.match(r.out, /on \d{4}-\d{2}-\d{2}T/, "names the cut date");
  assert.match(r.out, /newest tag is v0\.9\.0/);
  // ci-status.sh refuses on the same state before any CI readout.
  const c = sh(join(clone, "scripts/ci-status.sh"), [], { cwd: clone });
  assert.equal(c.status, 1, c.out);
  assert.ok(c.out.includes(sha));
});

test("a tagged state passes, with SemVer precedence over a lexically larger tag", () => {
  // "v0.9.0" > "v0.10.0" as strings; only a SemVer comparison sees 0.10.0
  // as the newest tag and the workspace as tagged.
  const clone = scratch({ version: "0.10.0", cutDate: isoDaysAgo(3), tags: ["v0.8.1", "v0.9.0", "v0.10.0"] });
  const r = sh(join(clone, "scripts/untagged-release.sh"), [], { cwd: clone });
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /is tagged \(v0\.10\.0/);
  // ci-status.sh: the untagged arm passes, the CI readout fails open on a
  // non-GitHub remote and says so.
  const c = sh(join(clone, "scripts/ci-status.sh"), [], { cwd: clone });
  assert.equal(c.status, 0, c.out);
  assert.match(c.out, /is tagged/);
  assert.match(c.out, /fails open/);
});

test("a cut inside the grace period is reported, not refused", () => {
  const clone = scratch({ version: "0.10.0", cutDate: isoDaysAgo(0.1), tags: ["v0.8.1", "v0.9.0"] });
  const r = sh(join(clone, "scripts/untagged-release.sh"), [], { cwd: clone });
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /not yet tagged; the gate trips after 24h/);
  // The grace period is a parameter: shrink it and the same state trips.
  const t = sh(join(clone, "scripts/untagged-release.sh"), ["--max-age-hours", "1"], { cwd: clone });
  assert.equal(t.status, 1, t.out);
});

test("an unreadable remote skips with exit 3 and a fail-open notice", () => {
  const clone = scratch({ version: "0.10.0", cutDate: isoDaysAgo(3), tags: ["v0.8.1"] });
  git(clone, ["remote", "set-url", "origin", join(clone, "does-not-exist.git")]);
  const r = sh(join(clone, "scripts/untagged-release.sh"), [], { cwd: clone });
  assert.equal(r.status, 3, r.out);
  assert.match(r.out, /SKIPPED, this check fails open/);
  const c = sh(join(clone, "scripts/ci-status.sh"), [], { cwd: clone });
  assert.equal(c.status, 0, "ci-status fails open on the skip:\n" + c.out);
});

// ── the issue logic, against a recording fake `gh` ─────────────────────────
//
// The fake keeps the open-issue state in a file so one scenario can run
// the script twice (file, then update) and records every invocation.

function fakeGh(dir) {
  const bin = join(dir, "bin");
  mkdirSync(bin, { recursive: true });
  const state = join(dir, "gh-state.json");
  const log = join(dir, "gh-log");
  writeFileSync(state, JSON.stringify({ open: [] }));
  writeFileSync(join(bin, "gh"), `#!/bin/bash
# recording fake gh: issue list / create / edit / close
STATE="${state}"
echo "$*" >> "${log}"
case "$1 $2" in
  "issue list")
    node -e 'const s=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); process.stdout.write(JSON.stringify(s.open))' "$STATE" \\
      | node -e 'let j="";process.stdin.on("data",d=>j+=d).on("end",()=>{const a=JSON.parse(j);for(const i of a){if(i.title.startsWith("Untagged release:"))console.log(i.number)}})'
    ;;
  "issue create")
    node -e 'const fs=require("fs");const s=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));const t=process.argv[process.argv.indexOf("--title")+1];const n=(s.next||100);s.open.push({number:n,title:t});s.next=n+1;fs.writeFileSync(process.argv[1],JSON.stringify(s));console.log("https://example.test/issues/"+n)' "$STATE" "$@"
    ;;
  "issue edit")
    node -e 'const fs=require("fs");const s=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));const n=Number(process.argv[2]);const t=process.argv[process.argv.indexOf("--title")+1];for(const i of s.open){if(i.number===n)i.title=t}fs.writeFileSync(process.argv[1],JSON.stringify(s))' "$STATE" "$3" "$@"
    ;;
  "issue close")
    node -e 'const fs=require("fs");const s=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));const n=Number(process.argv[2]);s.open=s.open.filter(i=>i.number!==n);fs.writeFileSync(process.argv[1],JSON.stringify(s))' "$STATE" "$3"
    ;;
  *) echo "fake gh: unsupported: $*" >&2; exit 9 ;;
esac
`);
  chmodSync(join(bin, "gh"), 0o755);
  return {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}` },
    calls: () => (existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").filter(Boolean) : []),
    open: () => JSON.parse(readFileSync(state, "utf8")).open,
  };
}

test("issue logic: files once, updates the existing issue, closes when clear, files nothing when tagged", () => {
  // Tripped state first.
  const tripped = scratch({ version: "0.10.0", cutDate: isoDaysAgo(3), tags: ["v0.8.1", "v0.9.0"] });
  const gh = fakeGh(dirname(tripped));
  const script = join(tripped, "scripts/untagged-release-issue.sh");

  const first = sh(script, [], { cwd: tripped, env: gh.env });
  assert.equal(first.status, 1, first.out);
  assert.match(first.out, /filed https:\/\/example\.test\/issues\/100/);
  assert.equal(gh.open().length, 1);
  assert.match(gh.open()[0].title, /^Untagged release: v0\.10\.0 was cut but never tagged/);

  const second = sh(script, [], { cwd: tripped, env: gh.env });
  assert.equal(second.status, 1, second.out);
  assert.match(second.out, /updated open issue #100/);
  assert.equal(gh.open().length, 1, "never a second issue");
  assert.equal(gh.calls().filter((c) => c.startsWith("issue create")).length, 1);
  assert.equal(gh.calls().filter((c) => c.startsWith("issue edit")).length, 1);

  // The condition clears: the tag lands on origin.
  git(tripped, ["tag", "v0.10.0"]);
  git(tripped, ["push", "--quiet", "origin", "v0.10.0"]);
  const cleared = sh(script, [], { cwd: tripped, env: gh.env });
  assert.equal(cleared.status, 0, cleared.out);
  assert.match(cleared.out, /closed issue #100/);
  assert.equal(gh.open().length, 0);

  // A tagged state with nothing open files nothing.
  const again = sh(script, [], { cwd: tripped, env: gh.env });
  assert.equal(again.status, 0, again.out);
  assert.match(again.out, /nothing to file/);
  assert.equal(gh.calls().filter((c) => c.startsWith("issue create")).length, 1);
  assert.equal(gh.calls().filter((c) => c.startsWith("issue close")).length, 1);
});

test("issue logic: a skipped check touches no issue", () => {
  const clone = scratch({ version: "0.10.0", cutDate: isoDaysAgo(3), tags: ["v0.8.1"] });
  const gh = fakeGh(dirname(clone));
  git(clone, ["remote", "set-url", "origin", join(clone, "does-not-exist.git")]);
  const r = sh(join(clone, "scripts/untagged-release-issue.sh"), [], { cwd: clone, env: gh.env });
  assert.equal(r.status, 0, r.out);
  assert.match(r.out, /check skipped, issue state left as is/);
  assert.equal(gh.calls().filter((c) => !c.startsWith("issue list")).length, 0);
});

test("the workflow file parses and carries schedule, dispatch and the issue script", () => {
  const yml = readFileSync(join(ROOT, ".github/workflows/untagged-release.yml"), "utf8");
  assert.match(yml, /^\s+schedule:\n\s+- cron: '[^']+'/m);
  assert.match(yml, /^\s+workflow_dispatch:/m);
  assert.match(yml, /scripts\/untagged-release-issue\.sh/);
  assert.match(yml, /issues: write/);
  // A YAML parse when a parser is on the machine (ruby ships one on macOS
  // and most CI images); otherwise the structural checks above stand.
  const rb = spawnSync("ruby", ["-ryaml", "-e", "YAML.load_file(ARGV[0]); puts 'yaml ok'", join(ROOT, ".github/workflows/untagged-release.yml")], { encoding: "utf8" });
  if (rb.status === 0) assert.match(rb.stdout, /yaml ok/);
});
