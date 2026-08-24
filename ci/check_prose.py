#!/usr/bin/env python3
"""Do the documents describe the binary that exists?

One checker, run three ways: over the public prose set as a
``run-tests.sh`` leg, over the flagship documents at release time (from
``xtask release``, whole-file scope), and over the private prose set in the
workspace repo's hygiene lane. It takes the file set and the binary as
arguments and hard-codes no path of either repository.

What it resolves, per Markdown file:

* every ``memstead <cmd> [<sub>]`` invocation and every long flag attached
  to it, inside fenced ``bash`` / ``sh`` / ``shell`` / ``zsh`` / ``console``
  blocks and inside ``run:`` lines of fenced ``yaml`` blocks (``--scope
  fenced``, the default), or additionally in prose and inline code
  (``--scope whole-file``, the flagship's historical polarity, where a
  sentence like "run memstead quickstart" is a documented command);
* every relative link ``[text](path)`` in the same files: the target must
  exist on disk, relative to the file; inside a routed content tree
  (``--routes-root``, a docs site whose pages link by route) a link
  resolves as a route against the page's own route.

A command resolves when ``<bin> <cmd> [<sub>] --help`` exits 0 (a second
token that is not a subcommand is tried as a positional: ``<bin> <cmd>
--help``). A flag resolves when the resolved command's ``--help`` output
lists it, or the root ``--help`` does (global flags). Flags on commands
other than ``memstead`` are never checked.

Allowlist (``--allow``, repeatable): the ``xtask/docs-guard-allow.txt``
format, one phrase per line, ``#`` comments; a plain entry names the first
token after "memstead" or a two-token phrase that is prose, not a command;
a ``flag:--name`` entry admits a flag by name; a ``re:<regex>`` entry admits
any command phrase the regex fully matches (placeholders such as
``<scope>/<name>`` or ``acme/...`` patterns). An entry is a claim that the
phrase is prose, never a way to silence a missing command.

Exit 0 when every invocation, flag and link resolves; 1 with one line per
finding (``file:line: kind: detail``) otherwise; 2 on misuse. ``--self-test``
runs the fixtures under ``ci/fixtures/prose/`` against a stub binary and
exits 0 only when every fixture judges as documented there.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from functools import lru_cache

CODE_LANGS = {"bash", "sh", "shell", "zsh", "console"}
FENCE_RE = re.compile(r"^(\s*)(`{3,}|~{3,})\s*([A-Za-z0-9_+-]*)")
# `memstead` followed by one or two lowercase tokens (the command grammar).
CMD_RE = re.compile(r"(?:^|[\s`(>*\"'])memstead(?:\s+--?[A-Za-z][\w-]*(?:[= ]\S+)?)*\s+([a-z][a-z-]*)(?:\s+([a-z][a-z-]*))?")
LONG_FLAG_RE = re.compile(r"(?<![\w-])--([a-z][a-z0-9-]*)")
LINK_RE = re.compile(r"(?<!\!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
# Inline code spans and prose `memstead <cmd>` mentions, whole-file scope.
INLINE_CODE_RE = re.compile(r"`([^`\n]+)`")


def split_shell_commands(line: str) -> list[str]:
    """Split one shell line on the connectors that start a new command;
    a trailing `# comment` is not part of any command."""
    line = line.strip()
    if line.startswith("$ "):
        line = line[2:]
    line = re.sub(r"(^|\s)#.*$", "", line)
    parts = re.split(r"\s*(?:&&|\|\||\||;)\s*", line)
    return [p for p in parts if p]


# Global flags that take a value; the token after them is never the command.
VALUE_FLAGS = {"--workspace", "--role", "--mem", "--include", "--format", "--schema", "--agent", "--repo"}
TOKEN_RE = re.compile(r"^[a-z][a-z-]*$")


def memstead_invocations(command: str) -> list[tuple[str, str | None, list[str]]]:
    """`(cmd, sub, flags)` for every `memstead ...` in one shell command,
    by tokens: after `memstead`, leading flags are skipped (a value-taking
    global flag skips its value too), the first bare token is the command,
    a second bare token is tried as a subcommand, and every long flag on
    the invocation is collected."""
    tokens = command.replace("=", " = ").split()
    out = []
    i = 0
    while i < len(tokens):
        tok = tokens[i].strip("\"'`()")
        if tok != "memstead" and not tok.endswith("/memstead"):
            i += 1
            continue
        cmd, sub, flags = "", None, []
        j = i + 1
        while j < len(tokens):
            t = tokens[j]
            if t == "=":
                j += 1
                continue
            if t.startswith("--"):
                name = t[2:]
                if re.match(r"^[a-z][a-z0-9-]*$", name):
                    flags.append(name)
                if t in VALUE_FLAGS and j + 1 < len(tokens) and tokens[j + 1] != "=":
                    j += 2
                    continue
                j += 1
                continue
            if t.startswith("-"):
                j += 1
                continue
            if cmd == "":
                if TOKEN_RE.match(t):
                    cmd = t
                    j += 1
                    continue
                break  # a placeholder or path where the command would be: not an invocation
            if sub is None and TOKEN_RE.match(t):
                sub = t
            j += 1
        if cmd or flags:
            out.append((cmd, sub, flags))
        i = j if j > i else i + 1
    return out


def scan_file(path: str, scope: str):
    """Yield `(line_no, kind, payload)`; kind is `command` or `link`."""
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    in_fence = None  # (marker, lang)
    yaml_block_scalar = False
    for i, raw in enumerate(lines, start=1):
        fence = FENCE_RE.match(raw)
        if fence and (in_fence is None or raw.strip().startswith(in_fence[0])):
            if in_fence is None:
                in_fence = (fence.group(2)[0] * 3, fence.group(3).lower())
            else:
                in_fence = None
                yaml_block_scalar = False
            continue
        if in_fence is not None:
            lang = in_fence[1]
            if lang in CODE_LANGS:
                for cmd in split_shell_commands(raw.rstrip("\\")):
                    for inv in memstead_invocations(cmd):
                        yield i, "command", inv
            elif lang in {"yaml", "yml"}:
                stripped = raw.strip()
                if re.match(r"^-?\s*run:\s*[|>]", stripped):
                    yaml_block_scalar = True
                    continue
                if re.match(r"^-?\s*run:\s*\S", stripped):
                    yaml_block_scalar = False
                    value = stripped.split("run:", 1)[1].strip()
                    for cmd in split_shell_commands(value):
                        for inv in memstead_invocations(cmd):
                            yield i, "command", inv
                    continue
                if yaml_block_scalar:
                    if stripped and re.match(r"^-?\s*[a-z_-]+:\s", stripped) and not stripped.startswith("memstead"):
                        yaml_block_scalar = False
                    else:
                        for cmd in split_shell_commands(raw.rstrip("\\")):
                            for inv in memstead_invocations(cmd):
                                yield i, "command", inv
            # Other fenced languages: not shell, never scanned.
            continue
        # Outside fences.
        for link in LINK_RE.findall(raw):
            yield i, "link", link
        if scope == "whole-file":
            for span in INLINE_CODE_RE.findall(raw):
                for inv in memstead_invocations(span):
                    yield i, "command", inv
            prose = INLINE_CODE_RE.sub(" ", raw)
            for m in CMD_RE.finditer(prose):
                yield i, "command", (m.group(1), m.group(2), [])


class Resolver:
    def __init__(self, bin_path: str):
        self.bin = bin_path

    @lru_cache(maxsize=None)
    def help(self, *args: str) -> str | None:
        try:
            r = subprocess.run(
                [self.bin, *args, "--help"],
                capture_output=True,
                text=True,
                timeout=60,
                env={**os.environ, "NO_COLOR": "1"},
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        return (r.stdout + r.stderr) if r.returncode == 0 else None

    def resolve_command(self, cmd: str, sub: str | None) -> tuple[bool, tuple[str, ...]]:
        """(resolves, the arg vector whose help lists the flags)."""
        if cmd == "":
            return True, ()
        if sub is not None and self.help(cmd, sub) is not None:
            return True, (cmd, sub)
        if self.help(cmd) is not None:
            return True, (cmd,)
        return False, ()

    @lru_cache(maxsize=None)
    def flags_of(self, *args: str) -> set[str]:
        text = self.help(*args) or ""
        found = set(re.findall(r"(?m)^\s*(?:-[A-Za-z],\s+)?--([a-z][a-z0-9-]*)", text))
        # Global flags ride every subcommand; the root help lists them.
        if args:
            found |= self.flags_of()
        return found


def load_allow(paths: list[str]):
    phrases, flags, patterns = set(), set(), []
    for p in paths:
        if not p or not os.path.isfile(p):
            continue
        with open(p, encoding="utf-8") as f:
            for raw in f:
                line = raw.strip()
                if not line or line.startswith("#"):
                    continue
                if line.startswith("flag:"):
                    flags.add(line[len("flag:") :].strip().lstrip("-"))
                elif line.startswith("re:"):
                    patterns.append(re.compile(line[len("re:") :].strip()))
                else:
                    phrases.add(line)
    return phrases, flags, patterns


def routed_root_of(path: str, routes_roots: list[str]) -> str | None:
    """The routed content root `path` lives under, if any."""
    ap = os.path.abspath(path)
    for r in routes_roots:
        ar = os.path.abspath(r)
        if ap.startswith(ar + os.sep):
            return ar
    return None


SCHEME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]*:")


def link_resolves(path: str, target: str, root: str | None, routes_roots: list[str]) -> bool:
    """A relative link resolves as a file path; inside a routed content
    tree (a docs site whose pages link by route, `../../glossary/`,
    `/reference/cli/cli/`) it resolves as a route: the page's own route is
    its path without extension, a route maps to `<route>.md`,
    `<route>.mdx` or `<route>/index.md(x)` under the content root."""
    routed = routed_root_of(path, routes_roots)
    if routed is None:
        if target.startswith("/"):
            return os.path.exists(os.path.join(root or os.getcwd(), target.lstrip("/")))
        return os.path.exists(os.path.join(os.path.dirname(path), target))
    # Route resolution.
    page_route = os.path.splitext(os.path.relpath(os.path.abspath(path), routed))[0]
    if os.path.basename(page_route) == "index":
        page_route = os.path.dirname(page_route)
    if target.startswith("/"):
        route = os.path.normpath(target.lstrip("/"))
    else:
        route = os.path.normpath(os.path.join(page_route, target))
    route = route.strip("/")
    if route in ("", "."):
        return True
    for candidate in (
        os.path.join(routed, route + ".md"),
        os.path.join(routed, route + ".mdx"),
        os.path.join(routed, route, "index.md"),
        os.path.join(routed, route, "index.mdx"),
        os.path.join(routed, route),
    ):
        if os.path.exists(candidate):
            return True
    # A plain file link (an image, a sibling file) still resolves as a path.
    return os.path.exists(os.path.join(os.path.dirname(path), target))


def check(files, bin_path, scope, allow_paths, root=None, routes_roots=None):
    resolver = Resolver(bin_path)
    routes_roots = routes_roots or []
    phrases, allowed_flags, patterns = load_allow(allow_paths)
    findings = []
    for path in files:
        if not path.endswith((".md", ".mdx")) or not os.path.isfile(path):
            continue
        for line_no, kind, payload in scan_file(path, scope):
            if kind == "link":
                target = payload.split("#", 1)[0]
                # Only relative links are checked: anything with a URI
                # scheme (https:, mailto:, a house scheme such as attic:)
                # points outside the tree.
                if not target or SCHEME_RE.match(target):
                    continue
                if not link_resolves(path, target, root, routes_roots):
                    findings.append(f"{path}:{line_no}: link: `{payload}` does not resolve")
                continue
            cmd, sub, flags = payload
            phrase = f"{cmd} {sub}" if sub else cmd
            if cmd and (cmd in phrases or phrase in phrases or any(p.fullmatch(phrase) for p in patterns)):
                continue
            ok, argv = resolver.resolve_command(cmd, sub)
            if not ok:
                findings.append(f"{path}:{line_no}: command: `memstead {phrase}` is not a command of the binary")
                continue
            if flags:
                known = resolver.flags_of(*argv)
                for flag in flags:
                    if flag in ("help", "version") or flag in allowed_flags or flag in known:
                        continue
                    where = f"memstead {' '.join(argv)}" if argv else "memstead"
                    findings.append(f"{path}:{line_no}: flag: `--{flag}` is not a flag of `{where}`")
    return findings


def self_test(fixtures_dir: str) -> int:
    """Every fixture file names its expected findings in a trailing
    `<!-- expect: N -->` comment; the stub binary under the fixtures dir
    accepts exactly the commands and flags the fixtures assume."""
    stub = os.path.join(fixtures_dir, "memstead-stub.py")
    allow = os.path.join(fixtures_dir, "allow.txt")
    # `site/` is a routed content tree: its pages link by route.
    site = os.path.join(fixtures_dir, "site")
    failures = 0
    candidates = []
    for dirpath, _dirs, names in os.walk(fixtures_dir):
        for name in names:
            if name.endswith(".md"):
                candidates.append(os.path.join(dirpath, name))
    for path in sorted(candidates):
        name = os.path.relpath(path, fixtures_dir)
        with open(path, encoding="utf-8") as f:
            text = f.read()
        scope = "whole-file" if "<!-- scope: whole-file -->" in text else "fenced"
        m = re.search(r"<!-- expect: (\d+) -->", text)
        expected = int(m.group(1)) if m else 0
        found = check([path], stub, scope, [allow], root=fixtures_dir, routes_roots=[site])
        mark = "✓" if len(found) == expected else "✗"
        print(f"  {mark} {name} ({scope}): {len(found)} finding(s), expected {expected}")
        for line in found:
            print(f"      {line}")
        if len(found) != expected:
            failures += 1
    return 1 if failures else 0


def expand_dirs(paths: list[str]) -> list[str]:
    """A directory argument stands for every ``.md``/``.mdx`` file below it
    (sorted); files pass through unchanged, in the given order."""
    out = []
    for p in paths:
        if os.path.isdir(p):
            for dirpath, dirs, names in os.walk(p):
                dirs.sort()
                out.extend(os.path.join(dirpath, n) for n in sorted(names) if n.endswith((".md", ".mdx")))
        else:
            out.append(p)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("files", nargs="*", help="Markdown files to check; a directory stands for every .md/.mdx below it")
    ap.add_argument("--memstead", help="the binary every command and flag resolves against")
    ap.add_argument("--scope", choices=["fenced", "whole-file"], default="fenced")
    ap.add_argument("--allow", action="append", default=[], help="allowlist file (repeatable)")
    ap.add_argument("--root", help="base for absolute links (default: the working directory)")
    ap.add_argument(
        "--routes-root",
        action="append",
        default=[],
        help="a content tree whose pages link by route (a docs site); links inside it resolve as routes (repeatable)",
    )
    ap.add_argument("--self-test", metavar="FIXTURES_DIR", help="run the fixture suite and exit")
    args = ap.parse_args()

    if args.self_test:
        return self_test(args.self_test)
    if not args.memstead or not args.files:
        ap.error("--memstead and at least one file are required")
    if not (os.path.isfile(args.memstead) and os.access(args.memstead, os.X_OK)):
        print(f"check_prose: {args.memstead} is not an executable binary", file=sys.stderr)
        return 2
    files = expand_dirs(args.files)
    findings = check(files, args.memstead, args.scope, args.allow, root=args.root, routes_roots=args.routes_root)
    if findings:
        print(f"✗ {len(findings)} documented invocation(s), flag(s) or link(s) do not resolve against {args.memstead}:")
        for line in findings:
            print(f"    {line}")
        return 1
    print(f"✓ every documented memstead invocation, flag and relative link in {len(files)} file(s) resolves against {args.memstead}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
