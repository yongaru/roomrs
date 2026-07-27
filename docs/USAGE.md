# roomrs 상세 사용 가이드

[README로 돌아가기](../README.md) | [English](USAGE-en.md)

이 문서는 roomrs를 처음 설치하는 단계부터 스키마 변경과 배포까지 실제 작업 순서대로 설명합니다. 공개 API 목록이 필요하면 [docs.rs](https://docs.rs/roomrs)를, 실행 가능한 코드는 [예제 디렉터리](../crates/roomrs/examples/)를 참고하세요.

## 빠른 탐색

| 하려는 일 | 이동 |
|---|---|
| 처음 설치하고 실행하기 | [첫 데이터베이스 만들기](#첫-데이터베이스-만들기) |
| 엔티티와 컬럼 설정하기 | [엔티티 정의](#엔티티-정의) |
| CRUD DAO 만들기 | [DAO 정의](#dao-정의) |
| SQL 문자열을 바로 실행하기 | [SQL 문자열 직접 조회](#sql-문자열-직접-조회) |
| 비동기로 호출하기 | [비동기 API](#비동기-api) |
| 변경을 자동으로 관찰하기 | [LiveQuery](#livequery) |
| 여러 작업을 원자적으로 처리하기 | [트랜잭션](#트랜잭션) |
| 관계 데이터를 읽기 | [관계 매핑](#관계-매핑) |
| 컬럼을 추가하거나 이름을 바꾸기 | [스키마를 변경할 때](#스키마를-변경할-때) |
| 기존 사용자 DB를 업그레이드하기 | [마이그레이션](#마이그레이션) |
| SQLite 대신 SQLCipher 사용하기 | [설치와 backend 선택](#설치와-backend-선택) |
| 오류 원인과 조치 찾기 | [오류 처리와 문제 해결](#오류-처리와-문제-해결) |

## 핵심 작업 흐름

roomrs 프로젝트의 반복 작업은 다음 네 단계입니다.

1. Rust 코드에서 `#[entity]`, `#[dao]`, `#[database]`를 선언합니다.
2. `cargo roomrs schema export`로 현재 스키마 snapshot과, 안전하게 생성할 수 있을 때 migration SQL 초안을 만듭니다.
3. 생성물을 검토한 뒤 `cargo build`로 snapshot을 바이너리에 내장합니다.
4. 애플리케이션이 시작될 때 새 DB를 만들거나 등록된 forward migration을 실행합니다.

```text
엔티티 변경
  → cargo roomrs schema export
  → JSON·SQL 초안 검토 및 커밋
  → cargo build
  → 애플리케이션 배포
  → DB open 시 forward migration
```

`cargo build`와 `cargo test`는 snapshot이나 migration 파일을 생성·수정하지 않습니다. 파일 생성은 명시적인 `cargo roomrs schema export`에서만 일어납니다.

## 설치와 backend 선택

### 기본 설치

```toml
[dependencies]
roomrs = "0.4.0"
```

```sh
cargo install roomrs-cli
```

`roomrs-cli`는 `cargo-roomrs` 실행 파일 하나를 설치합니다. Cargo에서는 다음처럼 호출합니다.

```sh
cargo roomrs --help
```

기본 feature:

| feature | 기본 | 역할 |
|---|---:|---|
| `sqlite-bundled` | 예 | SQLite 소스를 함께 컴파일 |
| `async` | 예 | 런타임 무관 비동기 API |
| `tokio` | 예 | tokio 안에서 효율적으로 worker 작업 실행 |
| `live` | 예 | LiveQuery와 변경 무효화 |
| `time` | 예 | `time` 타입 매핑 |
| `uuid` | 예 | `Uuid` 타입 매핑 |
| `json` | 예 | `#[json]` 직렬화 |

### 순수 동기 최소 구성

```toml
[dependencies]
roomrs = {
    version = "0.4.0",
    default-features = false,
    features = ["sqlite-bundled"]
}
```

### backend 선택

다음 네 feature는 상호 배타적입니다. 애플리케이션에서는 정확히 하나를 선택합니다.

| backend | 용도 |
|---|---|
| `sqlite-bundled` | SQLite를 Cargo 빌드에 포함하는 기본값 |
| `sqlite-system` | OS package나 vcpkg의 SQLite에 링크 |
| `sqlcipher-bundled` | SQLCipher와 vendored OpenSSL을 Cargo에서 빌드 |
| `sqlcipher-system` | OS package나 vcpkg의 SQLCipher에 링크 |

SQLCipher bundled 예:

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

SQLCipher backend만 선택한다고 자동으로 암호화되지 않습니다. 모든 연결이 DB에 접근하기 전에 같은 `.encryption_key(...)`를 받아야 합니다. 키를 로그나 소스 저장소에 남기지 마세요.

Windows system backend는 저장소의 [`vcpkg/build-sqlcipher-system.cmd`](../vcpkg/build-sqlcipher-system.cmd)와 overlay port를 사용할 수 있습니다. `live` feature에는 `SQLITE_ENABLE_PREUPDATE_HOOK`이 활성화된 SQLite/SQLCipher가 필요합니다.

## 첫 데이터베이스 만들기

### 1. 프로젝트 생성

```sh
cargo new roomrs-example
cd roomrs-example
cargo add roomrs@0.4.0
cargo install roomrs-cli
```

### 2. 엔티티 선언

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

`Todo`는 Rust 값이고 `todos`는 SQLite 테이블입니다. `#[pk(autoincrement)]` 필드는 일반 `#[insert]`에서 제외되며 SQLite가 새 rowid를 만듭니다. 구조체를 만들 때 넣은 `id` 값은 이 insert 경로에서는 사용되지 않습니다. `0` 자체에 특별한 sentinel 의미는 없습니다.

### 3. DAO 선언

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

`#[query]`, `#[update]`, `#[delete]`의 `:name`은 메서드 인자명과 정확히 일치해야 합니다. snapshot이 존재하면 테이블과 컬럼도 컴파일 타임에 검사됩니다.

### 4. 데이터베이스 선언

```rust
use roomrs::database;

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct AppDb;
```

- `entities(...)`: 이 DB가 관리하는 엔티티
- `daos(...)`: 생성할 DAO 접근자
- `version = 1`: 현재 스키마 revision
- `version = auto`: export가 마지막 snapshot과 비교해 revision을 자동 결정

구조체 이름 `AppDb`는 snapshot 접두사 `app_db`가 됩니다. 한 crate 안의 모든 `#[database]`는 snake_case 변환 후에도 서로 다른 이름이어야 합니다.

### 5. snapshot 생성

```sh
cargo roomrs schema export
cargo build
```

첫 명령이 `migrations/schema/app_db.1.json`을 생성합니다. 두 번째 명령이 새 파일을 다시 읽어 바이너리에 내장합니다. snapshot 파일은 소스 코드와 함께 커밋하세요.

읽기 전용 확인만 하려면:

```sh
cargo roomrs schema check
```

### 6. DB 열기와 DAO 호출

```rust
fn main() -> roomrs::Result<()> {
    let db = AppDb::builder().sqlite("app.db").build()?;
    let handle = db.run_sync();
    let dao = handle.todo_dao();

    let id = dao.add(&Todo {
        id: 0,
        title: "문서 읽기".into(),
        done: false,
    })?;

    dao.set_done(id, true)?;

    for todo in dao.by_done(true)? {
        println!("{todo:?}");
    }
    Ok(())
}
```

신규 파일이면 roomrs가 현재 DDL을 하나의 transaction으로 생성하고 `PRAGMA user_version`을 기록합니다. 기존 파일이면 version과 등록된 migration chain을 확인합니다.

테스트에서는 파일 대신 다음을 사용할 수 있습니다.

```rust
let db = AppDb::builder().in_memory().build()?;
```

인메모리 DB는 SQLite 잠금 특성 때문에 통합 연결 하나를 사용합니다.

## Android Room과의 대응

| Android Room | roomrs |
|---|---|
| `@Entity` | `#[entity]` |
| `@PrimaryKey` | `#[pk]`, `#[entity(primary_key(...))]` |
| `@Ignore` | `#[column(ignore)]` |
| `@Dao` | `#[dao]` |
| `@Query` | `#[query("...")]` |
| `@Insert` | `#[insert]` |
| `@Update`, `@Delete` | SQL을 받는 `#[update("...")]`, `#[delete("...")]` |
| `@Transaction` | `#[transaction]` |
| `@Database(version = N)` | `#[database(..., version = N)]` |
| 자동 migration spec | snapshot diff + `.auto_migrate(true)` |
| `Migration(from, to)` | `Migration::sql`, `Migration::code` |
| `fallbackToDestructiveMigration()` | `.fallback_to_destructive_migration(true)` |
| `Flow<T>` | `LiveQuery<T>` |
| `@Relation`, `@Embedded` | `#[relation]`, `#[embedded]` |
| TypeConverter | rusqlite `ToSql`/`FromSql`, `#[json]` |
| KSP | proc-macro |

`#[embedded]`는 현재 관계 뷰의 부모 필드 표시입니다. 엔티티 내부 컬럼 평탄화 용도는 아직 지원하지 않습니다.

## 엔티티 정의

### Rust 타입과 SQLite 타입

| Rust 타입 | SQLite 선언 |
|---|---|
| 정수, `bool` | `INTEGER NOT NULL` |
| `f32`, `f64` | `REAL NOT NULL` |
| `String` | `TEXT NOT NULL` |
| `Vec<u8>` | `BLOB NOT NULL` |
| `Option<T>` | 같은 타입의 nullable 컬럼 |
| `time` 날짜·시간 타입 | `TEXT` |
| `uuid::Uuid` | `BLOB` |
| `#[json] T` | JSON `TEXT` |

알 수 없는 사용자 타입은 rusqlite의 `ToSql`/`FromSql` 구현에 위임됩니다. DDL 타입을 명시하려면 `sql_type`을 사용합니다.

```rust
#[column(sql_type = "DECIMAL(12,2)")]
amount: i64,
```

이 속성은 SQLite 저장 affinity와 DDL 선언을 바꿉니다. Rust 값 변환 자체는 여전히 해당 필드 타입의 `ToSql`/`FromSql`이 담당합니다.

### 컬럼 속성

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

지원하는 필드 속성:

| 속성 | 의미 |
|---|---|
| `#[pk]` | PRIMARY KEY 구성원 |
| `#[pk(autoincrement)]` | 단일 정수 auto-increment PK |
| `#[column(name = "...")]` | SQL 컬럼명 변경 |
| `#[column(unique)]` | 단일 컬럼 UNIQUE |
| `#[column(index)]` | 단일 컬럼 일반 index |
| `#[column(default = "...")]` | SQLite DEFAULT |
| `#[column(ignore)]` | DB 컬럼에서 제외 |
| `#[column(renamed_from = "...")]` | 이전 snapshot 컬럼명과 rename 연결 |
| `#[column(sql_type = "...")]` | custom SQL column type |
| `#[column(collate = "...")]` | BINARY·NOCASE·RTRIM 또는 사용자 collation |
| `#[column(generated = "...")]` | VIRTUAL generated column |
| `#[column(generated = "...", stored)]` | STORED generated column |
| `#[json]` | serde JSON을 TEXT에 저장 |

`default`는 숫자, `true`·`false`, `now`·`CURRENT_TIMESTAMP`, 괄호로 시작하는 SQL 식, 문자열을 구분해 렌더합니다. DEFAULT 변경은 기존 데이터 의미를 바꿀 수 있으므로 자동 migration 대상이 아닙니다.

### 단일·복합 PRIMARY KEY

필드 표기:

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

엔티티 표기:

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

복합 키 순서는 필드 선언 순서입니다. 두 표기를 함께 사용하면 목록과 순서가 정확히 같아야 하며, 다르면 schema export/check가 파일을 쓰기 전에 오류를 반환합니다.

`AUTOINCREMENT`는 단일 `INTEGER PRIMARY KEY`에만 사용할 수 있습니다.

### 테이블 제약과 index

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

index 컬럼에는 `asc`, `desc`, `collate 이름`을 지정할 수 있습니다. `where`를 주면 partial index가 됩니다. roomrs는 SQLite의 B-tree index를 생성하며 별도 index 알고리즘 선택 옵션은 제공하지 않습니다.

### STRICT, WITHOUT ROWID, generated column

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

generated column에는 `default`, PK, autoincrement를 함께 사용할 수 없습니다. 기존 테이블의 STRICT·WITHOUT ROWID·generated 정의 변경은 보통 테이블 재작성이 필요하므로 수동 migration으로 처리합니다.

### DB-level trigger

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

trigger는 테이블 부속 속성이 아니라 DB schema 객체입니다. 선언마다 `name`과 `sql` 또는 `file` 중 하나를 지정합니다. 파일 경로는 package의 `CARGO_MANIFEST_DIR` 기준입니다. SQL은 non-TEMP `CREATE TRIGGER` 문을 정확히 하나 포함해야 하며 선언 이름과 SQL 이름이 같아야 합니다. 한 connection에만 존재하는 TEMP trigger는 지원하지 않습니다.

이름·전체 SQL·선언 source가 snapshot과 hash에 포함됩니다. 신규 DB는 table과 index를 만든 뒤 trigger를 생성합니다. trigger 추가·삭제는 `CREATE TRIGGER`·`DROP TRIGGER`, 내용 변경은 drop 후 재생성하는 안전한 forward migration으로 자동 합성됩니다.

## DAO 정의

### 반환 타입

| DAO 반환 타입 | 동작 |
|---|---|
| `Result<Vec<T>>` | 모든 행 |
| `Result<Option<T>>` | 0행 또는 1행 |
| `Result<T>` | 정확히 1행, 없으면 `Error::NotFound` |
| `Result<u64>` | 영향받은 행 수 |
| `Result<i64>` on `#[insert]` | 새 rowid |
| `LiveQuery<Vec<T>>` | 변경을 관찰하는 목록 |
| `LiveQuery<Option<T>>` | 변경을 관찰하는 optional 값 |
| `LiveQuery<T>` | 변경을 관찰하는 scalar/단일 값 |

### query, update, delete

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

SQL은 SQLite 방언으로 parsing됩니다. 명명 파라미터는 문자열 안의 등장 순서와 관계없이 인자명으로 연결됩니다.

정적 스키마 대조가 불가능한 특수 SQL만 `unchecked`로 우회할 수 있습니다.

```rust
#[query(unchecked, "SELECT custom_runtime_function(value) FROM items")]
fn custom(&self) -> roomrs::Result<Vec<String>>;
```

`unchecked`도 SQL 파라미터와 메서드 인자의 정합성은 검사합니다. 일반 쿼리에 사용하면 컬럼 오타를 런타임까지 숨기므로 피하세요.

`#[update]`와 `#[delete]`는 보통 `Result<u64>` 영향 행 수를 반환합니다. SQL에 `RETURNING`이 있으면 `Result<T>`, `Result<Option<T>>`, `Result<Vec<T>>`로 반환할 수 있으며 `T`에는 아래와 같은 `FromRow` 규칙이 적용됩니다.

### SQL 문자열 직접 조회

고정 DAO 메서드가 필요하지 않은 쿼리는 동기 또는 비동기 handle에서 SQL 문자열로 바로 실행할 수 있습니다. `#[entity]` 타입은 `FromRow`가 자동 구현되므로 별도 변환 코드를 작성하지 않습니다.

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
    roomrs::params!["문서 읽기", 1_i64],
)?;

let changed: u64 = handle.execute(
    "UPDATE todos SET done = ?1 WHERE id = ?2",
    roomrs::params![true, 1_i64],
)?;
```

| 메서드 | 반환 타입 | 용도 |
|---|---|---|
| `query_one::<T, _>` | `T` | SELECT 또는 `... RETURNING` 한 행. 없으면 `roomrs::Error::NotFound` |
| `query_optional::<T, _>` | `Option<T>` | 0행 또는 한 행 |
| `query_all::<T, _>` | `Vec<T>` | 모든 행 |
| `query_scalar::<T, _>` | `T` | 첫 행의 단일 컬럼 |
| `execute` | `u64` | INSERT·UPDATE·DELETE 등으로 변경된 행 수 |

`Row<T>` wrapper는 없습니다. `::<Todo, _>`의 `_`는 `params![...]`의 파라미터 타입을 Rust가 추론하도록 둡니다. 파라미터가 없으면 `()`를 전달합니다. `query_scalar`의 `T`는 `FromSql`, 나머지 조회 메서드의 `T`는 `FromRow`를 구현해야 합니다.

비동기 handle은 SQL 문자열 타입 generic이 앞에 있어 turbofish 순서가 `::<_, Todo, _>`입니다.

```rust
let todo: Todo = db
    .run_async()
    .query_one::<_, Todo, _>(
        "SELECT id, title, done FROM todos WHERE id = ?1",
        roomrs::params![1_i64],
    )
    .await?;
```

SQL 문자열 직접 조회는 DAO 매크로의 snapshot 기반 컴파일 타임 SQL 검증을 거치지 않으므로, 반복 사용하는 고정 SQL은 `#[query]` DAO 메서드가 더 안전합니다.

`INSERT`, `UPDATE`, `DELETE ... RETURNING`은 결과 행 수에 맞춰 `query_one`, `query_optional`, `query_all`로 받을 수 있습니다. `RETURNING` 없는 쓰기는 `execute`를 사용합니다.

내부 매핑에 쓰이는 `roomrs::rusqlite::Row<'_>`는 `serde_json::Value`가 아닙니다. SQLite statement에서 현재 한 행을 빌린 타입이며 `FromRow`가 각 컬럼을 Rust 결과 타입으로 변환합니다. `query_one::<Todo, _>`의 실제 반환값은 `Row`가 아니라 `Todo`입니다.

### 임의 SELECT 결과 구조체

조회 결과 타입은 `#[entity]`일 필요가 없습니다. JOIN, 집계, 별칭을 사용한 projection처럼 독립 테이블과 일치하지 않는 결과는 일반 구조체에 `FromRow`를 직접 구현합니다.

다음 예제는 기존 `todos`, `users` 엔티티를 JOIN하지만 반환값은 별도 테이블이 아닌 화면용 구조체입니다.

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

이 구조체에는 `#[entity]`를 붙이지 않고 `#[database(entities(...))]`에도 넣지 않습니다. 따라서 snapshot, DDL, migration 대상이 되지 않습니다. 생성된 `todo_view_dao()` 접근자를 사용하려면 `TodoViewDao`만 `#[database(..., daos(...))]`에 등록합니다. SQL이 참조하는 실제 테이블과 컬럼은 등록된 엔티티 snapshot을 기준으로 계속 검증되며, SELECT alias와 `row.get(...)` 이름은 일치해야 합니다.

DAO 없이 직접 조회할 수도 있습니다.

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

`RETURNING` 결과도 같은 방식으로 매핑합니다.

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

동기 직접 조회는 `query_one`, `query_optional`, `query_all`을 제공합니다. 비동기 handle의 같은 메서드는 `.await`하며, 반환 구조체가 `Send + 'static`이어야 합니다. `LiveQuery` 결과로 사용하려면 `Clone + Send + 'static`도 충족해야 합니다.

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

- 기본 `#[insert]`는 autoincrement PK를 제외합니다.
- `keep_pk`는 PK를 INSERT 컬럼에 포함합니다.
- `on_conflict` 값으로 `replace`, `abort`, `rollback`, `fail`을 사용할 수 있습니다.
- parser는 `ignore`도 인식하지만 `#[insert]`의 필수 `Result<i64>` rowid 계약과 양립하지 않으므로 컴파일 오류입니다.
- `#[insert]`는 항상 `Result<i64>` 새 rowid를 반환합니다. `INSERT OR IGNORE`처럼 0행 성공이 가능한 동작은 `#[insert]`로 표현할 수 없으므로 영향 행 수를 반환하는 명시적 SQL DAO 메서드를 사용하세요.

## 동적 Query Builder

검색 화면처럼 조건이 런타임에 달라진다면 `Query`를 사용합니다.

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

컬럼명은 `Entity::COLUMNS_META`와 대조되므로 오타는 SQLite를 호출하기 전에 오류로 반환됩니다. 빈 IN 목록, NULL 비교, LIKE escape도 builder가 안전한 SQL과 bind 값으로 렌더합니다.

같은 query를 비동기로 실행할 수 있습니다.

```rust
let products = query.fetch_all(db.run_async()).await?;
```

정적이고 반복되는 SQL은 `#[dao]`가 더 간결하고 컴파일 타임 검증 범위가 넓습니다. `Query`는 조건·정렬·페이지가 동적으로 바뀌는 경우에 사용하세요.

## 데이터베이스 설정

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

| 설정 | 기본값·설명 |
|---|---|
| `.sqlite(path)` | 파일 DB |
| `.in_memory()` | 테스트용 메모리 DB, 연결 하나 |
| `.connections(n)` | CPU 기반, 최대 5개의 통합 read/write 연결 |
| `.busy_timeout(d)` | 기본 5초, SQLite lock 대기 |
| `.queue_timeout(d)` | 기본 무제한, pool checkout 대기 제한 |
| `.live_debounce(d)` | 기본 250ms, DB 전역 LiveQuery 고정 coalesce 창 |
| `.notifier_readers(n)` | 기본 `min(2, connections)`, LiveQuery 재조회 worker |
| `.on_create(f)` | 최초 생성 transaction 내부에서 한 번 |
| `.on_open(f)` | 연결을 열 때마다 실행 |
| `.query_logger(f)` | SQL 문자열과 소요 시간 callback |

모든 일반 연결은 읽기와 쓰기가 가능합니다. transaction은 checkout한 같은 연결에서 `BEGIN IMMEDIATE`로 시작합니다. WAL과 `busy_timeout`이 프로세스 안팎의 write 경합을 조정합니다.

`on_create` 안에서 직접 `BEGIN`이나 `COMMIT`을 실행하지 마세요. callback 실패 시 DDL과 `user_version`이 함께 rollback됩니다.

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

위 `on_create` 예시는 `settings` 엔티티가 DB에 포함되어 있다고 가정합니다. `query_logger`를 사용하려면 애플리케이션에 `log` 구현과 logger 초기화가 필요합니다.

## 비동기 API

기본 feature에서는 같은 DAO에 비동기 구현도 생성됩니다.

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
        title: "비동기 작업".into(),
        done: false,
    };
    let id = dao.add(&todo).await?;

    let todo = dao.find(id).await?;
    println!("{todo:?}");
    Ok(())
}
```

일반 query와 insert는 worker로 보내기 전에 인자를 소유 SQLite 값으로 변환하므로 동기 DAO와 같은 인자 형태를 유지할 수 있습니다. 다만 본문 전체가 worker로 이동하는 비동기 `#[transaction]` DAO 메서드는 빌린 인자를 받을 수 없으며 소유 인자와 `'static` 제약을 따릅니다.

roomrs 비동기 API는 `Future + Send`를 반환합니다. tokio, smol, async-std, `futures::executor`에서 사용할 수 있습니다. `tokio` feature를 꺼도 `async` feature만 켜면 자체 worker 실행 경로를 사용합니다.

비동기 transaction은 다음처럼 **동기 클로저**를 worker에서 실행합니다.

```rust
db.run_async()
    .transaction(|tx| {
        tx.todo_dao().set_done(first_id, true)?;
        tx.todo_dao().set_done(second_id, true)?;
        Ok(())
    })
    .await?;
```

클로저 안에서는 `.await`할 수 없습니다. 네트워크 요청 같은 비동기 작업은 transaction 전후에 수행하고, DB 변경만 클로저 안에 두세요.

## LiveQuery

`LiveQuery<T>`는 등록 즉시 현재 값을 한 번 조회하고, 관련 테이블의 write가 commit된 뒤 다시 조회해 전달합니다.

```rust
use roomrs::LiveQuery;

#[dao]
trait TodoDao {
    #[query("SELECT COUNT(*) FROM todos WHERE done = 0")]
    fn watch_open_count(&self) -> LiveQuery<i64>;
}

let live = db.run_sync().todo_dao().watch_open_count();
let guard = live.subscribe(|count| {
    println!("남은 작업: {count}");
});

// guard가 drop되면 구독이 끝납니다.
```

소비 방법:

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

`subscribe` callback은 LiveQuery worker 스레드에서 호출됩니다. callback에서 오래 걸리는 작업을 직접 실행하지 말고 다른 작업 큐로 넘기세요. 반환된 `SubscriptionGuard`를 보관하지 않으면 즉시 drop되어 구독이 끝납니다.

### debounce와 병합

기본 debounce는 250ms 고정 coalesce 창입니다.

```rust
let db = AppDb::builder()
    .live_debounce(Duration::from_millis(500))
    .build()?;

let live = db.run_sync()
    .todo_dao()
    .watch_open_count()
    .debounce(Duration::from_millis(100));
```

- DB 설정이 없으면 250ms
- observer 설정이 없으면 DB 값
- observer에 `.debounce(...)`를 지정하면 observer 값
- 첫 invalidation이 창을 시작
- 창 안의 추가 invalidation은 합치기만 하고 마감 시간을 연장하지 않음

transaction 내부 변경은 commit 성공 후 한 번에 방출됩니다. rollback된 변경은 방출되지 않습니다.

### 행 필터

테이블 전체 변경이 아니라 특정 행 조건만 관찰할 수 있습니다.

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

한 group 안의 조건은 AND, group 사이는 OR입니다. 지원 조건은 `eq`, `neq`, `is_null`, `is_not_null`입니다. filter 여러 개를 observer나 DAO에 넘기면 filter 사이는 OR로 동작합니다.

필터가 참조하는 table·column은 구독 등록 시 검증됩니다. 오타는 `Error::InvalidationFilter`로 반환됩니다.

직접 SQL 구독에는 `watch_all`, `watch_optional`, `watch_scalar`와 각각의 filtered 변형을 사용할 수 있습니다. SQL 의존 테이블을 분석하지 못하면 `.watching(&["table"])`로 명시합니다.

```rust
let live = db.run_sync()
    .watch_scalar::<i64>(
        "SELECT COUNT(*) FROM todos",
        roomrs::params![],
    )
    .watching(&["todos"]);
```

페이지 번호나 검색 조건만 바꿀 때는 observer를 새로 만들지 않고 bind 값을 교체할 수 있습니다.

```rust
let page = db.run_sync().watch_all::<Todo>(
    "SELECT * FROM todos ORDER BY id LIMIT ?1 OFFSET ?2",
    roomrs::params![20i64, 0i64],
);

page.rebind(roomrs::params![20i64, 20i64])?;
```

현재 LiveQuery는 같은 프로세스의 roomrs 연결이 수행한 변경만 관찰합니다. 다른 프로세스나 외부 SQLite 도구의 write는 관찰하지 않습니다.

## 트랜잭션

### DAO transaction

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
            return Err(roomrs::Error::Config("잔액 부족".into()));
        }
        self.adjust(from, -amount)?;
        self.adjust(to, amount)?;
        Ok(())
    }
}
```

매크로 본문 안의 `self.method(...)` 호출은 같은 transaction 연결을 사용하는 tx-bound DAO 호출로 재작성됩니다.

### 클로저 transaction과 savepoint

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

중첩 transaction은 SQLite savepoint가 됩니다.

### RAII transaction

```rust
{
    let tx = db.run_sync().begin()?;
    tx.execute(
        "UPDATE accounts SET balance = 0",
        roomrs::params![],
    )?;
    // commit하지 않고 scope 종료: rollback
}
```

명시적으로 `tx.commit()?` 또는 `tx.rollback()?`을 호출할 수 있습니다.

## 관계 매핑

관계 뷰는 부모 엔티티와 관련 엔티티를 별도 쿼리로 일괄 로딩합니다. 부모 행마다 쿼리하는 N+1 방식은 사용하지 않습니다.

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
- junction 설정이 있는 `Vec<T>`: N:M

N:M 예:

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

관계 전체 로딩은 일관된 결과를 위해 자동 transaction 안에서 실행됩니다.

## 스키마 snapshot

### 파일 위치

기본 경로:

```text
migrations/schema/[database_name].[version].json
```

예:

```text
migrations/schema/app_db.1.json
migrations/schema/app_db.2.json
```

`ROOMRS_SCHEMA_DIR` 환경 변수로 경로를 바꿀 수 있습니다. 팀과 CI가 같은 값을 사용해야 합니다.

### 명령

```sh
cargo roomrs schema export
cargo roomrs schema check

cargo roomrs migrate diff old.json new.json migration.sql
cargo roomrs migrate check old.json new.json
cargo roomrs migrate check-dir migrations/schema app_db --strict
```

`schema export`:

- 현재 workspace의 일반 lib·binary target에서 `#[database]`를 탐색
- snapshot이 없으면 생성
- 같은 revision의 파일과 현재 엔티티가 다르면 덮어쓰지 않고 실패
- `version = auto`에서 변경이 있으면 다음 revision과 forward SQL 초안 생성
- 여러 DB 작업을 먼저 검사한 뒤 충돌이 없을 때만 파일 기록

`schema check`:

- 파일을 쓰지 않음
- 코드와 최신 snapshot hash 대조
- PK 이중 선언 충돌, 파손 snapshot, version 불일치 등을 검사

### 수동 version

```rust
#[database(entities(Todo), daos(TodoDao), version = 2)]
struct AppDb;
```

스키마를 바꿀 때 version을 직접 올립니다. 같은 version의 snapshot이 이미 있는데 엔티티가 달라지면 export가 기존 파일을 보존하고 오류를 냅니다.

### 자동 version

```rust
#[database(entities(Todo), daos(TodoDao), version = auto)]
struct AppDb;
```

export 시 최신 snapshot과 현재 엔티티 hash를 비교합니다.

- 같음: 아무 파일도 만들지 않는 no-op
- 다름: 다음 정수 revision snapshot 생성
- 이전 revision이 있음: forward migration SQL 초안도 생성
- 파괴적·모호한 변경: 파일을 쓰지 않고 실패하며 수동 version과 migration을 안내

export 후에는 반드시 다시 빌드합니다.

```sh
cargo roomrs schema export
cargo build
```

파괴적 변경의 검토용 TODO 초안이 필요하면 이전·새 snapshot을 준비해 별도 diff 명령을 사용합니다.

```sh
cargo roomrs migrate diff old.json new.json migration.sql
```

### 스키마를 변경할 때

권장 순서:

1. entity를 수정합니다.
2. 수동 모드라면 `version = N + 1`로 올립니다.
3. rename이면 새 필드에 `#[column(renamed_from = "old_name")]`을 지정합니다.
4. `cargo roomrs schema export`를 실행합니다.
5. 새 JSON과 migration SQL 초안을 diff로 검토합니다.
6. 자동화할 수 없는 변경은 수동 migration으로 보완합니다.
7. `cargo roomrs schema check`를 실행합니다.
8. `cargo build`와 테스트를 실행합니다.
9. JSON과 migration 파일을 코드와 같은 commit에 포함합니다.

컬럼명만 바꾸고 `renamed_from`을 생략하면 기존 컬럼 삭제와 신규 컬럼 추가로 해석될 수 있습니다. 데이터 보존이 필요하면 rename 의도를 반드시 명시하세요.

배포된 version의 snapshot을 수정하거나 같은 version으로 다른 스키마를 다시 배포하지 마세요. 이미 해당 version을 가진 DB는 버전 번호만으로 변경 사실을 알 수 없습니다.

## 마이그레이션

SQLite DB 파일의 현재 version은 `PRAGMA user_version`에 정수로 저장됩니다. roomrs는 DB open 시 이 값과 현재 스키마 version을 비교합니다.

### SQL step

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

### Rust 코드 step

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

### SQL 파일 디렉터리

파일명:

```text
migrations/
├─ 1_2_add_note.sql
└─ 2_3_add_priority.sql
```

등록:

```rust
let db = AppDb::builder()
    .migrations(roomrs::migrations_dir!("migrations"))
    .build()?;
```

SQL 파일 내용은 컴파일 타임에 바이너리에 내장됩니다. 기존 파일 수정은 `include_str!` 의존성으로 재빌드되지만, 디렉터리에 새 파일을 추가한 사실은 proc-macro가 직접 추적할 수 없습니다. 매크로 호출부를 다시 컴파일하거나 해당 package를 clean한 뒤 빌드하세요.

### 내장 snapshot 자동 migration

```rust
let db = AppDb::builder()
    .sqlite("app.db")
    .auto_migrate(true)
    .build()?;
```

등록된 step이 없는 구간을 연속 snapshot diff로 채웁니다. 등록된 수동 step이 항상 우선합니다.

자동 실행 가능한 변경:

- CREATE TABLE
- nullable ADD COLUMN
- DEFAULT가 있는 NOT NULL ADD COLUMN
- 유효한 rename hint의 RENAME COLUMN
- 일반 CREATE INDEX
- DB-level trigger 추가·수정·삭제

수동 검토가 필요한 변경:

- DROP TABLE·DROP COLUMN
- 타입·DEFAULT·collation 변경
- PK·FK·CHECK·UNIQUE 변경
- UNIQUE INDEX
- generated column과 STRICT·WITHOUT ROWID 변경
- 데이터 변환

마지막 수단:

```rust
let db = AppDb::builder()
    .fallback_to_destructive_migration(true)
    .build()?;
```

필요한 chain이 없으면 모든 관리 테이블을 삭제하고 다시 만듭니다. 데이터 손실을 허용하는 cache·재생성 가능 데이터에서만 사용하세요.

roomrs는 downgrade migration을 자동 실행하지 않습니다. DB version이 프로그램보다 높으면 안전하게 열 수 없으므로 오류를 반환합니다. 정방향 migration은 각 step transaction으로 실행되며 실패하면 해당 step이 rollback됩니다.

## 타입 변환

### 기본 타입

Rust 기본 타입은 rusqlite의 `ToSql`/`FromSql`을 사용합니다. 사용자 newtype도 이 두 trait를 구현하면 사용할 수 있습니다.

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

`json` feature가 필요하며 기본으로 활성화됩니다. JSON 형식이 깨진 기존 데이터는 `Error::Json`으로 반환됩니다.

### time과 uuid

```rust
#[entity(table = "events")]
struct Event {
    #[pk]
    id: uuid::Uuid,
    created_at: time::OffsetDateTime,
}
```

각각 `uuid`, `time` feature가 필요하며 기본으로 활성화됩니다.

## 로깅과 관측성

roomrs는 `log` 파사드로만 로그를 방출하고 subscriber를 설치하지 않습니다. 애플리케이션이 원하는 logger나 tracing bridge를 설치해야 합니다.

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

SQL 파라미터, 암호화 키, 행 데이터는 roomrs 로그에 기록하지 않습니다.

LiveQuery 상태:

```rust
let metrics = db.live_metrics();
println!("{metrics:?}");
```

수신한 invalidation, 병합, 재조회 등의 누적 카운터를 확인할 수 있습니다.

## 오류 처리와 문제 해결

모든 공개 실패는 `roomrs::Error`로 반환됩니다.

```rust
match AppDb::builder().sqlite("app.db").build() {
    Ok(db) => {
        // 사용
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

`path()`는 실패 영역, `advice()`는 권장 조치를 구조화해 제공합니다.

### `SnapshotStale`

원인:

- snapshot이 없음
- 같은 version의 코드와 snapshot hash가 다름
- export 뒤 재빌드하지 않음

조치:

```sh
cargo roomrs schema export
cargo roomrs schema check
cargo build
```

같은 version 파일이 이미 다른 구조라면 덮어쓰지 말고 version을 올리세요.

### `Migration`

원인:

- 현재 DB에서 목표 version까지 연결되는 step이 없음
- 자동 diff에 파괴적 변경이 있음
- migration SQL 실패

조치:

- 오류에 표시된 `(from, to)` 구간 확인
- 수동 `Migration::sql` 또는 `Migration::code` 등록
- 생성된 SQL 초안의 TODO 검토
- 기존 데이터를 복사·변환해야 하면 명시적 table rebuild 작성

### `QueueTimeout`

원인:

- 모든 pool 연결이 오래 점유됨
- transaction이나 callback이 긴 작업 수행

조치:

- transaction 안에서 네트워크·파일 I/O 제거
- checkout을 중첩하지 않도록 호출 구조 점검
- 필요하면 `.connections(n)` 또는 `.queue_timeout(d)` 조정

### `SQLITE_BUSY`

다른 연결이나 프로세스가 write lock을 오래 보유하고 있습니다. transaction을 짧게 유지하고 `.busy_timeout(...)`을 조정하세요.

### LiveQuery가 갱신되지 않음

- 변경이 commit되었는지 확인
- `SubscriptionGuard`가 살아 있는지 확인
- `.watching(&["table"])` 의존성을 명시해야 하는 SQL인지 확인
- filter table·column과 OLD/NEW 값 조건 확인
- 다른 프로세스가 변경한 것은 현재 관찰되지 않음

## 알려진 제약

- SQLite 전용이며 다른 DB backend 추상화를 제공하지 않습니다.
- LiveQuery는 같은 프로세스의 roomrs 연결에서 발생한 변경만 관찰합니다.
- async transaction 클로저 내부 `.await`는 지원하지 않습니다.
- `#[embedded]`는 관계 뷰 부모 마커이며 엔티티 컬럼 평탄화 기능이 아닙니다.
- view 전용 entity DSL은 제공하지 않습니다. JOIN·집계·projection은 [임의 SELECT 결과 구조체](#임의-select-결과-구조체)에 설명한 일반 구조체와 `FromRow`를 사용합니다.
- 자동 migration은 정방향 안전 연산만 실행합니다.
- 스키마 version은 배포 후 변경 불가능한 revision으로 취급해야 합니다.
- snapshot 모든 version이 바이너리에 내장되므로 장기 프로젝트에서는 크기가 누적됩니다.
- FTS5와 R*Tree 전용 DSL은 제공하지 않습니다. 필요한 경우 수동 SQL과 `unchecked` 쿼리를 사용합니다.

## 실행 가능한 예제

저장소 checkout에서 실행합니다.

| 명령 | 내용 |
|---|---|
| `cargo run -p roomrs --example todo_sync` | 기본 동기 CRUD |
| `cargo run -p roomrs --example todo_async` | 런타임 무관 비동기 CRUD |
| `cargo run -p roomrs --example live_query` | LiveQuery callback |
| `cargo run -p roomrs --example transactions` | DAO transaction, savepoint, RAII |
| `cargo run -p roomrs --example migrations` | SQL·코드 migration과 diff |
| `cargo run -p roomrs --example relations` | 1:1, 1:N, N:M |
| `cargo run -p roomrs --example query_builder` | 동적 조건 조립 |
| `cargo run -p roomrs --example pagination` | LiveQuery rebind 페이지 이동 |
| `cargo run -p roomrs --example bench --release` | 간이 처리량 측정 |

모바일 FFI 예제는 [`examples/mobile-ffi`](../examples/mobile-ffi/)에 있습니다.

## 플랫폼과 크로스 빌드

- MSRV Rust 1.85, Edition 2024
- Windows, Linux, macOS
- Android·iOS는 Rust `cdylib` FFI 패턴

저장소 개발 명령:

```sh
cargo xtask cross-linux
cargo xtask cross-android
cargo xtask cross-all
```

| 대상 | 도구 |
|---|---|
| Linux x64·arm64 GNU | cargo-zigbuild |
| Linux x64 musl | cargo-zigbuild |
| Android arm64·armv7·x86_64 | cargo-ndk |
| iOS·macOS | Xcode가 있는 macOS host |

## 기여와 개발

```sh
git clone https://github.com/yongaru/roomrs
cd roomrs
cargo build --workspace
cargo test --workspace
```

PR 전 기본 검사:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

backend feature는 상호 배타적이므로 `--all-features`를 사용하지 않습니다. CI는 canonical backend별 check·test·clippy와 충돌 조합의 compile failure를 별도로 검증합니다.

공개 기능 변경 전 [ROADMAP](../ROADMAP.md)과 [개발계획서](../roomrs-개발계획서.md)를 확인하세요. 버그 보고에는 roomrs/Rust version, feature, OS, backend, 최소 재현 코드를 포함해 주세요.
