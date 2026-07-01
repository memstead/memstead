"""Shared JSON-RPC-over-stdio client for the basis-build smoke tests.

Spawns `memstead-mcp` as a subprocess with the given working directory,
performs the MCP `initialize` handshake, then exposes a `call` method
that issues `tools/call` requests and parses the response.

Designed for the CI smoke / strictness / mutation probes — not a
general-purpose MCP client. Three tradeoffs to keep in mind:

* **No notifications.** We send `initialized` as a notification (no
  response), but every subsequent message is a request that the script
  blocks on. Tools that emit progress notifications would deadlock
  here; the basis surface does not.
* **Synchronous, single-threaded.** The transport is one writer thread
  pumping JSON-RPC frames into stdin and one reader thread that yields
  responses by id. Each `call` waits for its own response.
* **No tool-list caching.** The script lists tools once after the
  handshake to verify the surface; subsequent tool calls go through
  `tools/call` directly.
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class ToolResponse:
    """Decoded `tools/call` response.

    `is_error` mirrors the wire-level `isError`. `text` is the
    concatenated `text` content blocks (the markdown body the basis
    server returns on read tools and on success of write tools).
    `structured_content` is the `structuredContent` envelope —
    `{ code, message, details }` on errors, tool-specific shape on
    success.
    """

    is_error: bool
    text: str
    structured_content: dict[str, Any] | None


class McpServer:
    """Spawns `memstead-mcp` over stdio and provides a request helper."""

    def __init__(self, binary: Path, cwd: Path, env: dict[str, str] | None = None) -> None:
        self.binary = binary
        self.cwd = cwd
        self.env = env or {}
        self.proc: subprocess.Popen[bytes] | None = None
        self._next_id = 1

    def __enter__(self) -> "McpServer":
        full_env = os.environ.copy()
        full_env.update(self.env)
        # No `--config` flag: the basis binary doesn't have one, and
        # the pro binary auto-discovers `.memstead/config.json` when no
        # `.memstead.toml` resolves. Either way the dispatcher boots
        # `FilesystemMcpServer` here.
        self.proc = subprocess.Popen(
            [str(self.binary)],
            cwd=str(self.cwd),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=full_env,
        )
        # Initialize handshake.
        init_resp = self._request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "basis-smoke", "version": "0"},
            },
        )
        assert "result" in init_resp, f"initialize failed: {init_resp}"
        # `initialized` is a notification — fire-and-forget.
        self._notify("notifications/initialized", {})
        return self

    def __exit__(self, *_excinfo: Any) -> None:
        if self.proc is None:
            return
        try:
            self.proc.stdin.close()
        except Exception:  # noqa: BLE001 — best-effort shutdown.
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.proc.kill()

    # ---- low-level transport ------------------------------------------------

    def _request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        assert self.proc is not None and self.proc.stdin is not None
        req_id = self._next_id
        self._next_id += 1
        frame = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params,
        }
        line = json.dumps(frame) + "\n"
        self.proc.stdin.write(line.encode())
        self.proc.stdin.flush()
        return self._read_response(req_id)

    def _notify(self, method: str, params: dict[str, Any]) -> None:
        assert self.proc is not None and self.proc.stdin is not None
        frame = {"jsonrpc": "2.0", "method": method, "params": params}
        line = json.dumps(frame) + "\n"
        self.proc.stdin.write(line.encode())
        self.proc.stdin.flush()

    def _read_response(self, expected_id: int) -> dict[str, Any]:
        assert self.proc is not None and self.proc.stdout is not None
        deadline = time.time() + 30.0
        while True:
            if time.time() > deadline:
                stderr_tail = (self.proc.stderr.read1(4096) if self.proc.stderr else b"").decode(errors="replace")
                raise TimeoutError(
                    f"memstead-mcp did not respond within 30s (expected id={expected_id}). "
                    f"Recent stderr:\n{stderr_tail}"
                )
            raw = self.proc.stdout.readline()
            if not raw:
                # EOF — process exited.
                stderr_tail = (self.proc.stderr.read() if self.proc.stderr else b"").decode(errors="replace")
                rc = self.proc.poll()
                raise RuntimeError(
                    f"memstead-mcp closed stdout (rc={rc}) before responding to id={expected_id}. "
                    f"Stderr:\n{stderr_tail}"
                )
            try:
                msg = json.loads(raw.decode())
            except json.JSONDecodeError:
                # Skip non-JSON lines defensively.
                continue
            if msg.get("id") == expected_id:
                return msg

    # ---- high-level helpers -------------------------------------------------

    def list_tools(self) -> list[str]:
        resp = self._request("tools/list", {})
        result = resp.get("result", {})
        return [t["name"] for t in result.get("tools", [])]

    def call(self, tool_name: str, arguments: dict[str, Any] | None = None) -> ToolResponse:
        resp = self._request(
            "tools/call",
            {"name": tool_name, "arguments": arguments or {}},
        )
        if "error" in resp:
            # Transport-level error — different from a tool-level error
            # envelope. Raise so the test harness sees the failure mode.
            raise RuntimeError(f"transport error on {tool_name}: {resp['error']}")
        result = resp.get("result", {})
        is_error = bool(result.get("isError", False))
        content = result.get("content", [])
        text = "\n".join(c.get("text", "") for c in content if c.get("type") == "text")
        structured = result.get("structuredContent")
        return ToolResponse(is_error=is_error, text=text, structured_content=structured)


# ---- workspace helpers ------------------------------------------------------


def init_vault_repo_workspace(
    memstead_binary: Path,
    memstead_mcp_binary: Path,
    root: Path,
    vault_name: str = "myvault",
    schema: str = "default@1.0.0",
) -> None:
    """Bootstrap a vault-repo workspace at `root`.

    Three steps, mirroring the documented cold-start flow:

    * `memstead vault-repo init` writes `vault-repo/.git/` with the unified
      `__MEMSTEAD` registry ref, the `main` README, and the
      `.memstead/workspace.toml` workspace marker.
    * `memstead workspace allow-create` adds the `[[vault_management.create]]`
      allowlist rule — a fresh workspace has none, and `vault init`
      refuses with VAULT_PATH_NOT_ALLOWED without it.
    * `memstead vault init` registers the writable vault. The vault command
      spawns `memstead-mcp` itself (operator-mode) and calls
      `memstead_vault_create` — passing `MEMSTEAD_MCP_BIN` as an env var so the
      spawn finds the right binary without a system-wide install.

    Vault-repo commits go through gix, which needs a committer
    identity for the reflog — CI runners have no git config, so pin
    one process-wide (the McpServer spawned later inherits it too).
    """
    for var in ("GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"):
        os.environ.setdefault(var, "memstead-ci")
    for var in ("GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"):
        os.environ.setdefault(var, "ci@memstead.io")
    subprocess.run(
        [str(memstead_binary), "vault-repo", "init"],
        cwd=str(root),
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [str(memstead_binary), "workspace", "allow-create", "--schema", "*", vault_name],
        cwd=str(root),
        check=True,
        capture_output=True,
    )
    env = os.environ.copy()
    env["MEMSTEAD_MCP_BIN"] = str(memstead_mcp_binary)
    subprocess.run(
        [str(memstead_binary), "vault", "init", vault_name, "--schema", schema],
        cwd=str(root),
        check=True,
        capture_output=True,
        env=env,
    )


def init_workspace(memstead_binary: Path, root: Path, name: str = "demo", schema: str = "default@1.0.0") -> None:
    """Run `memstead init --name <name> --schema <schema>` in `root`.

    Equivalent to the manual smoke flow: bootstrap `.memstead/config.json`
    plus the empty `.memstead/cache/` and `.memstead/memstead-io/` subdirs that
    `FilesystemEngine::init` expects.
    """
    subprocess.run(
        [str(memstead_binary), "init", "--name", name, "--schema", schema],
        cwd=str(root),
        check=True,
        capture_output=True,
    )


def assert_eq(actual: Any, expected: Any, label: str) -> None:
    """Loud assertion with a labelled failure message — easier to scan
    than vanilla `assert` output in CI logs."""
    if actual != expected:
        sys.stderr.write(f"FAIL ({label}): expected {expected!r}, got {actual!r}\n")
        sys.exit(1)


def assert_true(cond: bool, label: str) -> None:
    if not cond:
        sys.stderr.write(f"FAIL ({label})\n")
        sys.exit(1)


def fresh_workspace() -> Path:
    """Return a freshly-created temp dir. Caller is responsible for
    cleanup; on CI the runner reaps the dir on job teardown."""
    return Path(tempfile.mkdtemp(prefix="memstead-basis-smoke-"))


def cleanup_workspace(path: Path) -> None:
    shutil.rmtree(path, ignore_errors=True)
