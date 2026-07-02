### command cheat-sheet — full walkthrough with error codes:
### docs-site guide "Publish a mem"
### (../docs-site/src/content/docs/guides/publish-a-mem.md)

### log in once (GitHub device flow) — optional, publish auto-triggers it
memstead login

### preview — resolves mem/version/scope/size, uploads nothing
cd /path/to/workspace
memstead publish --mem knowledge --dry-run

### publish — one step, any workspace (folder or multi-mem)
memstead publish --mem knowledge

### ship an update — bump + publish together
memstead publish --mem knowledge --version 0.2.0

### install someone else's
memstead install github:<handle>/knowledge

### unpublish
memstead unpublish github:<handle>/knowledge
