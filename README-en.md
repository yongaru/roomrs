# roomrs

[한국어](README.md) | **English**

[![CI](https://github.com/yongaru/roomrs/actions/workflows/ci.yml/badge.svg)](https://github.com/yongaru/roomrs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/roomrs.svg)](https://crates.io/crates/roomrs)
[![docs.rs](https://img.shields.io/docsrs/roomrs)](https://docs.rs/roomrs)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-informational)](#supported-environments)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

A Rust SQLite persistence library where you declare entities, DAOs, and SQL in a style inspired by Android Room.

```rust
#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, todo: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;
}
```

## Why roomrs?

roomrs is not a general-purpose server ORM. It focuses on **local SQLite data** in desktop and mobile applications.

- Map structs to tables and traits to DAOs.
- Validate SQL tables, columns, and named parameters at compile time.
- Observe query results with `LiveQuery<T>` when data changes.
- Generate synchronous and runtime-independent asynchronous APIs from the same DAO.
- Embed versioned schema snapshots and validate forward migrations.
- Choose bundled or system linking for SQLite and SQLCipher.

If you need PostgreSQL or MySQL, Active Record, or a multi-database abstraction, Diesel or SeaORM will probably fit better. If you want an Android Room-like declarative local store, roomrs is designed for that use case.

## Quick start

### 1. Prepare the project and tools

```sh
cargo new roomrs-example
cd roomrs-example
cargo add roomrs@0.3.0
cargo install roomrs-cli
```

The default features compile SQLite from bundled source, so no system SQLite installation is required.

### 2. Write `src/main.rs`

```rust
use roomrs::{dao, database, entity};

#[entity(table = "todos")]
#[derive(Debug)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, todo: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct AppDb;

fn main() -> roomrs::Result<()> {
    let db = AppDb::builder().sqlite("app.db").build()?;
    let handle = db.run_sync();
    let dao = handle.todo_dao();

    let id = dao.add(&Todo {
        id: 0,
        title: "Get started with roomrs".into(),
        done: false,
    })?;
    println!("new ID: {id}");

    for todo in dao.by_done(false)? {
        println!("- {}", todo.title);
    }
    Ok(())
}
```

### 3. Export the schema and run

```sh
cargo roomrs schema export
cargo build
cargo run
```

`schema export` creates `migrations/schema/app_db.1.json`. The following `cargo build` embeds that snapshot for compile-time SQL validation and runtime schema validation. `cargo build` and `cargo test` never modify schema files automatically.

After changing an entity, increment the version or use `version = auto`, then run `schema export → build` again. If export creates a migration SQL draft, always review it before applying it.

See [Changing a schema](docs/USAGE-en.md#changing-a-schema) for the complete workflow and failure recovery.

## SELECT result structs without entities

Query results are not limited to whole-table `#[entity]` types. For view-like results such as joins, aggregates, and partial-column projections, implement `FromRow` on an ordinary struct.

```rust
use roomrs::dao;

#[derive(Debug)]
struct TodoListItem {
    todo_id: i64,
    title: String,
    owner_name: String,
}

impl roomrs::FromRow for TodoListItem {
    fn from_row(row: &roomrs::rusqlite::Row<'_>) -> roomrs::rusqlite::Result<Self> {
        Ok(Self {
            todo_id: row.get("todo_id")?,
            title: row.get("title")?,
            owner_name: row.get("owner_name")?,
        })
    }
}

#[dao]
trait TodoViewDao {
    #[query(
        "SELECT t.id AS todo_id, t.title, u.name AS owner_name
         FROM todos t
         JOIN users u ON u.id = t.owner_id
         ORDER BY t.id"
    )]
    fn list_items(&self) -> roomrs::Result<Vec<TodoListItem>>;
}
```

`TodoListItem` does not need `#[entity]` and must not be listed in `#[database(entities(...))]`. It therefore creates no table or schema object. Register only `TodoViewDao` in `daos(...)` when a generated DAO accessor is needed. The same struct works with `run_sync().query_all(...)`, direct asynchronous queries, and `UPDATE` or `DELETE ... RETURNING` results. See [arbitrary SELECT result structs](docs/USAGE-en.md#arbitrary-select-result-structs) for DAO and direct-query examples.

## Key features

| Feature | Description |
|---|---|
| Entity DSL | Single/composite PKs, DEFAULT, UNIQUE, CHECK, FKs, ordered/partial indexes, generated columns, custom SQL types, trigger files |
| DAO | `#[query]`, `#[insert]`, `#[update]`, `#[delete]`, `#[transaction]` |
| SELECT result structs | Map joins, aggregates, and projections into ordinary structs implementing `FromRow` |
| SQL validation | Snapshot-based table/column checks and `:name` parameter matching |
| LiveQuery | Refresh after commit, row filters, observer debounce, sync and Stream consumers |
| Transactions | DAO transactions, closure transactions, nested savepoints, RAII rollback |
| Relations | 1:1, 1:N, and N:M relation views with batched loading |
| Migrations | SQL and Rust code steps, embedded directories, safe diff execution |
| Async | `Future + Send` without a hard tokio dependency; works with tokio, smol, async-std, and others |
| Encrypted DB | Bundled or system SQLite and SQLCipher |

Android Room users can start with the [Room-to-roomrs mapping](docs/USAGE-en.md#mapping-from-android-room).

## Documentation

| Document | Contents |
|---|---|
| [Detailed usage guide](docs/USAGE-en.md) | Installation, CRUD, LiveQuery, transactions, relations, migrations, and operations |
| [API documentation](https://docs.rs/roomrs) | Public types and functions |
| [Examples](crates/roomrs/examples/) | Runnable sync, async, relation, and migration examples |
| [Roadmap](ROADMAP.md) | Public feature priorities and exclusions |
| [Changelog](CHANGELOG.md) | Changes by released version |
| [Development plan](roomrs-개발계획서.md) | Internal implementation contracts and design decisions |
| [Non-returning path policy](docs/RETURN_UNAVAILABLE.md) | Crash prevention for Drop, threads, and callbacks |

## Feature flags

Default configuration:

```toml
roomrs = "0.3.0"
```

The default features are `sqlite-bundled`, `async`, `tokio`, `live`, `time`, `uuid`, and `json`.

Minimal synchronous configuration:

```toml
roomrs = {
    version = "0.3.0",
    default-features = false,
    features = ["sqlite-bundled"]
}
```

SQLCipher:

```toml
roomrs = {
    version = "0.3.0",
    default-features = false,
    features = ["sqlcipher-bundled", "async", "tokio", "live", "time", "uuid", "json"]
}
```

Select exactly one of `sqlite-bundled`, `sqlite-system`, `sqlcipher-bundled`, and `sqlcipher-system`. Selecting SQLCipher does not encrypt a database unless you also configure `.encryption_key(...)`.

See [Installation and backend selection](docs/USAGE-en.md#installation-and-backend-selection) for every feature and Windows vcpkg setup.

## Current limitations

- SQLite only.
- LiveQuery does not observe writes made by other processes.
- Async transactions run synchronous closures on a worker; `.await` is not supported inside the closure.
- Automatic migration executes only forward operations classified as safe. Destructive changes and data transformations require manual migrations.
- After exporting a new schema snapshot, run `cargo build` again so the new file is embedded.

See [Known limitations](docs/USAGE-en.md#known-limitations) for exact boundaries and recommended alternatives.

## Running examples

```sh
cargo run -p roomrs --example todo_sync
cargo run -p roomrs --example todo_async
cargo run -p roomrs --example live_query
cargo run -p roomrs --example transactions
cargo run -p roomrs --example migrations
cargo run -p roomrs --example relations
```

The [example guide](docs/USAGE-en.md#runnable-examples) lists every example.

## Supported environments

- Rust 1.85 or newer, Edition 2024
- Windows, macOS, and Linux
- Android and iOS through a Rust `cdylib` FFI pattern
- A C compiler for the default bundled SQLite build
- A C compiler and Perl for bundled SQLCipher

CI covers Windows, macOS, Linux, MSRV, and backend-specific feature combinations. See [Platforms and cross-building](docs/USAGE-en.md#platforms-and-cross-building) for cross-build commands.

## Contributing

Please include the following in bug reports:

- roomrs and Rust versions
- enabled features
- OS and SQLite/SQLCipher linking mode
- a minimal reproducible example
- the actual error and expected behavior

See [Contributing and development](docs/USAGE-en.md#contributing-and-development) for development setup and verification gates. Check [ROADMAP.md](ROADMAP.md) and the [development plan](roomrs-개발계획서.md) before implementing a new feature.

## License

Licensed under either of:

- [Apache License 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)
