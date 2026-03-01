---
paths:
  - crates/atuin-memory/**
---

# Memory Crate Rules (`crates/atuin-memory/`)

## Schema
- `memories` table: id, description, cwd, repo_root, branch, commit_hash, agent_id, created_at
- `memory_commands` join table: links memories to history entries by history_id
- `memories_fts` FTS5 virtual table for full-text search on descriptions

## API requirements
- All public commands must support `--json` output for agent consumption
- Use `serde_json` for serialization; match the output format in `docs/AGENT_SETUP.md`

## Known gotcha: SQL parameter numbering
The `list()` method in `database.rs` builds dynamic SQL with optional filters. Parameter placeholders (`?1`, `?2`, `?3`) are hardcoded but filters are optional — if an earlier filter is `None`, parameter numbers shift and binds won't match placeholders. See `database.rs:211-250`. Use a counter to assign parameter numbers dynamically when adding new filters.

## agent_id field
Always include `agent_id` when constructing `Memory` structs. Read from `ATUIN_AGENT_ID` env var when available; default to `None`.

## Database
- Path: `~/.local/share/atuin/memory.db`
- SQLite with WAL mode, same pattern as other atuin client databases
- Use `":memory:"` SQLite for unit tests (see `SqliteMemoryDb::new`)
- Never modify existing migrations — only add new ones
