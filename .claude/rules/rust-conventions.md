# Rust Conventions

## Error handling
- `eyre::Result` in binary crates (`atuin`, `atuin-server`)
- `thiserror` for typed errors in library crates (`atuin-client`, `atuin-common`, etc.)
- Never use `unwrap()` in production code; `expect()` only with descriptive messages

## Safety
- `#![deny(unsafe_code)]` on client and common crates
- `#![forbid(unsafe_code)]` on server crates
- No exceptions — find safe alternatives

## Linting
- Clippy: `pedantic` + `nursery` lints on the main crate
- CI enforces `-D warnings -D clippy::redundant_clone`
- Run `cargo clippy -- -D warnings` before committing

## Formatting
- `cargo fmt` with default config (`reorder_imports = true` is the only override)

## IDs
- UUIDv7 (time-ordered) for all identifiers
- Use newtype wrappers: `HistoryId`, `RecordId`, `HostId`

## Serialization
- MessagePack for encrypted payloads (record store)
- JSON for API responses and `--json` output
- TOML for configuration files

## Async
- Runtime: tokio
- Client uses `current_thread` runtime; server uses `multi_thread`
- All database traits use `#[async_trait]`

## Storage traits
- `Database` trait for client-side storage
- `Store` trait for record store
- `Database` trait (separate) for server storage
- All are `async_trait` with `Send + Sync + 'static` bounds
