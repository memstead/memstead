# Rollback — Design Intent

What this skill must achieve. Use this as the reference when tuning SKILL.md.

## Core purpose

- restore the graph to the state of an earlier git commit
- safe, reversible operation — creates a new forward commit, never rewrites history

## Safety

- always show the commit list and get user confirmation first
- never use git reset --hard
- the rollback is a new commit on top, not a history rewrite

## Process

- restore entity files from the target commit
- inform the user that a Claude Code restart is needed to rebuild the in-memory store
- record the restoration as a new forward commit for traceability
