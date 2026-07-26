# roomrs

[한국어](README.md) | **English**

[![CI](https://github.com/yongaru/roomrs/actions/workflows/ci.yml/badge.svg)](https://github.com/yongaru/roomrs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/roomrs.svg)](https://crates.io/crates/roomrs)
[![docs.rs](https://img.shields.io/docsrs/roomrs)](https://docs.rs/roomrs)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-informational)](#platforms--msrv--cross-building)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

> A **local SQLite persistence** library for Rust that aims for the same developer experience as Android **Room**.

---

## Introduction

roomrs is a Rust library for working with local SQLite databases. If you have used Room on Android, it will feel immediately familiar — **entities are structs, DAOs are traits, SQL lives in macro strings**, and the macros generate the rest.

Why build it? The Rust ecosystem has excellent general-purpose ORMs (diesel, SeaORM), but nothing that delivers the whole Room experience in one package: **live queries** (subscriptions that automatically re-notify when data changes), **compile-time SQL validation**, and a single codebase that covers desktop and mobile. roomrs fills that gap. It does not try to be a general-purpose ORM — the goal is to do **one thing well: SQLite-only local persistence**.

## Quick start

```toml
[dependencies]
roomrs = "0.3.0"
```

To clone the repository and run a working example immediately:

```sh
cargo run -p roomrs --example todo_sync
```

The default build bundles SQLite, so no system SQLite installation is required. Copy your first entity, DAO, and database declaration from the [detailed example](#detailed-example) below.

A concept map for Room users:

| Room | roomrs |
|---|---|
| `@Entity` / `@PrimaryKey` / `@Ignore` | `#[entity]` / `#[pk]` / `#[column(ignore)]` |
| `@Dao` / `@Query` / `@Insert` | `#[dao]` / `#[query]` / `#[insert]` |
| `@Transaction` | `#[transaction]` |
| `@Database` | `#[database(entities(...), version = N)]` |
| `@Relation` / `@Embedded` | `#[relation]` / `#[embedded]` (parent marker for relation views — flattening entity columns is not supported yet, planned for v1.x) |
| `@TypeConverter` | rusqlite `ToSql`/`FromSql` delegation + `#[json]` |
| `Flow<List<T>>` | `LiveQuery<Vec<T>>` |
| `Migration(1, 2) { execSQL(...) }` | `Migration::sql(1, 2, "...")` |
| `fallbackToDestructiveMigration()` | `.fallback_to_destructive_migration(true)` |
| `suspend fun` | `async fn` on `db.run_async()` |
| KSP | proc-macro |

## Roadmap

Public feature priorities and out-of-scope work are listed in [ROADMAP.md](ROADMAP.md). The implementation contract lives in the [development specification](roomrs-개발계획서.md), while released changes are recorded in [CHANGELOG](CHANGELOG.md).

---

## Key features

- **Predictable SQLite concurrency — a unified read/write pool.** All N general-purpose connections can read and write, and a checkout guard gives one operation exclusive use of one connection. WAL and `busy_timeout` coordinate lock contention within and across processes; `SQLITE_BUSY` can still be returned when the timeout expires.
- **Live queries.** A query that returns `LiveQuery<T>` automatically re-runs and emits fresh results whenever a dependent table changes. Both synchronous consumption (`recv`/`iter`/`subscribe` callbacks) and asynchronous consumption (`Stream`) are supported.
- **Row-filtered invalidation.** `InvalidationFilter` compares preupdate-hook OLD/NEW rows so unrelated changes do not re-query a subscription. Table/column names are validated against the live SQLite schema at subscription time; typos surface as `Error::InvalidationFilter`. `watch_*_filtered` is available on sync, async, and DAO paths; **multiple filters are OR-matched** so any matching filter re-queries. Invalidations are emitted per commit/statement and coalesced with a DB-wide default **fixed** 250ms debounce window (`.live_debounce(Duration)`; per-observer override via `.debounce(Duration)`). Re-queries run on `roomrs-live-worker` threads that checkout the unified connection pool (`.notifier_readers(N)`, default `min(2, connections)`). `db.live_metrics()` exposes receive/coalesce/refresh counters (no SQL parameters or row payloads). It observes roomrs connections in one process only.
- **Compile-time SQL validation.** The SQL inside `#[query("...")]` is checked against a committed schema snapshot. Referencing a table or column that does not exist is a compile error. Parameter consistency (`:name` ↔ arguments) is also verified at compile time.
- **Versioned schema snapshots + binary-embedded auto-migration.** Each schema version is committed as a `[db_name].[version].json` file, and all versions are compressed and embedded into your binary. With `.auto_migrate(true)`, version gaps with no registered migration step are filled automatically from the embedded snapshot diff — but only with **safe operations** (CREATE TABLE, nullable ADD COLUMN, `NOT NULL DEFAULT` ADD COLUMN, CREATE INDEX, RENAME COLUMN from a valid rename hint). Destructive changes are never executed automatically; you get a clear error instead.
- **Runtime-agnostic async.** The async API returns plain std `Future`s (`+ Send`), so you can await them on tokio, async-std, smol, or `futures::executor` alike. The tokio-optimized integration is enabled by default and can be excluded with `default-features = false`.

Under the hood: a synchronous [rusqlite](https://github.com/rusqlite/rusqlite) core plus a purpose-built mini pool. Bundled SQLite is the default, while both SQLite and SQLCipher support bundled and system linkage. SQLite is synchronous at the C level, so any library's "async" is ultimately worker offloading — which is why roomrs is built as a **synchronous core with an async facade**.

---

## Detailed example

### Installation

```toml
[dependencies]
roomrs = "0.3.0"
```

> If it is not yet published on crates.io, you can use it as a git dependency:
> `roomrs = { git = "https://github.com/yongaru/roomrs" }`

SQLite is compiled in as a bundled build — no system SQLite installation required.

### Synchronous usage

```rust
use roomrs::{MigrationPolicy, dao, database, entity};

#[entity(table = "todos")]
#[derive(Debug, Clone)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert] // PK is omitted on insert — the new id comes back as the return value
    fn add(&self, t: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

fn main() -> roomrs::Result<()> {
    let db = Db::builder()
        .sqlite("todo.db")
        .migrate(MigrationPolicy::Auto)
        .build()?;
    let h = db.run_sync();

    let id = h.todo_dao().add(&Todo { id: 0, title: "read the spec".into(), done: false })?;
    println!("new id = {id}");
    for t in h.todo_dao().by_done(false)? {
        println!("- [{}] {}", t.id, t.title);
    }
    Ok(())
}
```

### PRIMARY KEY syntax

A PRIMARY KEY can be declared with field-level `#[pk]` attributes or once at the entity level. The entity-level list uses Rust field names, and its order defines the composite-key order.

```rust
#[entity(
    table = "payments",
    primary_key(store_id, payment_id)
)]
struct Payment {
    store_id: String,
    payment_id: String,
    amount: i64,
}
```

Both forms may be present, but their fields and order must match exactly. Otherwise, `cargo roomrs schema export` and `cargo roomrs schema check` return an error with corrective advice before writing files. Continue to place `#[pk(autoincrement)]` directly on its single integer field.

### Asynchronous usage

Keep the exact same entity/DAO declarations and just switch the handle to `run_async()`. The method names are identical.

```rust
use roomrs::{BuildAsyncExt, MigrationPolicy, dao, database, entity};

// ... the same #[entity] / #[dao] / #[database] declarations as above ...

fn main() -> roomrs::Result<()> {
    // Using smol here — tokio / async-std work exactly the same
    smol::block_on(async {
        let db = Db::builder()
            .sqlite("todo.db")
            .migrate(MigrationPolicy::Auto)
            .build_async()
            .await?;
        let h = db.run_async();

        let id = h.todo_dao().add(&Todo { id: 0, title: "async".into(), done: false }).await?;
        println!("new id = {id}");
        for t in h.todo_dao().by_done(false).await? {
            println!("- [{}] {}", t.id, t.title);
        }
        Ok(())
    })
}
```

The full set of examples lives in [crates/roomrs/examples/](crates/roomrs/examples/) — run them with e.g. `cargo run --example todo_sync`.

---

## Usage

### Live queries

Declare a return type of `LiveQuery<T>` and that query becomes a "subscription". You receive the current value immediately, and a re-queried result every time a dependent table is written to.

For direct SQL subscriptions, `InvalidationFilter` limits re-queries to relevant row changes. A single filter still works; pass `vec![f1, f2]` (or an array) for OR matching across filters. DAO live methods accept one or more `InvalidationFilter` arguments with the same rules. Predicates inside a group are ANDed and groups are ORed; filter-to-filter AND and nested boolean trees are not supported. Complex SQL conditions are not inferred automatically; use table-level subscriptions when the filter cannot express the condition. Under bursty writes, set a DB-wide delay with `.live_debounce(...)` or override one observer with `.debounce(...)` (default 250ms fixed window — extra invalidations inside the window merge without extending the deadline).

```rust
use roomrs::LiveQuery;

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, t: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT COUNT(*) FROM todos")]
    fn watch_count(&self) -> LiveQuery<i64>;
}

let live = db.run_sync().todo_dao().watch_count();

// Callback subscription — invoked on a live-query worker thread
let guard = live.subscribe(|n| println!("current todo count: {n}"));
// Dropping the guard ends the subscription — beware `let _ = ...`, which drops immediately

// Blocking receive is also available
// let first = live.recv()?;

// In async code, consume it as a Stream (feature `async`)
// let mut stream = live.into_stream();
```

Changes made inside a transaction are accumulated and only emitted **after the commit succeeds**. A rollback emits nothing. To subscribe without a DAO, use `db.run_sync().watch_all(...)` / `watch_optional(...)` / `watch_scalar(...)`.

### Transactions

Three forms are available.

**1) `#[transaction]` DAO methods** — the entire method body becomes one transaction. The important detail: `self.xxx()` calls inside the body are **rewritten by the macro into tx-bound DAO calls**, so they all use the same transaction connection (no pool re-acquisition → no self-lock contention or deadlocks). Keep in mind this rewrite applies only to `self` method calls inside the macro body.

```rust
#[dao]
trait AccountDao {
    #[query("SELECT balance FROM Account WHERE id = :id")]
    fn balance(&self, id: i64) -> roomrs::Result<i64>;

    #[update("UPDATE Account SET balance = balance + :delta WHERE id = :id")]
    fn adjust(&self, id: i64, delta: i64) -> roomrs::Result<u64>;

    /// Transfer — a failure midway rolls back everything
    #[transaction]
    fn transfer(&self, from: i64, to: i64, amount: i64) -> roomrs::Result<()> {
        if self.balance(from)? < amount {
            return Err(roomrs::Error::Config("insufficient balance".into()));
        }
        self.adjust(from, -amount)?;
        self.adjust(to, amount)?;
        Ok(())
    }
}
```

**2) Closure transactions** — nesting becomes a savepoint.

```rust
db.run_sync().transaction(|tx| {
    tx.account_dao().adjust(a, -10)?;
    // Nesting = SAVEPOINT — an inner failure rolls back only the inner part
    let inner: roomrs::Result<()> = roomrs::SqlContext::ctx_transaction(&&*tx, |sp| {
        sp.account_dao().adjust(b, 999)?;
        Err(roomrs::Error::Config("inner cancel".into()))
    });
    println!("savepoint result: {inner:?}");
    Ok(())
})?;
```

**3) RAII** — open with `begin()`; dropping without a commit rolls back.

```rust
{
    let tx = db.run_sync().begin()?;
    tx.execute("UPDATE Account SET balance = 0", roomrs::params![])?;
    // scope ends without commit → rollback
}
```

In async code, v1 supports **the synchronous-closure form only** (`db.run_async().transaction(|tx| { ... }).await`) — the whole closure runs on a worker while holding the same checked-out connection from `BEGIN IMMEDIATE` through commit or rollback. You cannot await inside the closure. Also, async `#[transaction]` methods accept **owned arguments only** due to the `'static` bound (borrowed arguments are a compile error).

### Migrations

Three paths can be combined; they all merge into a chain of `(from, to)` version pairs and run in per-step transactions.

```rust
use roomrs::Migration;

let db = AppDb::builder()
    .sqlite("app.db")
    // 1) Inline SQL step
    .migration(Migration::sql(
        1, 2,
        r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#,
    ))
    // 2) Code step — arbitrary logic allowed
    .migration(Migration::code(2, 3, |tx| {
        tx.execute_batch(r#"ALTER TABLE "docs" ADD COLUMN "done" INTEGER NOT NULL DEFAULT 0"#)
    }))
    .build()?;
```

The third path is the **auto-diff draft** — `cargo roomrs migrate diff` (or `roomrs::diff_sql`) compares two snapshots and produces a draft migration SQL. Drafts are for review, are never executed automatically, and destructive changes are marked with TODO comments. There is also `migrations_dir!("path")`, which embeds a directory of SQL files at compile time (using the `{from}_{to}_name.sql` convention).

**Binary-embedded auto-migration (opt-in)** — since every snapshot version is embedded in the binary, gaps with no registered step can be filled automatically:

```rust
let db = AppDb::builder()
    .sqlite("app.db")
    .auto_migrate(true) // fill unregistered gaps from the embedded snapshot diff
    .build()?;
```

Automatic execution is limited to **safe operations** (CREATE TABLE, nullable ADD COLUMN, `NOT NULL DEFAULT` ADD COLUMN, CREATE INDEX, RENAME COLUMN from a valid rename hint). A gap that requires DROP, a type change, a DEFAULT change, or a definition-breaking rename is never executed automatically — you get an error telling you which gap failed and why. If you register a manual step for that gap, the registered step always wins. As a last resort there is an opt-in fallback that drops everything and recreates it:

```rust
.fallback_to_destructive_migration(true) // drop + recreate when the chain is insufficient — data loss!
```

### Multi-process invalidation

This is not currently supported. Background and reconsideration criteria are kept in the [multi-process LiveQuery invalidation note](docs/MULTI_PROCESS_INVALIDATION.md).

### Type converters

Type conversion is delegated to rusqlite's `ToSql`/`FromSql`. Common types map straight to column types via features (`time`, `uuid`, `json` are on by default), and any serde type can be serialized into a TEXT column with `#[json]`.

```rust
#[entity(table = "profiles")]
struct Profile {
    #[pk(autoincrement)]
    id: i64,
    created_at: time::OffsetDateTime, // feature `time` — stored as TEXT
    token: uuid::Uuid,                // feature `uuid` — stored as BLOB
    #[json]
    prefs: Prefs,                     // serde-serialized — stored as TEXT (needs Serialize + Deserialize)
    #[column(ignore)]
    cache: Option<String>,            // excluded from the table
}
```

### More examples

| Example | Use case |
|---|---|
| `todo_sync` / `todo_async` | Basic CRUD — sync / async (runtime-agnostic) |
| `transactions` | `#[transaction]` transfer · nested savepoints · RAII rollback |
| `migrations` | Version chain (1→2→3) · SQL/code steps · diff draft |
| `relations` | 1:N / 1:1 / N:M — `with_relations` avoiding N+1 |
| `query_builder` | Dynamic condition building · schema validation · sync/async handle symmetry |
| `live_query` | `LiveQuery` subscription callback + tracing log bridge |
| `pagination` | Page moves via `rebind` + automatic refresh on writes |
| `bench` | Simple throughput measurement (`--release`) |

For the mobile FFI pattern (cdylib, `extern "C"`) and its stable negative error-code contract, see [examples/mobile-ffi/](examples/mobile-ffi/).

---

## Feature flags

| feature | default | description |
|---|---|---|
| `sqlite-bundled` | on | Compile SQLite source as part of the Cargo build |
| `sqlite-system` | off | Link an OS package or vcpkg SQLite library |
| `sqlcipher-bundled` | off | Build SQLCipher with vendored OpenSSL through Cargo |
| `sqlcipher-system` | off | Link an OS package or vcpkg SQLCipher library |
| `bundled` | off | Backward-compatible alias for `sqlite-bundled` |
| `cipher` | off | Backward-compatible alias for `sqlcipher-bundled` |
| `async` | on | Runtime-agnostic async facade. Turn off for pure sync |
| `tokio` | on | tokio-optimized integration (implies `async`) — falls back to the built-in worker pool outside a tokio runtime |
| `live` | on | Live queries / invalidation |
| `time`, `uuid`, `json` | on | Type converters |

A minimal pure-sync build:

```toml
roomrs = { version = "0.3.0", default-features = false, features = ["sqlite-bundled"] }
```

The complete default set is `sqlite-bundled`, `async`, `tokio`, `live`, `time`, `uuid`, and `json`. A default build compiles SQLite from source and does not depend on OpenSSL. Choose SQLCipher explicitly with `default-features = false` plus `sqlcipher-bundled` or `sqlcipher-system`. Selecting a SQLCipher backend does not encrypt a database automatically; encryption starts only when `.encryption_key(...)` is configured.

No two canonical backend features may be selected together. A backend-free `roomrs-core` build is allowed, but an application that uses a database must select exactly one. Existing `bundled` and `cipher` users continue to use the same bundled backends.

### Windows system backends with vcpkg

System backends link an installed library instead of rebuilding the SQLite/SQLCipher C source through Cargo. Because `live` uses the preupdate hook, SQLite requires vcpkg's `session` feature.

```powershell
$env:VCPKG_ROOT = "D:\tools\vcpkg"
$env:VCPKGRS_TRIPLET = "x64-windows-static-release"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
& "$env:VCPKG_ROOT\vcpkg.exe" install "sqlite3[session]:x64-windows-static-release"
```

```toml
roomrs = { version = "0.3.0", default-features = false, features = ["sqlite-system", "async", "live", "time", "uuid", "json"] }
```

The official SQLCipher port does not enable the preupdate hook. The repository's
`vcpkg/build-sqlcipher-system.cmd` uses the adjacent `vcpkg/ports/sqlcipher` overlay,
installs SQLCipher for `x64-windows-static-release`, and verifies `SQLITE_ENABLE_PREUPDATE_HOOK`.
Set `VCPKG_ROOT` or add `vcpkg.exe` to PATH, then run it from the repository root.
It installs every feature exposed by the overlay (`fts5`, `geopoly`, `json1`, and `tool`).

```powershell
$env:VCPKG_ROOT = "D:\tools\vcpkg"
$env:VCPKGRS_TRIPLET = "x64-windows-static-release"
$env:SQLCIPHER_STATIC = "1"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
vcpkg\build-sqlcipher-system.cmd
```

You may also run it from inside the `vcpkg` directory:

```powershell
cd vcpkg
.\build-sqlcipher-system.cmd
```

Pass another static triplet as the first argument when needed:

```powershell
vcpkg\build-sqlcipher-system.cmd x64-windows-static
```

```toml
roomrs = { version = "0.3.0", default-features = false, features = ["sqlcipher-system", "async", "live", "time", "uuid", "json"] }
```

These commands use the `x64-windows-static-release` community triplet: release-only static libraries with the static CRT. Rust must use the same static CRT through `RUSTFLAGS=-C target-feature=+crt-static`; do not set `VCPKGRS_DYNAMIC`. No SQLite or SQLCipher DLL needs to be deployed. If you separately choose a dynamic-library triplet, deploying its DLLs is the application's responsibility.

vcpkg compiles the C/C++ libraries on the first install and Cargo only links the resulting `.lib` files afterward. Therefore, `cargo clean` neither removes vcpkg artifacts nor rebuilds SQLite/SQLCipher. Set `VCPKG_DEFAULT_BINARY_CACHE` to a shared location to reuse the same ABI result across checkouts and CI. With a system backend, the linked library version, compile options, ABI, license acceptance, and deployment model are application acceptance criteria.

---

## Schema snapshot workflow

The source of truth for compile-time SQL validation and auto-migration is the **schema snapshot files committed to your repository**.

- Location and naming: `migrations/schema/[db_name].[version].json` — where db_name is the snake_case of your `#[database]` struct name (e.g. `AppDb` at v3 → `app_db.3.json`). The directory can be overridden with the `ROOMRS_SCHEMA_DIR` environment variable.
- **Generation is explicit.** `cargo test` and `cargo build` never create or modify snapshot or migration files. Only `cargo roomrs schema export` (or `export_schema_snapshot` / `run_registered_schema_export`) creates the current-version snapshot. If a same-version file already exists and differs from the code, it is **not overwritten** and guidance asks you to bump `version`. After export, run `cargo build` so macros re-embed snapshots.
- `cargo roomrs schema export/check` compiles package lib and regular binary targets through a custom-cfg harness to discover `#[database]`. Apps with only `src/main.rs` are supported without modifying consumer source. Ordinary `cargo test` does not include this entry point.
- When a snapshot is updated, the macros re-expand (file dependencies are registered via `include_bytes!`) and all versions are compressed and embedded into the binary. `build()` compares the embedded snapshot hash against the runtime entity-metadata hash and returns a clear error if they are stale.
- **`build()` fails by default when the current-version snapshot is missing.** If the macro expanded without that file, startup returns `SnapshotStale` and asks you to run `export_schema_snapshot` and rebuild. Only special cases such as this repository’s integration tests set `ROOMRS_ALLOW_MISSING_SNAPSHOT=1`. Static SQL parameter checks always run regardless of snapshots.

Snapshots can be managed from the CLI:

```
cargo roomrs schema export                              # create snapshots/safe SQL drafts for every #[database]
cargo roomrs schema check                               # read-only verification of every #[database]
cargo roomrs migrate diff <old.json> <new.json> [out.sql]   # generate a draft migration SQL
cargo roomrs migrate check <a.json> <b.json>                # compare snapshot hashes (for CI)
cargo roomrs migrate check-dir <schema_dir> <db_name>       # scan version files — parse/consistency checks, destructive-change warnings
```

---

## Architecture

```
roomrs/
├─ crates/
│  ├─ roomrs/          # facade — re-exports the public API only
│  ├─ roomrs-core/     # Database · built-in unified read/write pool · errors ·
│  │                   #   invalidation tracker · notifier · migration runner · SQL/DDL render · hooks
│  ├─ roomrs-async/    # async facade — runtime-agnostic Future/Stream + optional tokio integration
│  ├─ roomrs-macros/   # proc-macros: #[entity] #[dao] #[database] ...
│  ├─ roomrs-migrate/  # SchemaSnapshot · diff · compression · codegen (shared by macros and runtime)
│  └─ roomrs-cli/      # cargo roomrs schema / migrate
├─ examples/           # mobile-ffi and friends
└─ xtask/              # cross-build tasks
```

Dependencies flow in one direction only: `roomrs → {core, async, macros}`, `macros → migrate`, `async → core`, `core → migrate`. The snapshot model (`roomrs-migrate`) is shared between the macros (compile time) and the runtime, which is what lets compile-time validation and runtime staleness detection operate on the same types.

The heart of the concurrency model is a unified read/write mini pool of **N general-purpose connections**. Every general-purpose connection can read and write, and a checkout guard gives one operation exclusive use of one connection. `query` and `execute` differ by whether the caller consumes result rows, not by connection permissions, so `INSERT ... RETURNING`, CTEs, and writable PRAGMAs run without SQL routing. A transaction keeps the same checked-out connection and starts with `BEGIN IMMEDIATE`. WAL and `busy_timeout` coordinate lock contention. The primary invalidation path is statement-based (determine the target tables of each executed SQL statement → emit after a successful commit), with `preupdate_hook` installed on every general-purpose connection as a secondary path for indirect trigger writes and row-filter matching.

---

## Logging

The roomrs libraries emit English messages exclusively through the [`log`](https://crates.io/crates/log) facade and never initialize a subscriber. `error` reports operation failures and fatal pool states; `warn` reports recovery, retries, and destructive changes; `info` covers database, pool, notifier, and snapshot lifecycles; `debug` covers migration plans, checkout waits, and subscriptions; and `trace` covers SQL structure, checkouts, async jobs, and invalidation details. SQL parameters, encryption keys, and row data are never logged.

The CLI and every runnable example in this repository initialize a `tracing-log` bridge and `tracing-subscriber`. Control verbosity with `RUST_LOG`:

```powershell
$env:RUST_LOG = "info,roomrs_core=trace,roomrs_async=debug,roomrs_migrate=debug"
cargo run --example live_query
```

Install the same bridge in a consumer application that uses tracing:

```rust
/// Initialize the log → tracing bridge —
/// roomrs only emits via the log facade (installing a subscriber is the consumer's job).
fn init_tracing() {
    // 1) Install the log → tracing converter (global log logger)
    tracing_log::LogTracer::init().expect("failed to init LogTracer");
    // 2) Install the fmt subscriber with a RUST_LOG filter —
    //    fmt().init() would try to install LogTracer again and fail, so use set_global_default
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("roomrs_core=debug"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("failed to set tracing subscriber");
}
```

With the debug level enabled you can see connection opens, transaction begin/commit/rollback, invalidation events, and other internals. See it in action with `cargo run --example live_query`.

---

## Platforms · MSRV · Cross-building

- **MSRV 1.85** (Edition 2024) — included in the CI matrix.
- Tested on all three desktop OSes (Windows/macOS/Linux); mobile (Android/iOS) is supported through the FFI pattern.
- Default bundled SQLite needs no system library installation beyond a C compiler. Bundled SQLCipher additionally requires Perl; system backends require the OS package or vcpkg setup above.

Cross-building from a Windows host uses zig/NDK. The default configuration compiles bundled SQLite (C) along with it. Selecting SQLCipher additionally compiles OpenSSL (C).

```
cargo xtask cross-linux      # x86_64/aarch64-linux-gnu + x86_64-musl (static CLI) — zig
cargo xtask cross-android    # arm64-v8a / armeabi-v7a / x86_64 .so — cargo-ndk
cargo xtask cross-all
```

| Target | Tooling | Status |
|---|---|---|
| Windows x64 (host) | MSVC | ✅ build + test |
| Linux x64 / arm64 (gnu) | zig (cargo-zigbuild) | ✅ build |
| Linux x64 (musl, static) | zig | ✅ build |
| Android arm64 / armv7 / x86_64 | NDK (cargo-ndk) | ✅ build (.so) |
| iOS / macOS | requires Xcode | ⬜ follow-up on a macOS host |

For tool installation and constraints (Android cannot be built with zig alone — bionic ships only with the NDK), see [docs/cross-build.md](docs/cross-build.md).

---

## Contributing

Contributions are welcome! Bug reports, documentation improvements, and feature proposals are all appreciated.

### Development setup

All you need is Rust 1.85 or newer. SQLite is compiled in as a bundled build, so nothing else needs to be installed.

```
git clone https://github.com/yongaru/roomrs
cd roomrs
cargo test --workspace
```

### Pre-PR checks (the same gates as CI)

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features tokio -- -D warnings
cargo test --workspace
cargo test --workspace --features tokio
```

### Conventions

- **The spec is the single source of truth (SSOT).** Any change affecting the public API must first be reconciled with [roomrs-개발계획서.md](roomrs-개발계획서.md) (especially the §0 decision log). If the spec and the code conflict, updating the spec comes first.
- Code comments and error messages are written in **Korean**; public rustdoc (`///`) is written in **English** (for crates.io).
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/), with the crate name as the scope (e.g. `feat(core): ...`, `fix(macros): ...`).
- Behavior changes must come with tests. Macro compile-failure cases are covered with trybuild (`tests/ui/`).
- Use `:memory:` or tempfile for test databases — **never create `.db` files inside the repository.**


### Reporting issues

When you find a bug, please include reproduction steps, the roomrs version, your OS/target, and relevant logs (debug level if possible). A minimal reproducible example is best of all.

---

## License

Licensed under either of the following, at your option:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
