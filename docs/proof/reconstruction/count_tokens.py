#!/usr/bin/env python3
"""Token-count Claude Code session transcripts (JSONL).

Two independent measures per transcript:

1. content_tokens — the conversation as text: every user turn (including
   tool results fed back to the model), every assistant turn (text and
   tool-call arguments), tokenized with tiktoken's o200k_base. This is a
   neutral, deterministic stand-in for "how many tokens is this
   conversation" — the same tokenizer must be used on both sides of any
   ratio, which makes the ratio robust to the tokenizer choice to first
   order. It is NOT the Anthropic tokenizer.

2. api_usage — the token accounting Claude Code itself recorded per
   assistant API call (input_tokens, cache_creation_input_tokens,
   cache_read_input_tokens, output_tokens), deduplicated by API message id
   (streaming writes several rows per message; the last row carries the
   final usage).

Usage: count_tokens.py FILE.jsonl [FILE.jsonl ...]
Prints a per-file table and totals as JSON on stdout.
"""
import json
import sys

import tiktoken

ENC = tiktoken.get_encoding("o200k_base")


def text_of(content):
    """Flatten a message content field (str or block list) to text."""
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    parts = []
    for block in content:
        if not isinstance(block, dict):
            continue
        btype = block.get("type")
        if btype == "text":
            parts.append(block.get("text", ""))
        elif btype == "tool_result":
            parts.append(text_of(block.get("content")))
        elif btype == "tool_use":
            parts.append(json.dumps(block.get("input", {}), ensure_ascii=False))
        elif btype == "thinking":
            parts.append(block.get("thinking", ""))
    return "\n".join(parts)


def count_file(path):
    content_tokens = 0
    usage_by_id = {}
    turns = 0
    for line in open(path, encoding="utf-8"):
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if row.get("type") not in ("user", "assistant"):
            continue
        msg = row.get("message") or {}
        text = text_of(msg.get("content"))
        if text:
            content_tokens += len(ENC.encode(text, disallowed_special=()))
            turns += 1
        usage = msg.get("usage")
        if usage and msg.get("id"):
            usage_by_id[msg["id"]] = usage  # last row per id wins
    api = {
        "input_tokens": 0,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": 0,
    }
    for usage in usage_by_id.values():
        for key in api:
            api[key] += usage.get(key) or 0
    return {
        "file": path,
        "turns": turns,
        "content_tokens": content_tokens,
        "api_calls": len(usage_by_id),
        "api_usage": api,
    }


def main(paths):
    results = [count_file(p) for p in paths]
    total = {
        "files": len(results),
        "content_tokens": sum(r["content_tokens"] for r in results),
        "api_usage": {
            k: sum(r["api_usage"][k] for r in results)
            for k in results[0]["api_usage"]
        }
        if results
        else {},
    }
    json.dump({"per_file": results, "total": total}, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main(sys.argv[1:])
