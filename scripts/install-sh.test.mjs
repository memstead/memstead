// Fixture tests for install.sh: latest-release resolution goes through the
// release host's redirect and never the REST API, so an address whose
// anonymous REST quota is exhausted installs like a fresh one; a failed
// resolution names the cause and the pinned-version escape and exits
// non-zero; a failed child-installer fetch is never masked by the pipe.
//
// Node built-ins only (node --test). The network is replaced by a fake
// `curl` on PATH whose api.github.com arm always answers the rate-limited
// shape — an error body plus curl's -f exit code, not an absent response —
// so the passing install proves that arm is unreachable.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, chmodSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPTS = dirname(fileURLToPath(import.meta.url));
const INSTALL_SH = join(SCRIPTS, "..", "install.sh");

const FAKE_CURL = `#!/bin/sh
# fake curl: logs every URL, serves the redirect resolution, the child
# installers, and a rate-limited REST API (error body + exit 22, the shape
# a real 403 has under -f).
url=""; out=""; prev=""
for a in "$@"; do
  case "$prev" in
    -o) out="$a"; prev=""; continue ;;
    -w) prev=""; continue ;;
  esac
  case "$a" in
    -o|-w) prev="$a" ;;
    http*) url="$a" ;;
  esac
done
echo "$url" >> "$CURL_LOG"
case "$url" in
  *api.github.com*)
    echo '{"message":"API rate limit exceeded for 1.2.3.4."}'
    exit 22 ;;
  */releases/latest)
    if [ "\${FAKE_FAIL_RESOLVE:-}" = 1 ]; then
      echo "curl: (6) Could not resolve host: github.com" >&2
      exit 6
    fi
    if [ "\${FAKE_BAD_REDIRECT:-}" = 1 ]; then
      printf 'https://github.com/%s/releases' "\${FAKE_REPO}"
    else
      printf 'https://github.com/%s/releases/tag/%s' "\${FAKE_REPO}" "\${FAKE_TAG}"
    fi
    exit 0 ;;
  *-installer.sh)
    if [ "\${FAKE_FAIL_CHILD_FETCH:-}" = 1 ]; then
      echo "curl: (22) The requested URL returned error: 404" >&2
      exit 22
    fi
    printf '#!/bin/sh\\n%s\\n' "\${FAKE_CHILD_BODY:-exit 0}" > "\${out:-/dev/stdout}"
    exit 0 ;;
esac
exit 0
`;

function scratch() {
  const dir = mkdtempSync(join(tmpdir(), "install-sh-"));
  mkdirSync(join(dir, "bin"));
  writeFileSync(join(dir, "bin", "curl"), FAKE_CURL);
  chmodSync(join(dir, "bin", "curl"), 0o755);
  return dir;
}

function runInstall({ args = [], env = {} } = {}) {
  const dir = scratch();
  const log = join(dir, "curl.log");
  writeFileSync(log, "");
  const res = spawnSync("sh", [INSTALL_SH, ...args], {
    encoding: "utf8",
    env: {
      PATH: `${join(dir, "bin")}:${process.env.PATH}`,
      CURL_LOG: log,
      FAKE_REPO: "memstead/memstead",
      FAKE_TAG: "v9.9.9",
      TMPDIR: dir,
      ...env,
    },
  });
  const urls = existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").filter(Boolean) : [];
  return { ...res, urls };
}

test("latest resolves via the redirect while the REST API is rate-limited", () => {
  const r = runInstall();
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /memstead installed \(v9\.9\.9\)/);
  assert.ok(r.urls.some((u) => u.endsWith("/releases/latest")), "redirect endpoint consulted");
  assert.ok(!r.urls.some((u) => u.includes("api.github.com")), "REST API never contacted");
});

test("no code path names the REST API at all", () => {
  const text = readFileSync(INSTALL_SH, "utf8");
  assert.ok(!text.includes("api.github.com"), "install.sh must not reference api.github.com");
});

test("failed resolution exits non-zero, names a cause and the pinned-version escape", () => {
  const r = runInstall({ env: { FAKE_FAIL_RESOLVE: "1" } });
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /could not resolve the latest release/);
  assert.match(r.stderr, /no network, or the repository has no published release/);
  assert.match(r.stderr, /--version <tag> or MEMSTEAD_VERSION=<tag>/);
  assert.ok(!r.stdout.includes("memstead installed"));
});

test("a redirect that lands off the tag page exits non-zero with the escape", () => {
  const r = runInstall({ env: { FAKE_BAD_REDIRECT: "1" } });
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /no tag redirect/);
  assert.match(r.stderr, /--version <tag> or MEMSTEAD_VERSION=<tag>/);
});

test("--version skips resolution entirely and pulls the pinned release", () => {
  const r = runInstall({ args: ["--version", "v1.2.3"] });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /memstead installed \(v1\.2\.3\)/);
  assert.ok(!r.urls.some((u) => u.endsWith("/releases/latest")), "no resolution call");
  assert.ok(r.urls.every((u) => !u.includes("api.github.com")));
  assert.ok(r.urls.some((u) => u.includes("/releases/download/v1.2.3/")));
});

test("MEMSTEAD_VERSION behaves like --version", () => {
  const r = runInstall({ env: { MEMSTEAD_VERSION: "v1.2.3" } });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /memstead installed \(v1\.2\.3\)/);
  assert.ok(!r.urls.some((u) => u.endsWith("/releases/latest")));
});

test("a failed child-installer fetch is not masked as success", () => {
  const r = runInstall({ env: { FAKE_FAIL_CHILD_FETCH: "1" } });
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /installer download failed/);
  assert.ok(!r.stdout.includes("memstead installed"));
});

test("a failing child installer fails the wrapper", () => {
  const r = runInstall({ env: { FAKE_CHILD_BODY: "exit 3" } });
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /install failed/);
  assert.ok(!r.stdout.includes("memstead installed"));
});
