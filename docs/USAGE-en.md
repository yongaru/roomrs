# roomrs Detailed Usage Guide

[Back to README](../README-en.md) | [한국어](USAGE.md)

This guide follows the actual workflow from first installation through schema changes and deployment. Use [docs.rs](https://docs.rs/roomrs) for the public API reference and the [examples directory](../crates/roomrs/examples/) for runnable code.

## Quick navigation

| Goal | Section |
|---|---|
| Install and run for the first time | [Creating your first database](#creating-your-first-database) |
| Configure entities and columns | [Defining entities](#defining-entities) |
| Create CRUD DAOs | [Defining DAOs](#defining-daos) |
| Execute a SQL string directly | [Direct SQL string queries](#direct-sql-string-queries) |
| Call APIs asynchronously | [Asynchronous API](#asynchronous-api) |
| Observe data changes | [LiveQuery](#livequery) |
| Perform atomic work | [Transactions](#transactions) |
| Load related data | [Relation mapping](#relation-mapping) |
| Add or rename columns | [Changing a schema](#changing-a-schema) |
| Upgrade existing user databases | [Migrations](#migrations) |
| Use SQLCipher instead of SQLite | [Installation and backend selection](#installation-and-backend-selection) |
| Diagnose errors | [Error handling and troubleshooting](#error-handling-and-troubleshooting) |

## Core workflow

A roomrs project repeats these four steps:

1. Declare `#[entity]`, `#[dao]`, and `#[database]` in Rust.
2. Run `cargo roomrs schema export` to create the current schema snapshot and, when it can be generated safely, a migration SQL draft.
3. Review the generated files, then run `cargo build` to embed snapshots.
4. When the application starts, create a new database or execute registered forward migrations.

```text
change entities
  → cargo roomrs schema export
  → review and commit JSON/SQL drafts
  → cargo build
  → deploy the application
  → run forward migration when opening the DB
```

`cargo build` and `cargo test` never create or modify snapshot or migration files. Only the explicit `cargo roomrs schema export` command writes them.

## Installation and backend selection

### Default installation

```toml
[dependencies]
roomrs = "0.4.0"
```

```sh
cargo install roomrs-cli
```

`roomrs-cli` installs one executable named `cargo-roomrs`. Invoke it through Cargo:

```sh
cargo roomrs --help
```

Default features:

| Feature | Default | Purpose |
|---|---:|---|
| `sqlite-bundled` | yes | Compile SQLite from bundled source |
| `async` | yes | Runtime-independent async API |
| `tokio` | yes | Efficient worker execution inside tokio |
| `live` | yes | LiveQuery and change invalidation |
| `time` | yes | `time` type mappings |
| `uuid` | yes | `Uuid` type mapping |
| `json` | yes | `#[json]` serialization |

### Minimal synchronous configuration

```toml
[dependencies]
roomrs = {
    version = "0.4.0",
    default-features = false,
    features = ["sqlite-bundled"]
}
```

### Selecting a backend

The following four features are mutually exclusive. Select exactly one for an application.

| Backend | Purpose |
|---|---|
| `sqlite-bundled` | Bundle SQLite in the Cargo build; the default |
| `sqlite-system` | Link an OS package or vcpkg SQLite |
| `sqlcipher-bundled` | Build SQLCipher and vendored OpenSSL through Cargo |
| `sqlcipher-system` | Link an OS package or vcpkg SQLCipher |

Bundled SQLCipher example:

```toml
roomrs = {
    version = "0.4.0",
    default-features = false,
    features = [
        "sqlcipher-bundled",
        "async",
        "tokio",
        "live",
        "time",
        "uuid",
        "json",
    ]
}
```

```rust
let db = AppDb::builder()
    .sqlite("secure.db")
    .encryption_key(secret)
    .build()?;
```

Selecting a SQLCipher backend does not automatically encrypt the database. Every connection must receive the same `.encryption_key(...)` before accessing the database. Never put the key in logs or source control.

For a Windows system backend, the repository provides [`vcpkg/build-sqlcipher-system.cmd`](../vcpkg/build-sqlcipher-system.cmd) and an overlay port. The `live` feature requires SQLite or SQLCipher built with `SQLITE_ENABLE_PREUPDATE_HOOK`.

## Creating your first database

### 1. Create a project

```sh
cargo new roomrs-example
cd roomrs-example
cargo add roomrs@0.4.0
cargo install roomrs-cli
```

### 2. Declare an entity

```rust
use roomrs::entity;

#[entity(table = "todos")]
#[derive(Debug)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}
```

`Todo` is the Rust value and `todos` is the SQLite table. A plain `#[insert]` excludes an `#[pk(autoincrement)]` field and lets SQLite create the rowid. The `id` value used to construct the Rust struct is ignored by this insert path. The value `0` has no special sentinel meaning.

### 3. Declare a DAO

```rust
use roomrs::dao;

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, todo: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;

    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Option<Todo>>;

    #[update("UPDATE todos SET done = :done WHERE id = :id")]
    fn set_done(&self, id: i64, done: bool) -> roomrs::Result<u64>;

    #[delete("DELETE FROM todos WHERE id = :id")]
    fn remove(&self, id: i64) -> roomrs::Result<u64>;
}
```

Every `:name` in `#[query]`, `#[update]`, and `#[delete]` must match a method argument. Once a snapshot exists, tables and columns are also checked at compile time.

### 4. Declare the database

```rust
use roomrs::database;

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct AppDb;
```

- `entities(...)`: entities managed by this database
- `daos(...)`: generated DAO accessors
- `version = 1`: current schema revision
- `version = auto`: let export compare the latest snapshot and select a revision

The struct name `AppDb` becomes the snapshot prefix `app_db`. Every `#[database]` in a crate must remain unique after snake_case conversion.

### 5. Export the snapshot

```sh
cargo roomrs schema export
cargo build
```

The first command creates `migrations/schema/app_db.1.json`. The second command reads the new file and embeds it. Commit snapshot files with the source code.

For read-only verification:

```sh
cargo roomrs schema check
```

### 6. Open the DB and call a DAO

```rust
fn main() -> roomrs::Result<()> {
    let db = AppDb::builder().sqlite("app.db").build()?;
    let handle = db.run_sync();
    let dao = handle.todo_dao();

    let id = dao.add(&Todo {
        id: 0,
        title: "Read the guide".into(),
        done: false,
    })?;

    dao.set_done(id, true)?;

    for todo in dao.by_done(true)? {
        println!("{todo:?}");
    }
    Ok(())
}
```

For a new file, roomrs creates the current DDL in one transaction and records `PRAGMA user_version`. For an existing file, it checks the version and registered migration chain.

Tests can use an in-memory database:

```rust
let db = AppDb::builder().in_memory().build()?;
```

Because of SQLite locking behavior, an in-memory database uses one unified connection.

## Mapping from Android Room

| Android Room | roomrs |
|---|---|
| `@Entity` | `#[entity]` |
| `@PrimaryKey` | `#[pk]`, `#[entity(primary_key(...))]` |
| `@Ignore` | `#[column(ignore)]` |
| `@Dao` | `#[dao]` |
| `@Query` | `#[query("...")]` |
| `@Insert` | `#[insert]` |
| `@Update`, `@Delete` | SQL-bearing `#[update("...")]`, `#[delete("...")]` |
| `@Transaction` | `#[transaction]` |
| `@Database(version = N)` | `#[database(..., version = N)]` |
| auto migration spec | snapshot diff + `.auto_migrate(true)` |
| `Migration(from, to)` | `Migration::sql`, `Migration::code` |
| `fallbackToDestructiveMigration()` | `.fallback_to_destructive_migration(true)` |
| `Flow<T>` | `LiveQuery<T>` |
| `@Relation`, `@Embedded` | `#[relation]`, `#[embedded]` |
| TypeConverter | rusqlite `ToSql`/`FromSql`, `#[json]` |
| KSP | proc-macro |

`#[embedded]` currently marks the parent field of a relation view. Entity column flattening is not supported yet.

## Defining entities

### Rust and SQLite types

| Rust type | SQLite declaration |
|---|---|
| Integers and `bool` | `INTEGER NOT NULL` |
| `f32`, `f64` | `REAL NOT NULL` |
| `String` | `TEXT NOT NULL` |
| `Vec<u8>` | `BLOB NOT NULL` |
| `Option<T>` | Nullable column of the inner type |
| `time` date/time types | `TEXT` |
| `uuid::Uuid` | `BLOB` |
| `#[json] T` | JSON `TEXT` |

Unknown custom types delegate to rusqlite `ToSql` and `FromSql`. Use `sql_type` to specify the DDL type:

```rust
#[column(sql_type = "DECIMAL(12,2)")]
amount: i64,
```

This changes SQLite storage affinity and the DDL declaration. Rust value conversion is still handled by the field type's `ToSql` and `FromSql` implementations.

### Column attributes

```rust
#[entity(table = "profiles")]
struct Profile {
    #[pk(autoincrement)]
    id: i64,

    #[column(name = "display_name", unique, index, collate = "NOCASE")]
    name: String,

    #[column(default = "active")]
    status: String,

    #[column(renamed_from = "old_note")]
    note: Option<String>,

    #[json]
    preferences: Preferences,

    #[column(ignore)]
    cached_label: Option<String>,
}
```

Supported field attributes:

| Attribute | Meaning |
|---|---|
| `#[pk]` | PRIMARY KEY member |
| `#[pk(autoincrement)]` | Single integer auto-increment PK |
| `#[column(name = "...")]` | Override the SQL column name |
| `#[column(unique)]` | Single-column UNIQUE |
| `#[column(index)]` | Single-column ordinary index |
| `#[column(default = "...")]` | SQLite DEFAULT |
| `#[column(ignore)]` | Exclude the field from the table |
| `#[column(renamed_from = "...")]` | Connect to a previous snapshot column name |
| `#[column(sql_type = "...")]` | Custom SQL column type |
| `#[column(collate = "...")]` | BINARY, NOCASE, RTRIM, or a custom collation |
| `#[column(generated = "...")]` | VIRTUAL generated column |
| `#[column(generated = "...", stored)]` | STORED generated column |
| `#[json]` | Store serde JSON as TEXT |

`default` distinguishes numbers, `true`/`false`, `now`/`CURRENT_TIMESTAMP`, parenthesized SQL expressions, and strings. Changing a DEFAULT can change existing data semantics and is not an automatic migration.

### Single and composite PRIMARY KEY

Field syntax:

```rust
#[entity(table = "payments")]
struct Payment {
    #[pk]
    store_id: String,
    #[pk]
    payment_id: String,
    amount: i64,
}
```

Entity syntax:

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

Composite key order follows field declaration order. If both forms are present, their lists and order must match exactly; otherwise schema export/check fails before writing files.

`AUTOINCREMENT` is valid only for a single `INTEGER PRIMARY KEY`.

### Table constraints and indexes

```rust
#[entity(
    table = "payments",
    primary_key(store_id, payment_id),
    unique(store_id, external_id),
    check = "amount >= 0",
    foreign_key(
        columns(store_id, customer_id),
        references = "customers(store_id, customer_id)",
        on_delete = "CASCADE",
        on_update = "NO ACTION"
    ),
    index(
        name = "idx_payment_created",
        columns(store_id, created_at desc)
    ),
    index(
        name = "idx_payment_active",
        columns(store_id),
        where = "deleted_at IS NULL"
    )
)]
struct Payment {
    store_id: String,
    payment_id: String,
    customer_id: String,
    external_id: String,
    amount: i64,
    created_at: String,
    deleted_at: Option<String>,
}
```

Index columns accept `asc`, `desc`, and `collate name`. A `where` clause creates a partial index. roomrs creates SQLite B-tree indexes and does not expose a separate index algorithm option.

### STRICT, WITHOUT ROWID, and generated columns

```rust
#[entity(
    table = "line_items",
    strict,
    without_rowid,
    index(name = "idx_item_name", columns(name collate nocase))
)]
struct LineItem {
    #[pk]
    id: String,
    name: String,
    price: i64,
    quantity: i64,
    #[column(generated = "price * quantity", stored)]
    total: i64,
}
```

A generated column cannot also have a default, be a PK, or use autoincrement. Changing generated definitions, STRICT, or WITHOUT ROWID on an existing table normally requires a table rebuild and therefore a manual migration.

### Database-level triggers

```rust
#[database(
    entities(Note, NoteAudit),
    version = auto,
    trigger(
        name = "trg_note_audit",
        sql = "CREATE TRIGGER trg_note_audit AFTER INSERT ON notes BEGIN INSERT INTO note_audit(note_id) VALUES (NEW.id); END"
    ),
    trigger(
        name = "trg_note_cleanup",
        file = "migrations/triggers/note_cleanup.sql"
    )
)]
struct AppDb;
```

A trigger is a database schema object rather than an entity attribute. Each declaration requires `name` and exactly one of `sql` or `file`. File paths are relative to the package's `CARGO_MANIFEST_DIR`. The SQL must contain exactly one non-TEMP `CREATE TRIGGER` statement, and its name must match the declared name. TEMP triggers are connection-local and therefore unsupported.

The name, complete SQL, and declaration source are included in the snapshot and hash. A new database creates triggers after tables and indexes. Trigger additions and removals become `CREATE TRIGGER` and `DROP TRIGGER`; definition changes become drop-and-recreate safe forward migrations.

## Defining DAOs

### Return shapes

| DAO return type | Behavior |
|---|---|
| `Result<Vec<T>>` | All rows |
| `Result<Option<T>>` | Zero or one row |
| `Result<T>` | Exactly one row; otherwise `Error::NotFound` |
| `Result<u64>` | Number of affected rows |
| `Result<i64>` on `#[insert]` | New rowid |
| `LiveQuery<Vec<T>>` | Observable list |
| `LiveQuery<Option<T>>` | Observable optional value |
| `LiveQuery<T>` | Observable scalar/single value |

### query, update, and delete

```rust
#[dao]
trait TodoDao {
    #[query("SELECT * FROM todos ORDER BY id")]
    fn all(&self) -> roomrs::Result<Vec<Todo>>;

    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Option<Todo>>;

    #[query("SELECT COUNT(*) FROM todos")]
    fn count(&self) -> roomrs::Result<i64>;

    #[update("UPDATE todos SET title = :title WHERE id = :id")]
    fn rename(&self, id: i64, title: String) -> roomrs::Result<u64>;

    #[delete("DELETE FROM todos WHERE id = :id")]
    fn remove(&self, id: i64) -> roomrs::Result<u64>;
}
```

SQL is parsed with the SQLite dialect. Named parameters connect by argument name, independent of their order in the SQL string.

Use `unchecked` only for special SQL that static schema validation cannot understand:

```rust
#[query(unchecked, "SELECT custom_runtime_function(value) FROM items")]
fn custom(&self) -> roomrs::Result<Vec<String>>;
```

`unchecked` still validates SQL parameter names against method arguments. Avoid it for ordinary queries because it moves column typo detection to runtime.

`#[update]` and `#[delete]` normally return an affected-row count as `Result<u64>`. When the SQL contains `RETURNING`, they may return `Result<T>`, `Result<Option<T>>`, or `Result<Vec<T>>`; `T` follows the same `FromRow` rules described below.

### Direct SQL string queries

Queries that do not need a fixed DAO method can run directly on a synchronous or asynchronous handle. An `#[entity]` type already has an automatically generated `FromRow` implementation, so no separate mapping code is required.

```rust
let handle = db.run_sync();

let todo: Todo = handle.query_one::<Todo, _>(
    "SELECT id, title, done FROM todos WHERE id = ?1",
    roomrs::params![1_i64],
)?;

let maybe_todo: Option<Todo> = handle.query_optional::<Todo, _>(
    "SELECT id, title, done FROM todos WHERE id = ?1",
    roomrs::params![-1_i64],
)?;

let todos: Vec<Todo> = handle.query_all::<Todo, _>(
    "SELECT id, title, done
     FROM todos
     WHERE done = ?1
     ORDER BY id",
    roomrs::params![false],
)?;

let count: i64 = handle.query_scalar::<i64, _>("SELECT COUNT(*) FROM todos", ())?;

let updated: Todo = handle.query_one::<Todo, _>(
    "UPDATE todos SET title = ?1 WHERE id = ?2
     RETURNING id, title, done",
    roomrs::params!["Read the manual", 1_i64],
)?;

let changed: u64 = handle.execute(
    "UPDATE todos SET done = ?1 WHERE id = ?2",
    roomrs::params![true, 1_i64],
)?;
```

| Method | Return type | Purpose |
|---|---|---|
| `query_one::<T, _>` | `T` | One row from SELECT or `... RETURNING`; returns `roomrs::Error::NotFound` when absent |
| `query_optional::<T, _>` | `Option<T>` | Zero or one row |
| `query_all::<T, _>` | `Vec<T>` | All rows |
| `query_scalar::<T, _>` | `T` | One column from the first row |
| `execute` | `u64` | Number of rows changed by INSERT, UPDATE, DELETE, and similar statements |

There is no `Row<T>` wrapper. In `::<Todo, _>`, `_` lets Rust infer the type of `params![...]`. Pass `()` when the SQL has no parameters. `T` must implement `FromSql` for `query_scalar` and `FromRow` for the other query methods.

On an asynchronous handle, the SQL string type is the first generic argument, so the turbofish order is `::<_, Todo, _>`.

```rust
let todo: Todo = db
    .run_async()
    .query_one::<_, Todo, _>(
        "SELECT id, title, done FROM todos WHERE id = ?1",
        roomrs::params![1_i64],
    )
    .await?;
```

Direct SQL strings do not receive the DAO macro's snapshot-based compile-time SQL validation, so prefer a `#[query]` DAO method for fixed SQL used repeatedly.

Use `query_one`, `query_optional`, or `query_all` for `INSERT`, `UPDATE`, or `DELETE ... RETURNING`, depending on the result cardinality. Use `execute` for writes without `RETURNING`.

The internal `roomrs::rusqlite::Row<'_>` used for mapping is not a `serde_json::Value`. It borrows the current row from a SQLite statement, and `FromRow` converts its columns into the Rust result type. The actual return value of `query_one::<Todo, _>` is `Todo`, not `Row`.

### Arbitrary SELECT result structs

A query result type does not need to be an `#[entity]`. For joins, aggregates, and aliased projections that do not match a standalone table, implement `FromRow` directly on an ordinary struct.

The following query joins existing `todos` and `users` entities but returns a screen-specific struct rather than another table.

```rust
use roomrs::dao;

#[derive(Debug, Clone)]
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
         WHERE t.done = :done
         ORDER BY t.id"
    )]
    fn list_items(&self, done: bool) -> roomrs::Result<Vec<TodoListItem>>;
}
```

Do not add `#[entity]` to this struct or list it in `#[database(entities(...))]`. It is therefore not part of snapshots, DDL, or migrations. Register only `TodoViewDao` in `#[database(..., daos(...))]` when the generated `todo_view_dao()` accessor is needed. The actual tables and columns referenced by the SQL are still checked against registered entity snapshots. SELECT aliases must match the names passed to `row.get(...)`.

The same type also works without a DAO:

```rust
let handle = db.run_sync();
let items: Vec<TodoListItem> = handle.query_all::<TodoListItem, _>(
    "SELECT t.id AS todo_id, t.title, u.name AS owner_name
     FROM todos t
     JOIN users u ON u.id = t.owner_id
     WHERE t.done = ?1
     ORDER BY t.id",
    roomrs::params![false],
)?;
```

Map `RETURNING` results in the same way:

```rust
#[update(
    "UPDATE todos SET title = :title
     WHERE id = :id
     RETURNING id AS todo_id, title, '' AS owner_name"
)]
fn rename_returning(
    &self,
    id: i64,
    title: String,
) -> roomrs::Result<TodoListItem>;
```

Synchronous direct queries provide `query_one`, `query_optional`, and `query_all`. The corresponding asynchronous handle methods are awaited and require the result struct to be `Send + 'static`. Using it as a `LiveQuery` result additionally requires `Clone + Send + 'static`.

### insert

```rust
#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, todo: &Todo) -> roomrs::Result<i64>;

    #[insert(keep_pk)]
    fn import(&self, todo: &Todo) -> roomrs::Result<i64>;

    #[insert(on_conflict = "replace")]
    fn replace(&self, todo: &Todo) -> roomrs::Result<i64>;
}
```

- Plain `#[insert]` excludes an autoincrement PK.
- `keep_pk` includes PK columns in the INSERT.
- `on_conflict` accepts `replace`, `abort`, `rollback`, and `fail`.
- The parser also recognizes `ignore`, but it is incompatible with the mandatory `Result<i64>` rowid contract of `#[insert]` and is rejected at compile time.
- `#[insert]` always returns a new rowid as `Result<i64>`. For an operation such as `INSERT OR IGNORE` that may succeed with zero rows, use an explicit SQL DAO method returning an affected-row count.

## Dynamic Query Builder

Use `Query` when search conditions vary at runtime.

```rust
use roomrs::{Order, Query, col};

let mut query = Query::select::<Product>();

if let Some(keyword) = keyword {
    query = query.and_where(
        col("name").like(format!("%{keyword}%")),
    );
}
if let Some(max_price) = max_price {
    query = query.and_where(col("price").le(max_price));
}
if !categories.is_empty() {
    query = query.and_where(
        col("category").in_list(categories),
    );
}

let query = query
    .order_by("price", Order::Asc)
    .limit(20);

let products = query.clone().fetch_all(db.run_sync())?;
```

Column names are checked against `Entity::COLUMNS_META`, so a typo is returned before SQLite runs. The builder also renders empty IN lists, NULL comparisons, and LIKE escaping into safe SQL and bind values.

Execute the same query asynchronously:

```rust
let products = query.fetch_all(db.run_async()).await?;
```

For static SQL used repeatedly, `#[dao]` is shorter and provides broader compile-time validation. Use `Query` when conditions, sorting, or pagination are dynamic.

## Database configuration

```rust
use std::time::Duration;

let db = AppDb::builder()
    .sqlite("app.db")
    .connections(5)
    .busy_timeout(Duration::from_secs(5))
    .queue_timeout(Duration::from_secs(2))
    .live_debounce(Duration::from_millis(250))
    .notifier_readers(2)
    .build()?;
```

| Setting | Default and purpose |
|---|---|
| `.sqlite(path)` | File database |
| `.in_memory()` | In-memory test database with one connection |
| `.connections(n)` | CPU-based default, at most five unified read/write connections |
| `.busy_timeout(d)` | Five seconds by default; wait for SQLite locks |
| `.queue_timeout(d)` | Unlimited by default; bound pool checkout waits |
| `.live_debounce(d)` | 250ms by default; DB-wide fixed LiveQuery coalesce window |
| `.notifier_readers(n)` | `min(2, connections)` by default; LiveQuery refresh workers |
| `.on_create(f)` | Runs once inside initial schema creation |
| `.on_open(f)` | Runs whenever a connection opens |
| `.query_logger(f)` | Receives the SQL string and elapsed time |

Every general-purpose connection can read and write. A transaction uses the same checked-out connection and begins with `BEGIN IMMEDIATE`. WAL and `busy_timeout` coordinate write contention within and across processes.

Do not execute `BEGIN` or `COMMIT` inside `on_create`. If the callback fails, DDL and `user_version` roll back together.

```rust
let db = AppDb::builder()
    .on_create(|conn| {
        conn.execute_batch(
            "INSERT INTO settings(key, value) VALUES ('created', '1')",
        )
        .map_err(roomrs::Error::from)
    })
    .on_open(|conn| {
        conn.execute_batch("PRAGMA optimize")
            .map_err(roomrs::Error::from)
    })
    .query_logger(|sql, elapsed| {
        log::debug!("query completed in {elapsed:?}: {sql}");
    })
    .build()?;
```

The `on_create` example assumes that a `settings` entity belongs to the database. Using `query_logger` requires a `log` implementation and logger initialization in the application.

## Asynchronous API

The default features generate an async implementation for the same DAO:

```rust
use roomrs::BuildAsyncExt;

async fn run() -> roomrs::Result<()> {
    let db = AppDb::builder()
        .sqlite("app.db")
        .build_async()
        .await?;
    let handle = db.run_async();
    let dao = handle.todo_dao();

    let todo = Todo {
        id: 0,
        title: "Asynchronous work".into(),
        done: false,
    };
    let id = dao.add(&todo).await?;

    let todo = dao.find(id).await?;
    println!("{todo:?}");
    Ok(())
}
```

Ordinary query and insert methods convert arguments to owned SQLite values before dispatching work, so they can retain the synchronous DAO argument shape. An async `#[transaction]` DAO method is different: its entire body moves to a worker and therefore accepts only owned arguments satisfying the `'static` bound.

roomrs async APIs return `Future + Send`. They work with tokio, smol, async-std, and `futures::executor`. If the `tokio` feature is disabled while `async` remains enabled, roomrs uses its runtime-independent worker path.

Async transactions run a **synchronous closure** on a worker:

```rust
db.run_async()
    .transaction(|tx| {
        tx.todo_dao().set_done(first_id, true)?;
        tx.todo_dao().set_done(second_id, true)?;
        Ok(())
    })
    .await?;
```

The closure cannot use `.await`. Complete network requests and other async work before or after the transaction, and keep only database changes inside it.

## LiveQuery

`LiveQuery<T>` queries once immediately after registration, then refreshes after a write to a related table commits.

```rust
use roomrs::LiveQuery;

#[dao]
trait TodoDao {
    #[query("SELECT COUNT(*) FROM todos WHERE done = 0")]
    fn watch_open_count(&self) -> LiveQuery<i64>;
}

let live = db.run_sync().todo_dao().watch_open_count();
let guard = live.subscribe(|count| {
    println!("open tasks: {count}");
});

// The subscription ends when guard is dropped.
```

Consumption options:

```rust
let first = live.recv()?;
let next = live.recv_timeout(Duration::from_secs(1))?;
let ready = live.try_recv()?;

for value in live.iter() {
    println!("{}", value?);
}

// feature `async`
let mut stream = live.into_stream();
```

The `subscribe` callback runs on a LiveQuery worker thread. Forward long-running work to another queue instead of blocking the callback. Keep the returned `SubscriptionGuard`; dropping it immediately cancels the subscription.

### Debounce and coalescing

The default debounce is a fixed 250ms coalesce window:

```rust
let db = AppDb::builder()
    .live_debounce(Duration::from_millis(500))
    .build()?;

let live = db.run_sync()
    .todo_dao()
    .watch_open_count()
    .debounce(Duration::from_millis(100));
```

- 250ms when the DB setting is absent
- the DB value when the observer setting is absent
- the observer value when `.debounce(...)` is specified
- the first invalidation starts the window
- additional invalidations merge without extending the deadline

Changes inside a transaction emit only after a successful commit. Rolled-back changes never emit.

### Row filters

Observe only changes matching row conditions instead of every table change:

```rust
use roomrs::InvalidationFilter;

let mine = InvalidationFilter::table("todos")
    .where_group(|group| {
        group
            .eq("owner_id", current_user_id)
            .eq("done", false)
    })
    .or_where_group(|group| {
        group.is_null("owner_id")
    })
    .build()?;
```

Conditions inside one group use AND; groups use OR. Supported conditions are `eq`, `neq`, `is_null`, and `is_not_null`. Passing multiple filters to an observer or DAO combines those filters with OR.

Filter table and column names are validated when registering the subscription. Typos return `Error::InvalidationFilter`.

Direct SQL subscriptions provide `watch_all`, `watch_optional`, `watch_scalar`, and filtered variants. If dependency analysis cannot identify a table, declare one with `.watching(&["table"])`.

```rust
let live = db.run_sync()
    .watch_scalar::<i64>(
        "SELECT COUNT(*) FROM todos",
        roomrs::params![],
    )
    .watching(&["todos"]);
```

When only the page or search values change, replace bind values without creating another observer:

```rust
let page = db.run_sync().watch_all::<Todo>(
    "SELECT * FROM todos ORDER BY id LIMIT ?1 OFFSET ?2",
    roomrs::params![20i64, 0i64],
);

page.rebind(roomrs::params![20i64, 20i64])?;
```

LiveQuery currently observes only writes made through roomrs connections in the same process. It does not observe another process or an external SQLite tool.

## Transactions

### DAO transactions

```rust
#[dao]
trait AccountDao {
    #[query("SELECT balance FROM accounts WHERE id = :id")]
    fn balance(&self, id: i64) -> roomrs::Result<i64>;

    #[update("UPDATE accounts SET balance = balance + :delta WHERE id = :id")]
    fn adjust(&self, id: i64, delta: i64) -> roomrs::Result<u64>;

    #[transaction]
    fn transfer(
        &self,
        from: i64,
        to: i64,
        amount: i64,
    ) -> roomrs::Result<()> {
        if self.balance(from)? < amount {
            return Err(roomrs::Error::Config(
                "insufficient balance".into(),
            ));
        }
        self.adjust(from, -amount)?;
        self.adjust(to, amount)?;
        Ok(())
    }
}
```

Calls in the form `self.method(...)` inside the macro body are rewritten to tx-bound DAO calls that use the same transaction connection.

### Closure transactions and savepoints

```rust
use AppDbTxDaos as _;

db.run_sync().transaction(|tx| {
    tx.account_dao().adjust(a, -10)?;

    let nested: roomrs::Result<()> =
        roomrs::SqlContext::ctx_transaction(&&*tx, |savepoint| {
            savepoint.account_dao().adjust(b, 10)?;
            Ok(())
        });

    nested?;
    Ok(())
})?;
```

A nested transaction becomes a SQLite savepoint.

### RAII transactions

```rust
{
    let tx = db.run_sync().begin()?;
    tx.execute(
        "UPDATE accounts SET balance = 0",
        roomrs::params![],
    )?;
    // Leaving the scope without commit rolls back.
}
```

Call `tx.commit()?` or `tx.rollback()?` to end it explicitly.

## Relation mapping

A relation view loads a parent entity and related entities using batched queries. It does not run one query per parent.

```rust
use roomrs::Relation;

#[derive(Relation, Debug)]
struct AuthorView {
    #[embedded]
    author: Author,

    #[relation(
        entity = Book,
        parent_key = "id",
        entity_key = "author_id"
    )]
    books: Vec<Book>,

    #[relation(
        entity = Profile,
        parent_key = "id",
        entity_key = "author_id"
    )]
    profile: Option<Profile>,
}

#[dao]
trait LibraryDao {
    #[query(with_relations, "SELECT * FROM authors ORDER BY id")]
    fn authors(&self) -> roomrs::Result<Vec<AuthorView>>;
}
```

- `Vec<T>`: 1:N
- `Option<T>`: 1:1
- `Vec<T>` with junction settings: N:M

N:M example:

```rust
#[relation(
    entity = Genre,
    parent_key = "id",
    entity_key = "id",
    junction = "book_genres",
    junction_parent_key = "book_id",
    junction_entity_key = "genre_id"
)]
genres: Vec<Genre>,
```

Relation loading runs in an automatic transaction to provide a consistent result.

## Schema snapshots

### File location

Default path:

```text
migrations/schema/[database_name].[version].json
```

Example:

```text
migrations/schema/app_db.1.json
migrations/schema/app_db.2.json
```

Set `ROOMRS_SCHEMA_DIR` to override the location. Every developer and CI job must use the same value.

### Commands

```sh
cargo roomrs schema export
cargo roomrs schema check

cargo roomrs migrate diff old.json new.json migration.sql
cargo roomrs migrate check old.json new.json
cargo roomrs migrate check-dir migrations/schema app_db --strict
```

`schema export`:

- discovers `#[database]` in ordinary library and binary targets in the current workspace
- creates a missing snapshot
- refuses to overwrite a same-revision file when entities differ
- creates the next revision and a forward SQL draft for changed `version = auto` schemas
- plans all database writes before creating any file

`schema check`:

- never writes files
- compares code to the latest snapshot hash
- checks conflicting PK declarations, corrupt snapshots, and version mismatches

### Manual versions

```rust
#[database(entities(Todo), daos(TodoDao), version = 2)]
struct AppDb;
```

Increment the version explicitly when changing the schema. If a snapshot already exists for the same version and the entity differs, export preserves the file and returns an error.

### Automatic versions

```rust
#[database(entities(Todo), daos(TodoDao), version = auto)]
struct AppDb;
```

At export time, roomrs compares the latest snapshot with the current entity hash:

- same hash: no-op, no new files
- different hash: create the next integer revision
- previous revision present: also create a forward migration SQL draft
- destructive or ambiguous change: write no files, fail, and direct the user to a manual version and migration

Always rebuild after export:

```sh
cargo roomrs schema export
cargo build
```

To obtain a review-only TODO draft for a destructive change, prepare the old and new snapshots and run the separate diff command:

```sh
cargo roomrs migrate diff old.json new.json migration.sql
```

### Changing a schema

Recommended sequence:

1. Change the entity.
2. In manual mode, increment `version = N + 1`.
3. For a rename, add `#[column(renamed_from = "old_name")]`.
4. Run `cargo roomrs schema export`.
5. Review the new JSON and migration SQL draft in a diff.
6. Complete non-automatic changes with a manual migration.
7. Run `cargo roomrs schema check`.
8. Run `cargo build` and tests.
9. Commit JSON and migration files with the code.

Renaming a field without `renamed_from` may look like deleting the old column and adding a new one. State the rename explicitly when data must survive.

Never edit a released version's snapshot or deploy a different schema under the same version. A database already carrying that version cannot detect the change from the version number.

## Migrations

SQLite stores the database revision as an integer in `PRAGMA user_version`. roomrs compares it with the current schema version while opening the database.

### SQL steps

```rust
use roomrs::Migration;

let db = AppDb::builder()
    .sqlite("app.db")
    .migration(Migration::sql(
        1,
        2,
        r#"
        ALTER TABLE "todos"
        ADD COLUMN "note" TEXT NOT NULL DEFAULT '';
        "#,
    ))
    .build()?;
```

### Rust code steps

```rust
let db = AppDb::builder()
    .migration(Migration::code(2, 3, |tx| {
        tx.execute_batch(
            r#"
            ALTER TABLE "todos"
            ADD COLUMN "priority" INTEGER NOT NULL DEFAULT 0;
            "#,
        )
    }))
    .build()?;
```

### SQL file directories

File names:

```text
migrations/
├─ 1_2_add_note.sql
└─ 2_3_add_priority.sql
```

Registration:

```rust
let db = AppDb::builder()
    .migrations(roomrs::migrations_dir!("migrations"))
    .build()?;
```

SQL file contents are embedded at compile time. `include_str!` tracks modifications to existing files, but a proc-macro cannot track the addition of a new directory entry. Recompile the macro call site or clean the package before building after adding a file.

### Automatic migration from embedded snapshots

```rust
let db = AppDb::builder()
    .sqlite("app.db")
    .auto_migrate(true)
    .build()?;
```

This fills gaps without registered steps by diffing consecutive snapshots. A registered manual step always takes precedence.

Automatically executable changes:

- CREATE TABLE
- nullable ADD COLUMN
- NOT NULL ADD COLUMN with DEFAULT
- RENAME COLUMN with a valid rename hint
- ordinary CREATE INDEX
- adding, changing, or deleting database-level triggers

Changes requiring manual review:

- DROP TABLE or DROP COLUMN
- type, DEFAULT, or collation changes
- PK, FK, CHECK, or UNIQUE changes
- UNIQUE INDEX
- generated columns and STRICT/WITHOUT ROWID changes
- data transformations

Last resort:

```rust
let db = AppDb::builder()
    .fallback_to_destructive_migration(true)
    .build()?;
```

When a required chain is absent, this drops all managed tables and recreates them. Use it only for caches or reproducible data where data loss is acceptable.

roomrs never runs downgrade migrations automatically. If the database version is newer than the program, opening it safely is impossible and returns an error. Forward migration steps each run in a transaction; a failed step rolls back.

## Type conversion

### Built-in types

Rust built-in types use rusqlite `ToSql` and `FromSql`. Custom newtypes work after implementing those traits.

### JSON

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Preferences {
    theme: String,
}

#[entity(table = "profiles")]
struct Profile {
    #[pk]
    id: String,
    #[json]
    preferences: Preferences,
}
```

This requires the `json` feature, enabled by default. Invalid existing JSON data returns `Error::Json`.

### time and uuid

```rust
#[entity(table = "events")]
struct Event {
    #[pk]
    id: uuid::Uuid,
    created_at: time::OffsetDateTime,
}
```

These require the `uuid` and `time` features, enabled by default.

## Logging and observability

roomrs emits through the `log` facade and never installs a subscriber. The application chooses a logger or tracing bridge.

```rust
fn init_tracing() -> Result<(), Box<dyn std::error::Error>> {
    tracing_log::LogTracer::init()?;
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("roomrs_core=debug")
            });
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
```

PowerShell:

```powershell
$env:RUST_LOG = "info,roomrs_core=trace,roomrs_async=debug"
cargo run
```

Bash:

```sh
RUST_LOG="info,roomrs_core=trace,roomrs_async=debug" cargo run
```

roomrs does not log SQL parameters, encryption keys, or row data.

LiveQuery state:

```rust
let metrics = db.live_metrics();
println!("{metrics:?}");
```

This exposes cumulative counts for received invalidations, coalescing, refreshes, and related events.

## Error handling and troubleshooting

Every public failure returns `roomrs::Error`.

```rust
match AppDb::builder().sqlite("app.db").build() {
    Ok(db) => {
        // Use the database.
    }
    Err(error) => {
        log::error!(
            "DB open failed: {error}; path={}; advice={}",
            error.path().as_str(),
            error.advice().as_str(),
        );
        return Err(error);
    }
}
```

`path()` identifies the failure area and `advice()` provides a structured recommended action.

### `SnapshotStale`

Causes:

- no snapshot
- code and snapshot hashes differ under the same version
- export was not followed by a rebuild

Action:

```sh
cargo roomrs schema export
cargo roomrs schema check
cargo build
```

If the same-version file already represents another schema, increment the version instead of overwriting it.

### `Migration`

Causes:

- no step chain from the current DB version to the target
- a destructive operation appears in an automatic diff
- migration SQL failed

Action:

- inspect the `(from, to)` pair in the error
- register `Migration::sql` or `Migration::code`
- review TODO entries in the generated SQL draft
- write an explicit table rebuild when existing data must be copied or transformed

### `QueueTimeout`

Causes:

- every pool connection is held for too long
- a transaction or callback performs slow work

Action:

- remove network and file I/O from transactions
- check for nested connection checkout
- adjust `.connections(n)` or `.queue_timeout(d)` if appropriate

### `SQLITE_BUSY`

Another connection or process holds a write lock for too long. Keep transactions short and adjust `.busy_timeout(...)`.

### LiveQuery does not refresh

- confirm the change committed
- confirm the `SubscriptionGuard` is still alive
- check whether the SQL needs explicit `.watching(&["table"])`
- verify filter table/column names and OLD/NEW value conditions
- remember that writes from another process are not observed

## Known limitations

- SQLite only; no abstraction for other database backends.
- LiveQuery observes changes only from roomrs connections in the same process.
- `.await` inside an async transaction closure is not supported.
- `#[embedded]` marks a relation view parent and does not flatten entity columns.
- There is no view-specific entity DSL. Use an ordinary struct and `FromRow` for joins, aggregates, and projections as described in [arbitrary SELECT result structs](#arbitrary-select-result-structs).
- Automatic migration executes only safe forward operations.
- Treat every deployed schema version as an immutable revision.
- Every snapshot version is embedded, so binary size accumulates in long-lived projects.
- There is no FTS5- or R*Tree-specific DSL. Use manual SQL and `unchecked` queries when needed.

## Runnable examples

Run these from a repository checkout:

| Command | Demonstrates |
|---|---|
| `cargo run -p roomrs --example todo_sync` | Basic synchronous CRUD |
| `cargo run -p roomrs --example todo_async` | Runtime-independent async CRUD |
| `cargo run -p roomrs --example live_query` | LiveQuery callback |
| `cargo run -p roomrs --example transactions` | DAO transaction, savepoint, RAII |
| `cargo run -p roomrs --example migrations` | SQL/code migrations and diff |
| `cargo run -p roomrs --example relations` | 1:1, 1:N, N:M |
| `cargo run -p roomrs --example query_builder` | Dynamic condition building |
| `cargo run -p roomrs --example pagination` | LiveQuery rebind pagination |
| `cargo run -p roomrs --example bench --release` | Simple throughput measurement |

The mobile FFI example is in [`examples/mobile-ffi`](../examples/mobile-ffi/).

## Platforms and cross-building

- MSRV Rust 1.85, Edition 2024
- Windows, Linux, and macOS
- Android and iOS through a Rust `cdylib` FFI pattern

Repository development commands:

```sh
cargo xtask cross-linux
cargo xtask cross-android
cargo xtask cross-all
```

| Target | Tool |
|---|---|
| Linux x64/arm64 GNU | cargo-zigbuild |
| Linux x64 musl | cargo-zigbuild |
| Android arm64/armv7/x86_64 | cargo-ndk |
| iOS/macOS | macOS host with Xcode |

## Contributing and development

```sh
git clone https://github.com/yongaru/roomrs
cd roomrs
cargo build --workspace
cargo test --workspace
```

Basic pre-PR checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

Do not use `--all-features`, because backend features are mutually exclusive. CI separately checks each canonical backend and verifies that conflicting combinations fail to compile.

Review the [roadmap](../ROADMAP.md) and [development plan](../roomrs-개발계획서.md) before changing public behavior. Bug reports should include roomrs and Rust versions, enabled features, OS, backend, and a minimal reproduction.
