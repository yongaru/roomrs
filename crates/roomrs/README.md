# roomrs

[roomrs](https://github.com/yongaru/roomrs) is a Room-style local SQLite persistence library for Rust. It provides entities, DAOs, compile-time SQL validation, migrations, live queries, and runtime-agnostic async support.

## Installation

```toml
[dependencies]
roomrs = "0.2"
```

See the [repository README](https://github.com/yongaru/roomrs#readme) for examples, feature flags, and migration guidance.

## Crate role

This is the public facade crate. Depend on `roomrs` rather than its internal implementation crates.
