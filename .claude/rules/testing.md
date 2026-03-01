---
paths:
  - "**/*test*"
  - "**/tests/**"
---

# Testing Conventions

## Unit tests
- Inline with source: `#[cfg(test)] mod tests { ... }`
- Async tests: `#[tokio::test]`
- Use `":memory:"` SQLite for tests needing a database — never hit the real filesystem
- The memory crate provides `SqliteMemoryDb::new()` for in-memory test databases

## Integration tests
- Located in `crates/atuin/tests/`
- Require Postgres — set `ATUIN_DB_URI` environment variable
- These are slower and typically run in CI, not locally

## Test runner
- Preferred: `cargo nextest`
- Fallback: `cargo test`

## History struct gotcha
When constructing `History` structs in tests, always include `agent_id: None`. This field was added by the agent-atuin fork and is required by the builder. Missing it causes compile errors.

Example:
```rust
History {
    id: HistoryId(id),
    timestamp: OffsetDateTime::now_utc(),
    command: "test command".into(),
    cwd: "/tmp".into(),
    exit: 0,
    duration: 100,
    session: "test".into(),
    hostname: "test:user".into(),
    deleted_at: None,
    agent_id: None,  // required by agent-atuin fork
}
```

## Benchmarks
- Framework: `divan` (in `atuin-history`)
- Run with `cargo bench`
