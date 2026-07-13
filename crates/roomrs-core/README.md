# roomrs-core

Core runtime for [roomrs](https://github.com/yongaru/roomrs): SQLite database setup, connection pool, query execution, migrations, live-query invalidation, and shared error and type support.

## Usage

This is an internal implementation crate. Application code should normally depend on [`roomrs`](https://crates.io/crates/roomrs), which re-exports its supported public API.

See the [repository README](https://github.com/yongaru/roomrs#readme) for library usage and feature details.
