#!/usr/bin/env python3
"""A stand-in `memstead` for the prose checker's self-test: a fixed
command tree whose `--help` lists fixed flags, nothing else."""
import sys

ROOT_FLAGS = ["--json", "--quiet", "--workspace <PATH>", "--role <ROLE>"]
TREE = {
    ("quickstart",): ["--agent <AGENT>", "--repo <PATH>"],
    ("health",): ["--include <INCLUDE>", "--strict"],
    ("install",): ["--registry <URL>"],
    ("schema",): [],
    ("schema", "validate"): [],
    ("projection",): [],
    ("projection", "brief"): ["--all", "--consume", "--operation <OPERATION>"],
}

args = sys.argv[1:]
if not args or args[-1] != "--help":
    sys.exit(2)
path = tuple(a for a in args[:-1] if not a.startswith("-"))
if path and path not in TREE:
    print("error: unrecognized subcommand", file=sys.stderr)
    sys.exit(2)
print("stub\n\nOptions:")
for f in (TREE.get(path, []) if path else []) + ROOT_FLAGS:
    print(f"      {f}")
print("  -h, --help")
