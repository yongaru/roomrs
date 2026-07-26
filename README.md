# roomrs

**한국어** | [English](README-en.md)

[![CI](https://github.com/yongaru/roomrs/actions/workflows/ci.yml/badge.svg)](https://github.com/yongaru/roomrs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/roomrs.svg)](https://crates.io/crates/roomrs)
[![docs.rs](https://img.shields.io/docsrs/roomrs)](https://docs.rs/roomrs)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-informational)](#플랫폼--msrv--크로스-빌드)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#라이선스)

> Android **Room**과 같은 개발 경험을 목표로 하는 Rust용 **로컬 SQLite 퍼시스턴스** 라이브러리입니다.

---

## 소개

roomrs는 로컬 SQLite 데이터베이스를 다루는 Rust 라이브러리입니다. Android에서 Room을 써 본 분이라면 바로 익숙할 방식 그대로 — **엔티티는 구조체, DAO는 trait, SQL은 매크로 문자열**로 선언하면 나머지는 매크로가 생성합니다.

왜 만들었을까요? Rust 생태계에는 훌륭한 범용 ORM(diesel, SeaORM)이 있지만, Room이 제공하던 것들 — **라이브 쿼리**(데이터가 바뀌면 자동으로 다시 알려주는 구독), **컴파일 타임 SQL 검증**, 그리고 데스크톱·모바일을 하나의 코드로 커버하는 로컬 퍼시스턴스 경험 — 을 한 번에 주는 라이브러리는 없었습니다. roomrs는 그 빈자리를 채웁니다. 범용 ORM이 되려 하지 않고, **SQLite 전용 로컬 퍼시스턴스 한 가지를 제대로** 하는 것이 목표입니다.

## 빠른 실행

```toml
[dependencies]
roomrs = "0.3.0"
```

저장소를 내려받아 바로 동작을 확인하려면 다음을 실행합니다.

```sh
cargo run -p roomrs --example todo_sync
```

기본 설정은 SQLite를 함께 빌드하므로 시스템 SQLite 설치가 필요 없습니다. 첫 엔티티·DAO·데이터베이스 선언은 아래 [상세 예제](#상세-예제)에서 복사해 시작할 수 있습니다.

Room 사용자를 위한 개념 대응표:

| Room | roomrs |
|---|---|
| `@Entity` / `@PrimaryKey` / `@Ignore` | `#[entity]` / `#[pk]` / `#[column(ignore)]` |
| `@Dao` / `@Query` / `@Insert` | `#[dao]` / `#[query]` / `#[insert]` |
| `@Transaction` | `#[transaction]` |
| `@Database` | `#[database(entities(...), version = N)]` |
| `@Relation` / `@Embedded` | `#[relation]` / `#[embedded]`(관계 뷰의 부모 마커 — 엔티티 컬럼 평탄화는 아직 지원하지 않으며 v1.x에서 예정) |
| `@TypeConverter` | rusqlite `ToSql`/`FromSql` 위임 + `#[json]` |
| `Flow<List<T>>` | `LiveQuery<Vec<T>>` |
| `Migration(1, 2) { execSQL(...) }` | `Migration::sql(1, 2, "...")` |
| `fallbackToDestructiveMigration()` | `.fallback_to_destructive_migration(true)` |
| `suspend fun` | `db.run_async()`의 `async fn` |
| KSP | proc-macro |

## 로드맵

공개 기능 우선순위와 제외 범위는 [ROADMAP.md](ROADMAP.md)에서 확인할 수 있습니다. 구현 계약은 [개발계획서](roomrs-개발계획서.md), 이미 배포된 변경은 [CHANGELOG](CHANGELOG.md)에 기록합니다.

---

## 핵심 특징

- **예측 가능한 SQLite 동시성 — 통합 read/write 풀.** 일반 커넥션 N개는 모두 읽기와 쓰기가 가능하며, checkout guard가 커넥션 하나를 작업 동안 독점합니다. WAL과 `busy_timeout`이 프로세스 안팎의 잠금 경합을 조정하며, 제한 시간이 지나면 `SQLITE_BUSY`가 반환될 수 있습니다.
- **라이브 쿼리.** `LiveQuery<T>`를 반환하는 쿼리는 의존 테이블이 바뀔 때마다 자동으로 재조회 결과를 보내 줍니다. 동기(`recv`/`iter`/`subscribe` 콜백)와 비동기(`Stream`) 소비를 모두 지원합니다.
- **행 필터 무효화.** `InvalidationFilter`가 `preupdate_hook`의 OLD/NEW 행을 확인해 관계없는 변경의 재조회를 막습니다. 구독 등록 때 table/column 스키마를 검증해 오타는 `Error::InvalidationFilter`로 드러납니다. `watch_*_filtered`는 sync·async·DAO 모두에서 쓸 수 있으며, **복수 filter는 OR**로 하나라도 매칭되면 재조회합니다. 무효화는 commit/문장 단위로 방출되고, DB 전역 기본 250ms **고정** coalesce debounce(`.live_debounce(Duration)` · observer `.debounce(Duration)` override)로 재조회를 묶습니다. 재조회는 `roomrs-live-worker`가 통합 커넥션 풀에서 checkout해 병렬로 수행합니다(`.notifier_readers(N)`, 기본 `min(2, connections)`). `db.live_metrics()`로 수신·병합·재조회 카운터를 읽을 수 있습니다. 단일 프로세스 roomrs 커넥션만 관찰합니다.
- **컴파일 타임 SQL 검증.** `#[query("...")]`의 SQL을 커밋된 스키마 스냅샷과 대조합니다. 존재하지 않는 테이블·컬럼을 참조하면 컴파일 에러가 납니다. 파라미터(`:name` ↔ 인자) 정합성도 컴파일 타임에 확인합니다.
- **버전별 스키마 스냅샷 + 바이너리 내장 자동 마이그레이션.** 스키마의 각 버전이 `[db이름].[버전].json` 파일로 커밋되고, 전 버전이 압축되어 바이너리에 내장됩니다. `.auto_migrate(true)`를 켜면 마이그레이션 스텝을 등록하지 않은 구간을 내장 스냅샷 diff의 **안전 연산**(CREATE TABLE, nullable ADD COLUMN, `NOT NULL DEFAULT` ADD COLUMN, CREATE INDEX, 유효한 rename 힌트의 RENAME COLUMN)으로 자동 실행합니다. 파괴적 변경은 절대 자동 실행하지 않고 명확한 에러로 안내합니다.
- **런타임 무관 비동기.** 비동기 API는 순수 std `Future`(+`Send`)를 반환하므로 tokio, async-std, smol, `futures::executor` 어디서든 그대로 await할 수 있습니다. tokio 최적화 통합은 기본으로 켜지며 `default-features = false`로 제외할 수 있습니다.

기반은 [rusqlite](https://github.com/rusqlite/rusqlite) 동기 코어 + 자체 미니 풀입니다. 기본값은 bundled SQLite이고, SQLite와 SQLCipher 모두 bundled/system 링크 방식을 선택할 수 있습니다. SQLite는 C 레벨에서 동기이므로 어떤 라이브러리의 "async"든 결국 워커 오프로드입니다 — 그래서 roomrs는 **동기 코어 + 비동기 파사드** 구조를 택했습니다.

---

## 상세 예제

### 설치

```toml
[dependencies]
roomrs = "0.3.0"
```

> 아직 crates.io에 공개되지 않았다면 git 의존성으로 사용할 수 있습니다:
> `roomrs = { git = "https://github.com/yongaru/roomrs" }`

SQLite는 번들로 함께 컴파일되므로 시스템에 SQLite를 설치할 필요가 없습니다.

### 동기 사용

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
    #[insert] // PK 생략 삽입 — 새 id는 반환값으로
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

    let id = h.todo_dao().add(&Todo { id: 0, title: "명세 읽기".into(), done: false })?;
    println!("새 id = {id}");
    for t in h.todo_dao().by_done(false)? {
        println!("- [{}] {}", t.id, t.title);
    }
    Ok(())
}
```

### PRIMARY KEY 표기

PRIMARY KEY는 필드에 `#[pk]`를 붙이거나 엔티티에 한 번에 선언할 수 있습니다. 엔티티 목록은 Rust 필드명이며 선언 순서가 복합 키 순서가 됩니다.

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

두 표기를 함께 써도 되지만 목록과 순서가 정확히 같아야 합니다. 다르면 `cargo roomrs schema export`와 `cargo roomrs schema check`가 파일을 쓰기 전에 오류와 수정 조언을 반환합니다. `#[pk(autoincrement)]`는 기존처럼 단일 정수 필드에 직접 표기합니다.

### 비동기 사용

같은 엔티티·DAO 선언을 그대로 두고 핸들만 `run_async()`로 바꾸면 됩니다. 메서드 이름도 동일합니다.

```rust
use roomrs::{BuildAsyncExt, MigrationPolicy, dao, database, entity};

// ... 위와 동일한 #[entity] / #[dao] / #[database] 선언 ...

fn main() -> roomrs::Result<()> {
    // smol 사용 예 — tokio / async-std 등 어떤 실행기든 동일하게 동작
    smol::block_on(async {
        let db = Db::builder()
            .sqlite("todo.db")
            .migrate(MigrationPolicy::Auto)
            .build_async()
            .await?;
        let h = db.run_async();

        let id = h.todo_dao().add(&Todo { id: 0, title: "비동기".into(), done: false }).await?;
        println!("새 id = {id}");
        for t in h.todo_dao().by_done(false).await? {
            println!("- [{}] {}", t.id, t.title);
        }
        Ok(())
    })
}
```

전체 예제는 [crates/roomrs/examples/](crates/roomrs/examples/)에 있습니다 — `cargo run --example todo_sync`처럼 실행할 수 있습니다.

---

## 주요 사용법

### 라이브 쿼리

반환 타입을 `LiveQuery<T>`로 선언하면 그 쿼리는 "구독"이 됩니다. 구독 즉시 현재 값이 한 번 오고, 이후 의존 테이블에 write가 일어날 때마다 재조회 결과가 옵니다.

직접 SQL 구독은 `InvalidationFilter`로 조건과 관련 있는 행 변경만 재조회할 수 있습니다. 단일 filter뿐 아니라 `vec![f1, f2]`처럼 여러 개를 넘기면 filter 간 OR로 매칭합니다. DAO LiveQuery 메서드에도 `InvalidationFilter` 인자를 하나 이상 두면 같은 규칙이 적용됩니다. filter 내부 그룹 안 조건은 AND, 그룹 사이는 OR입니다. filter 객체 간 AND·중첩 boolean expression은 지원하지 않습니다. 복잡한 SQL 조건을 자동 분석하지 않으므로, 필터로 표현하기 어려우면 테이블 단위 구독을 사용하세요. DB 전역 지연은 `.live_debounce(Duration::from_millis(100))`, observer만 바꿀 때는 `.debounce(...)`를 쓰세요(기본 250ms 고정 창 — 창 안 추가 무효화는 만료를 늘리지 않고 병합만 합니다).

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

// 콜백 구독 — live worker 스레드에서 호출된다
let guard = live.subscribe(|n| println!("현재 todo 개수: {n}"));
// guard가 drop되면 구독 종료 — `let _ = ...`로 받으면 즉시 해지되니 주의

// 블로킹 수신도 가능
// let first = live.recv()?;

// 비동기에서는 Stream으로 소비 (feature `async`)
// let mut stream = live.into_stream();
```

트랜잭션 중의 변경은 누적되었다가 **commit이 성공한 뒤에만** 방출됩니다. 롤백되면 알림이 가지 않습니다. DAO 없이 직접 구독하려면 `db.run_sync().watch_all(...)` / `watch_optional(...)` / `watch_scalar(...)`를 쓰면 됩니다.

### 트랜잭션

세 가지 형태를 제공합니다.

**1) `#[transaction]` DAO 메서드** — 본문 전체가 하나의 트랜잭션이 됩니다. 중요한 점: 본문 안의 `self.xxx()` 호출은 매크로가 **tx-바운드 DAO 호출로 재작성**하므로 전부 같은 트랜잭션 커넥션을 사용합니다(풀 재획득 없음 → 자기 락 경합·데드락 불가). 이 재작성은 매크로 본문 안의 `self` 메서드 호출에만 적용된다는 점을 기억해 주세요.

```rust
#[dao]
trait AccountDao {
    #[query("SELECT balance FROM Account WHERE id = :id")]
    fn balance(&self, id: i64) -> roomrs::Result<i64>;

    #[update("UPDATE Account SET balance = balance + :delta WHERE id = :id")]
    fn adjust(&self, id: i64, delta: i64) -> roomrs::Result<u64>;

    /// 이체 — 중간 실패 시 전체 롤백
    #[transaction]
    fn transfer(&self, from: i64, to: i64, amount: i64) -> roomrs::Result<()> {
        if self.balance(from)? < amount {
            return Err(roomrs::Error::Config("잔액 부족".into()));
        }
        self.adjust(from, -amount)?;
        self.adjust(to, amount)?;
        Ok(())
    }
}
```

**2) 클로저 트랜잭션** — 중첩하면 savepoint가 됩니다.

```rust
db.run_sync().transaction(|tx| {
    tx.account_dao().adjust(a, -10)?;
    // 중첩 = SAVEPOINT — 내부 실패는 내부만 롤백
    let inner: roomrs::Result<()> = roomrs::SqlContext::ctx_transaction(&&*tx, |sp| {
        sp.account_dao().adjust(b, 999)?;
        Err(roomrs::Error::Config("내부 취소".into()))
    });
    println!("savepoint 결과: {inner:?}");
    Ok(())
})?;
```

**3) RAII** — `begin()`으로 열고, commit 없이 drop되면 롤백됩니다.

```rust
{
    let tx = db.run_sync().begin()?;
    tx.execute("UPDATE Account SET balance = 0", roomrs::params![])?;
    // commit 호출 없이 스코프 종료 → 롤백
}
```

비동기에서는 v1 기준 **동기 클로저형만** 지원합니다(`db.run_async().transaction(|tx| { ... }).await`) — 클로저 전체가 워커에서 실행되며, checkout한 같은 커넥션에서 `BEGIN IMMEDIATE`부터 commit 또는 rollback까지 유지됩니다. 클로저 안에서 await는 불가합니다. 또한 비동기 `#[transaction]` 메서드는 `'static` 제약 때문에 **소유 인자만** 받을 수 있습니다(빌린 인자는 컴파일 에러).

### 마이그레이션

세 가지 경로를 조합할 수 있고, 전부 `(from, to)` 버전 쌍의 체인으로 병합되어 스텝별 트랜잭션으로 실행됩니다.

```rust
use roomrs::Migration;

let db = AppDb::builder()
    .sqlite("app.db")
    // 1) 인라인 SQL 스텝
    .migration(Migration::sql(
        1, 2,
        r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#,
    ))
    // 2) 코드 스텝 — 임의 로직 가능
    .migration(Migration::code(2, 3, |tx| {
        tx.execute_batch(r#"ALTER TABLE "docs" ADD COLUMN "done" INTEGER NOT NULL DEFAULT 0"#)
    }))
    .build()?;
```

세 번째 경로는 **자동 diff 초안**입니다 — `cargo roomrs migrate diff`(또는 `roomrs::diff_sql`)가 두 스냅샷을 비교해 마이그레이션 SQL 초안을 만들어 줍니다. 초안은 검토용이며 자동 실행되지 않고, 파괴적 변경은 TODO 주석으로 표시됩니다. SQL 파일 디렉토리를 통째로 컴파일 타임에 임베드하는 `migrations_dir!("경로")`도 있습니다(`{from}_{to}_이름.sql` 규칙).

**바이너리 내장 자동 마이그레이션(옵트인)** — 스냅샷 전 버전이 바이너리에 내장되므로, 스텝을 등록하지 않은 구간도 자동으로 메울 수 있습니다:

```rust
let db = AppDb::builder()
    .sqlite("app.db")
    .auto_migrate(true) // 등록 스텝 없는 구간을 내장 스냅샷 diff로 자동 실행
    .build()?;
```

단, 자동 실행은 **안전 연산**(CREATE TABLE, nullable ADD COLUMN, `NOT NULL DEFAULT` ADD COLUMN, CREATE INDEX, 유효한 rename 힌트의 RENAME COLUMN)에 한정됩니다. DROP·타입 변경·DEFAULT 변경·제약을 깨는 rename이 필요한 구간은 자동 실행하지 않고, 어느 구간이 왜 불가한지 알려 주는 에러를 냅니다. 그 구간에 수동 스텝을 등록하면 등록 스텝이 항상 우선합니다. 마지막 수단으로 전체를 버리고 재생성하는 옵트인 폴백도 있습니다:

```rust
.fallback_to_destructive_migration(true) // 체인 불충분 시 전체 drop + 재생성 — 데이터 소실 주의
```

### 멀티 프로세스 무효화

현재 지원하지 않습니다. 배경과 재검토 조건은 [다중 프로세스 LiveQuery 무효화 메모](docs/MULTI_PROCESS_INVALIDATION.md)에 보관합니다.

### 타입 컨버터

타입 변환은 rusqlite의 `ToSql`/`FromSql`에 위임합니다. 자주 쓰는 타입은 feature로 켜면 바로 컬럼 타입에 매핑되고(`time`·`uuid`·`json`은 기본 on), 임의의 serde 타입은 `#[json]`으로 TEXT 컬럼에 직렬화됩니다.

```rust
#[entity(table = "profiles")]
struct Profile {
    #[pk(autoincrement)]
    id: i64,
    created_at: time::OffsetDateTime, // feature `time` — TEXT로 저장
    token: uuid::Uuid,                // feature `uuid` — BLOB으로 저장
    #[json]
    prefs: Prefs,                     // serde 직렬화 — TEXT로 저장 (Serialize + Deserialize 필요)
    #[column(ignore)]
    cache: Option<String>,            // 테이블 컬럼에서 제외
}
```

### 그 밖의 예제

| 예제 | 케이스 |
|---|---|
| `todo_sync` / `todo_async` | 기본 CRUD — 동기 / 비동기(런타임 무관) |
| `transactions` | `#[transaction]` 이체 · 중첩 savepoint · RAII 롤백 |
| `migrations` | 버전 체인(1→2→3) · SQL/코드 스텝 · diff 초안 |
| `relations` | 1:N / 1:1 / N:M — `with_relations` N+1 회피 |
| `query_builder` | 동적 조건 조립 · 스키마 검증 · 동기/비동기 핸들 대칭 |
| `live_query` | `LiveQuery` 구독 콜백 + tracing 로그 브리지 |
| `pagination` | `rebind` 페이지 이동 + write 자동 갱신 |
| `bench` | 간이 처리량 측정 (`--release`) |

모바일 FFI(cdylib, `extern "C"`) 패턴과 안정적인 음수 에러 코드 규약은 [examples/mobile-ffi/](examples/mobile-ffi/)를 참고하세요.

---

## 기능 플래그

| feature | 기본 | 설명 |
|---|---|---|
| `sqlite-bundled` | on | SQLite 소스를 Cargo 빌드에 포함 |
| `sqlite-system` | off | OS package 또는 vcpkg SQLite에 링크 |
| `sqlcipher-bundled` | off | vendored OpenSSL을 포함한 SQLCipher를 Cargo에서 빌드 |
| `sqlcipher-system` | off | OS package 또는 vcpkg SQLCipher에 링크 |
| `bundled` | off | `sqlite-bundled` 하위 호환 alias |
| `cipher` | off | `sqlcipher-bundled` 하위 호환 alias |
| `async` | on | 런타임 무관 비동기 파사드. 끄면 순수 동기 |
| `tokio` | on | tokio 통합 최적화(`async` 포함) — tokio 런타임 밖에서는 자체 워커 풀로 폴백 |
| `live` | on | 라이브 쿼리 / 무효화 |
| `time`, `uuid`, `json` | on | 타입 컨버터 |

순수 동기 최소 빌드:

```toml
roomrs = { version = "0.3.0", default-features = false, features = ["sqlite-bundled"] }
```

기본 feature 전체는 `sqlite-bundled`, `async`, `tokio`, `live`, `time`, `uuid`, `json`입니다. 기본 빌드는 SQLite만 소스에서 컴파일하며 OpenSSL을 의존하지 않습니다. SQLCipher는 `default-features = false`와 `sqlcipher-bundled` 또는 `sqlcipher-system`을 명시해 선택합니다. SQLCipher backend를 사용해도 `.encryption_key(...)`를 지정하지 않은 데이터베이스는 자동으로 암호화되지 않습니다.

네 canonical backend feature는 동시에 둘 이상 선택할 수 없습니다. backend 없는 `roomrs-core` 빌드는 허용하지만, 데이터베이스를 사용하는 애플리케이션은 정확히 하나를 선택해야 합니다. 기존 `bundled`와 `cipher` 사용자는 각각 동일한 bundled backend를 계속 사용합니다.

### Windows system backend와 vcpkg

system backend는 SQLite/SQLCipher C 소스를 Cargo가 다시 컴파일하지 않고 이미 설치된 라이브러리에 링크합니다. `live`는 preupdate hook을 사용하므로 SQLite에는 vcpkg의 `session` feature가 필요합니다.

```powershell
$env:VCPKG_ROOT = "D:\tools\vcpkg"
$env:VCPKGRS_TRIPLET = "x64-windows-static-release"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
& "$env:VCPKG_ROOT\vcpkg.exe" install "sqlite3[session]:x64-windows-static-release"
```

```toml
roomrs = { version = "0.3.0", default-features = false, features = ["sqlite-system", "async", "live", "time", "uuid", "json"] }
```

공식 SQLCipher port는 preupdate hook을 활성화하지 않습니다. 저장소의
`vcpkg/build-sqlcipher-system.cmd`는 인접한 `vcpkg/ports/sqlcipher` overlay를 사용해
`x64-windows-static-release` SQLCipher를 설치한 뒤 `SQLITE_ENABLE_PREUPDATE_HOOK`을 검증합니다.
`VCPKG_ROOT`를 설정하거나 `vcpkg.exe`를 PATH에 추가한 후 저장소 루트에서 실행하세요.
overlay가 제공하는 `fts5`, `geopoly`, `json1`, `tool`을 모두 설치합니다.

```powershell
$env:VCPKG_ROOT = "D:\tools\vcpkg"
$env:VCPKGRS_TRIPLET = "x64-windows-static-release"
$env:SQLCIPHER_STATIC = "1"
$env:RUSTFLAGS = "-C target-feature=+crt-static"
vcpkg\build-sqlcipher-system.cmd
```

`vcpkg` 디렉터리로 이동해서 실행해도 됩니다.

```powershell
cd vcpkg
.\build-sqlcipher-system.cmd
```

다른 정적 triplet은 첫 인자로 지정할 수 있습니다.

```powershell
vcpkg\build-sqlcipher-system.cmd x64-windows-static
```

```toml
roomrs = { version = "0.3.0", default-features = false, features = ["sqlcipher-system", "async", "live", "time", "uuid", "json"] }
```

위 명령은 release 구성의 정적 라이브러리와 정적 CRT를 조합한 `x64-windows-static-release` community triplet을 사용합니다. Rust도 같은 정적 CRT를 쓰도록 `RUSTFLAGS=-C target-feature=+crt-static`이 필요하며, `VCPKGRS_DYNAMIC`은 설정하지 않습니다. SQLite/SQLCipher DLL도 배포할 필요가 없습니다. 동적 라이브러리 triplet을 별도로 선택한다면 해당 DLL 배포가 애플리케이션 책임입니다.

vcpkg 설치는 최초 한 번만 C/C++를 컴파일하고 이후 Cargo는 `.lib`에 링크합니다. 따라서 `cargo clean`은 vcpkg 산출물을 지우거나 SQLite/SQLCipher를 다시 빌드하지 않습니다. `VCPKG_DEFAULT_BINARY_CACHE`를 공유 경로로 지정하면 다른 checkout과 CI에서도 동일 ABI 결과를 재사용할 수 있습니다. system backend에서 실제로 연결되는 라이브러리 버전, compile option, ABI, 라이선스 수용과 배포 방식은 최종 애플리케이션의 acceptance 대상입니다.

---

## 스키마 스냅샷 워크플로

컴파일 타임 SQL 검증과 자동 마이그레이션의 진실 소스는 **리포에 커밋되는 스키마 스냅샷 파일**입니다.

- 위치·이름: `migrations/schema/[db이름].[버전].json` — db이름은 `#[database]` 구조체명의 snake_case (예: `AppDb` v3 → `app_db.3.json`). 디렉토리는 `ROOMRS_SCHEMA_DIR` 환경 변수로 재지정할 수 있습니다.
- **생성은 명시적입니다.** `cargo test`와 `cargo build`는 스냅샷·migration 파일을 생성하거나 수정하지 않습니다. `cargo roomrs schema export`(또는 `export_schema_snapshot` / `run_registered_schema_export`)로만 현재 version 스냅샷을 생성합니다. 동일 version 파일과 코드가 다르면 **파일을 덮어쓰지 않고** `version` 증가를 안내합니다. export 뒤에는 `cargo build`로 매크로가 스냅샷을 다시 내장하게 하세요.
- `cargo roomrs schema export/check`는 package의 lib·일반 binary target을 custom cfg harness로 컴파일해 `#[database]`를 찾습니다. `src/main.rs`만 있는 앱도 사용자 source 수정 없이 지원합니다. 일반 `cargo test`에는 이 진입점이 포함되지 않습니다.
- 스냅샷이 갱신되면 매크로가 재전개되고(`include_bytes!` 의존성 등록), 전 버전이 압축되어 바이너리에 내장됩니다. `build()`는 내장 스냅샷 해시와 런타임 엔티티 메타 해시를 대조해 스테일이면 명확한 에러를 냅니다.
- **스냅샷 부재 시 `build()`는 기본적으로 실패합니다.** 매크로 전개 때 현재 버전 파일이 없으면 `SnapshotStale`로 시작을 막고, `export_schema_snapshot` 후 재빌드를 안내합니다. 리포 통합 테스트 등 예외만 `ROOMRS_ALLOW_MISSING_SNAPSHOT=1`로 허용합니다. 정적 SQL 파라미터 검증은 스냅샷과 무관하게 항상 수행됩니다.

CLI로 스냅샷을 다룰 수 있습니다:

```
cargo roomrs schema export                              # 전체 #[database] snapshot/안전 SQL 초안 생성
cargo roomrs schema check                               # 전체 #[database] 읽기 전용 검증
cargo roomrs migrate diff <old.json> <new.json> [out.sql]   # 마이그레이션 SQL 초안 생성
cargo roomrs migrate check <a.json> <b.json>                # 스냅샷 해시 비교 (CI용)
cargo roomrs migrate check-dir <schema_dir> <db이름>         # 버전 파일 스캔 — 파스·정합성·파괴적 변경 경고
```

---

## 아키텍처

```
roomrs/
├─ crates/
│  ├─ roomrs/          # 파사드 — 공개 API 재수출만
│  ├─ roomrs-core/     # Database · 자체 통합 read/write 풀 · 에러 · 무효화 트래커 ·
│  │                   #   노티파이어 · 마이그레이션 런너 · SQL/DDL 렌더 · hook
│  ├─ roomrs-async/    # 비동기 파사드 — 런타임 무관 Future/Stream + tokio 통합(선택)
│  ├─ roomrs-macros/   # proc-macro: #[entity] #[dao] #[database] ...
│  ├─ roomrs-migrate/  # SchemaSnapshot · diff · 압축 · 코드젠 (매크로·런타임 공유)
│  └─ roomrs-cli/      # cargo roomrs schema / migrate
├─ examples/           # mobile-ffi 등
└─ xtask/              # 크로스 빌드 태스크
```

의존 방향은 한쪽으로만 흐릅니다: `roomrs → {core, async, macros}`, `macros → migrate`, `async → core`, `core → migrate`. 스냅샷 모델(`roomrs-migrate`)을 매크로(컴파일 타임)와 런타임이 공유하기 때문에 컴파일 타임 검증과 런타임 스테일 감지가 같은 타입 위에서 동작합니다.

동시성의 핵심은 **일반 커넥션 N개로 구성된 통합 read/write 미니 풀**입니다. 모든 일반 커넥션에서 읽기와 쓰기가 가능하고, checkout guard가 한 커넥션을 한 작업에 독점 대여합니다. `query`와 `execute`는 읽기·쓰기 권한이 아니라 결과 행을 소비하는지 여부로 구분하므로 `INSERT ... RETURNING`, CTE, 쓰기 PRAGMA도 SQL 라우팅 없이 실행됩니다. 트랜잭션은 checkout한 같은 커넥션을 유지하며 `BEGIN IMMEDIATE`로 시작합니다. WAL과 `busy_timeout`이 잠금 경합을 조정합니다. 무효화의 주 경로는 문장 기반(실행 SQL의 대상 테이블 확정 → commit 성공 후 방출)이고, 모든 일반 커넥션에 설치된 `preupdate_hook`은 트리거 간접 write와 행 필터 매칭을 처리하는 보조 경로입니다.

---

## 로깅

roomrs 라이브러리는 [`log`](https://crates.io/crates/log) 파사드로만 영어 로그를 방출하며 subscriber를 초기화하지 않습니다. `error`는 작업 실패와 풀 fatal 상태, `warn`은 복구·재시도·파괴적 변경, `info`는 DB·풀·노티파이어·스냅샷 lifecycle, `debug`는 마이그레이션 계획·checkout 대기·구독 상태, `trace`는 SQL 구조·checkout·async 작업·무효화 상세를 기록합니다. SQL 파라미터, 암호화 키, 행 데이터는 기록하지 않습니다.

CLI와 저장소의 모든 실행 예제는 `tracing-log` 브리지와 `tracing-subscriber`를 초기화합니다. `RUST_LOG`로 상세도를 바꿀 수 있습니다:

```powershell
$env:RUST_LOG = "info,roomrs_core=trace,roomrs_async=debug,roomrs_migrate=debug"
cargo run --example live_query
```

소비자 애플리케이션에서 tracing을 사용한다면 같은 브리지를 설치합니다:

```rust
/// log → tracing 브리지 초기화 —
/// roomrs는 log 파사드로만 방출한다(구독자 초기화는 소비자 몫).
fn init_tracing() {
    // 1) log → tracing 변환기 설치 (전역 log 로거)
    tracing_log::LogTracer::init().expect("LogTracer 초기화 실패");
    // 2) fmt 구독자를 RUST_LOG 필터로 설치 —
    //    fmt().init()은 LogTracer를 중복 설치하려다 실패하므로 set_global_default 사용
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("roomrs_core=debug"));
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("tracing 구독자 설치 실패");
}
```

debug 레벨을 켜면 커넥션 오픈, 트랜잭션 begin/commit/rollback, 무효화 이벤트 등 내부 동작이 보입니다. 동작 예시는 `cargo run --example live_query`로 확인할 수 있습니다.

---

## 플랫폼 · MSRV · 크로스 빌드

- **MSRV 1.85** (Edition 2024) — CI 매트릭스에 포함되어 있습니다.
- 데스크톱(Windows/macOS/Linux) 3 OS에서 테스트하며, 모바일(Android/iOS)은 FFI 패턴으로 지원합니다.
- 기본 bundled SQLite는 시스템 라이브러리 설치 없이 C 컴파일러만 필요합니다. SQLCipher bundled 구성은 추가로 Perl이 필요하며, system backend는 위의 OS package 또는 vcpkg 설치가 필요합니다.

Windows 호스트에서 zig/NDK로 크로스 빌드합니다. 기본 구성에서는 bundled SQLite(C)를 함께 컴파일합니다. SQLCipher를 선택하면 OpenSSL(C)도 추가로 컴파일됩니다.

```
cargo xtask cross-linux      # x86_64/aarch64-linux-gnu + x86_64-musl(정적 CLI) — zig
cargo xtask cross-android    # arm64-v8a / armeabi-v7a / x86_64 .so — cargo-ndk
cargo xtask cross-all
```

| 타깃 | 도구 | 상태 |
|---|---|---|
| Windows x64 (호스트) | MSVC | ✅ 빌드+테스트 |
| Linux x64 / arm64 (gnu) | zig (cargo-zigbuild) | ✅ 빌드 |
| Linux x64 (musl, 정적) | zig | ✅ 빌드 |
| Android arm64 / armv7 / x86_64 | NDK (cargo-ndk) | ✅ 빌드 (.so) |
| iOS / macOS | Xcode 필요 | ⬜ macOS 호스트에서 후속 |

도구 설치와 제약(Android는 zig 단독 불가 — bionic은 NDK에만 있음)은 [docs/cross-build.md](docs/cross-build.md)를 참고하세요.

---

## 기여하기

기여를 환영합니다! 버그 리포트, 문서 개선, 기능 제안 모두 좋습니다.

### 개발 환경

Rust 1.85 이상만 있으면 됩니다. SQLite는 번들로 컴파일되므로 별도 설치가 필요 없습니다.

```
git clone https://github.com/yongaru/roomrs
cd roomrs
cargo test --workspace
```

### PR 전 체크 (CI와 동일한 게이트)

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features tokio -- -D warnings
cargo test --workspace
cargo test --workspace --features tokio
```

### 컨벤션

- **명세가 진실 소스(SSOT)입니다.** 공개 API에 영향을 주는 변경은 [roomrs-개발계획서.md](roomrs-개발계획서.md)(특히 §0 결정 로그)와 먼저 맞춰야 합니다. 명세와 코드가 충돌하면 명세를 갱신하는 논의가 선행됩니다.
- 코드 주석과 에러 메시지는 **한국어**, 공개 rustdoc(`///`)은 **영어**로 작성합니다(crates.io 공개용).
- 커밋 메시지는 [Conventional Commits](https://www.conventionalcommits.org/) 형식을 따르고, 스코프는 크레이트명입니다(예: `feat(core): ...`, `fix(macros): ...`).
- 동작 변경에는 테스트가 따라야 합니다. 매크로 컴파일 실패 케이스는 trybuild(`tests/ui/`)로 다룹니다.
- 테스트 DB는 `:memory:` 또는 tempfile을 사용하세요 — **리포 안에 `.db` 파일을 만들지 않습니다.**


### 이슈 리포팅

버그를 발견하면 재현 절차, roomrs 버전, OS/타깃, 관련 로그(가능하면 debug 레벨)를 함께 남겨 주세요. 재현 가능한 최소 예제가 있으면 가장 좋습니다.

---

## 라이선스

다음 중 원하는 라이선스를 선택해 사용할 수 있습니다:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) 또는 http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) 또는 http://opensource.org/licenses/MIT)

별도로 명시하지 않는 한, 여러분이 이 프로젝트에 의도적으로 제출한 기여는 Apache-2.0 라이선스가 정의하는 바에 따라 위와 같이 듀얼 라이선스되며, 추가 조건은 없습니다.
