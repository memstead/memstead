### set alias
alias memstead=/path/to/memstead/engine/target/release/memstead

### log in once (GitHub device flow)
memstead login

### preview — resolves mem/version/scope/size, uploads nothing
cd /path/to/workspace
memstead publish --mem knowledge --dry-run

### publish — one step, any workspace (folder or multi-mem)
memstead publish --mem knowledge

### ship an update — bump + publish together
memstead publish --mem knowledge --version 0.2.0

### unpublish
memstead unpublish github:<handle>/knowledge
