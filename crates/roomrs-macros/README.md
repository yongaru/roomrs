# roomrs-macros

Procedural macros for [roomrs](https://github.com/yongaru/roomrs), including `#[entity]`, `#[dao]`, `#[query]`, `#[database]`, and migration-related helpers. The macros generate SQLite persistence code and validate SQL against schema snapshots at compile time.

## Usage

This is an internal implementation crate. Application code should normally depend on [`roomrs`](https://crates.io/crates/roomrs), which re-exports these macros.

See the [repository README](https://github.com/yongaru/roomrs#readme) for declarations and schema workflow.
