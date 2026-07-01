### set alias
alias memstead=/path/to/memstead/engine/target/release/memstead

### log in once (GitHub device flow)
memstead login

### preview — resolves vault/version/scope/size, uploads nothing
cd /path/to/workspace
memstead publish --vault knowledge --dry-run

### publish — one step, any workspace (folder or multi-vault)
memstead publish --vault knowledge

### ship an update — bump + publish together
memstead publish --vault knowledge --version 0.2.0

### unpublish
memstead unpublish github:<handle>/knowledge
