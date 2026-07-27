# roomrs

**한국어** | [English](README-en.md)

[![CI](https://github.com/yongaru/roomrs/actions/workflows/ci.yml/badge.svg)](https://github.com/yongaru/roomrs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/roomrs.svg)](https://crates.io/crates/roomrs)
[![docs.rs](https://img.shields.io/docsrs/roomrs)](https://docs.rs/roomrs)
[![MSRV](https://img.shields.io/badge/MSRV-1.85-informational)](#지원-환경)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#라이선스)

Android Room처럼 엔티티, DAO, SQL을 선언해 사용하는 Rust용 SQLite 퍼시스턴스 라이브러리입니다.

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

## 왜 roomrs인가요?

roomrs는 서버용 범용 ORM이 아니라 데스크톱·모바일 애플리케이션의 **로컬 SQLite 데이터**에 집중합니다.

- 구조체를 테이블로, trait를 DAO로 선언합니다.
- SQL의 테이블·컬럼·명명 파라미터를 컴파일 타임에 검사합니다.
- 데이터가 바뀌면 다시 조회해 알려 주는 `LiveQuery<T>`를 제공합니다.
- 동기 API와 런타임 무관 비동기 API를 같은 DAO에서 생성합니다.
- 버전별 스키마 스냅샷을 바이너리에 내장하고 forward migration을 검증합니다.
- SQLite·SQLCipher의 bundled/system 링크 방식을 선택할 수 있습니다.

PostgreSQL·MySQL 같은 서버 DB, Active Record, 다중 DB 추상화가 필요하면 Diesel이나 SeaORM이 더 적합합니다. Android Room과 비슷한 선언형 로컬 저장소가 필요하면 roomrs가 맞습니다.

## 빠른 시작

### 1. 프로젝트와 도구 준비

```sh
cargo new roomrs-example
cd roomrs-example
cargo add roomrs@0.4.0
cargo install roomrs-cli
```

기본 feature는 SQLite를 소스와 함께 빌드하므로 시스템 SQLite 설치가 필요 없습니다.

### 2. `src/main.rs` 작성

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
        title: "roomrs 시작하기".into(),
        done: false,
    })?;
    println!("새 ID: {id}");

    for todo in dao.by_done(false)? {
        println!("- {}", todo.title);
    }
    Ok(())
}
```

### 3. 스키마 생성 후 실행

```sh
cargo roomrs schema export
cargo build
cargo run
```

`schema export`는 `migrations/schema/app_db.1.json`을 만듭니다. 다음 `cargo build`가 이 스냅샷을 컴파일 타임 SQL 검증과 런타임 스키마 검증에 사용하도록 바이너리에 내장합니다. `cargo build`와 `cargo test`는 스키마 파일을 자동 수정하지 않습니다.

엔티티를 변경했다면 version을 올리거나 `version = auto`를 사용한 뒤 다시 `schema export → build` 순서로 실행합니다. migration SQL 초안이 생성되었다면 적용 전에 반드시 검토해야 합니다.

전체 과정과 실패 시 조치 방법은 [상세 사용 가이드](docs/USAGE.md#스키마를-변경할-때)를 참고하세요.

## SQL 문자열로 바로 조회

DAO 메서드를 선언하지 않고 SQL 문자열을 바로 실행할 수도 있습니다. `#[entity]`가 `FromRow` 구현을 자동 생성하므로 엔티티 타입만 지정하면 됩니다.

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
    "SELECT id, title, done FROM todos WHERE done = ?1 ORDER BY id",
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

`query_one`은 `SELECT`뿐 아니라 `INSERT`, `UPDATE`, `DELETE ... RETURNING` 한 행도 `Todo`로 반환합니다. `query_optional`은 `Option<Todo>`, `query_all`은 `Vec<Todo>`, `query_scalar`는 단일 컬럼 값, `execute`는 변경된 행 수를 반환합니다. `Row<Todo>` 같은 wrapper를 반환하지 않습니다. generic 인자 `_`는 SQL 파라미터 타입을 Rust가 추론하도록 둔 것입니다.

## 엔티티가 아닌 SELECT 결과 구조체

테이블 전체를 나타내는 `#[entity]`만 조회 결과로 사용할 수 있는 것은 아닙니다. JOIN, 집계, 일부 컬럼 projection처럼 뷰에 가까운 결과는 일반 구조체에 `FromRow`를 구현해 받을 수 있습니다.

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

`TodoListItem`에는 `#[entity]`가 필요하지 않으며 `#[database(entities(...))]`에도 등록하지 않습니다. 따라서 새 테이블이나 스키마가 생성되지 않습니다. 생성된 DAO 접근자를 사용하려면 `TodoViewDao`만 `daos(...)`에 등록합니다. 같은 구조체를 `run_sync().query_all(...)`, 비동기 직접 조회, `UPDATE`·`DELETE ... RETURNING` 결과에도 사용할 수 있습니다. 자세한 DAO·직접 조회 예제는 [임의 SELECT 결과 구조체](docs/USAGE.md#임의-select-결과-구조체)를 참고하세요.

## 주요 기능

| 기능 | 설명 |
|---|---|
| 스키마 DSL | 단일·복합 PK, DEFAULT, UNIQUE, CHECK, FK, 정렬·부분 index, generated column, custom SQL type, DB-level inline/file trigger |
| DAO | `#[query]`, `#[insert]`, `#[update]`, `#[delete]`, `#[transaction]` |
| SELECT 결과 구조체 | `FromRow`를 구현한 일반 구조체로 JOIN·집계·projection 결과 매핑 |
| SQL 검증 | 스키마 스냅샷 기반 테이블·컬럼 검사와 `:name` 파라미터 대조 |
| LiveQuery | commit 이후 자동 재조회, 행 필터, observer debounce, 동기·Stream 소비 |
| 트랜잭션 | DAO transaction, 클로저 transaction, 중첩 savepoint, RAII rollback |
| 관계 | 1:1, 1:N, N:M 관계 뷰와 일괄 로딩 |
| 마이그레이션 | SQL·Rust 코드 step, 디렉터리 임베드, 안전 diff 자동 실행 |
| 비동기 | tokio에 종속되지 않는 `Future + Send`; tokio·smol·async-std 등에서 실행 |
| 보안 DB | bundled/system SQLite 또는 SQLCipher 선택 |

Android Room 사용자라면 [Room과 roomrs 대응표](docs/USAGE.md#android-room과의-대응)를 먼저 보면 빠릅니다.

## 문서

| 문서 | 내용 |
|---|---|
| [상세 사용 가이드](docs/USAGE.md) | 설치부터 CRUD, LiveQuery, transaction, 관계, migration, 운영까지 |
| [API 문서](https://docs.rs/roomrs) | 공개 타입과 함수 |
| [예제](crates/roomrs/examples/) | 실행 가능한 동기·비동기·관계·migration 예제 |
| [로드맵](ROADMAP.md) | 공개 기능 우선순위와 제외 범위 |
| [변경 기록](CHANGELOG.md) | 배포 버전별 변경 사항 |
| [개발계획서](roomrs-개발계획서.md) | 내부 구현 계약과 설계 결정 |
| [반환 불가 경로 정책](docs/RETURN_UNAVAILABLE.md) | Drop·스레드·callback의 크래시 방지 정책 |

## 기능 플래그

기본 구성:

```toml
roomrs = "0.4.0"
```

기본 feature는 `sqlite-bundled`, `async`, `tokio`, `live`, `time`, `uuid`, `json`입니다.

순수 동기 최소 구성:

```toml
roomrs = {
    version = "0.4.0",
    default-features = false,
    features = ["sqlite-bundled"]
}
```

SQLCipher:

```toml
roomrs = {
    version = "0.4.0",
    default-features = false,
    features = ["sqlcipher-bundled", "async", "tokio", "live", "time", "uuid", "json"]
}
```

`sqlite-bundled`, `sqlite-system`, `sqlcipher-bundled`, `sqlcipher-system` 중 정확히 하나를 선택해야 합니다. SQLCipher를 선택해도 `.encryption_key(...)`를 지정하지 않으면 데이터베이스가 자동으로 암호화되지 않습니다.

각 feature와 Windows vcpkg 설정은 [설치와 backend 선택](docs/USAGE.md#설치와-backend-선택)을 참고하세요.

## 현재 제약

- SQLite 전용입니다.
- 다중 프로세스에서 다른 프로세스가 수행한 write는 LiveQuery가 관찰하지 않습니다.
- 비동기 transaction은 동기 클로저를 worker에서 실행합니다. 클로저 내부 `.await`는 지원하지 않습니다.
- 자동 migration은 안전하다고 판정한 forward 연산만 실행합니다. 파괴적 변경과 데이터 변환은 수동 migration이 필요합니다.
- 새 스키마 snapshot을 export한 뒤에는 새 파일을 바이너리에 내장하도록 `cargo build`를 다시 실행해야 합니다.

자세한 경계와 권장 대안은 [상세 사용 가이드](docs/USAGE.md#알려진-제약)를 참고하세요.

## 예제 실행

```sh
cargo run -p roomrs --example todo_sync
cargo run -p roomrs --example todo_async
cargo run -p roomrs --example live_query
cargo run -p roomrs --example transactions
cargo run -p roomrs --example migrations
cargo run -p roomrs --example relations
```

전체 목록은 [예제 안내](docs/USAGE.md#실행-가능한-예제)에 있습니다.

## 지원 환경

- Rust 1.85 이상, Edition 2024
- Windows, macOS, Linux
- Android·iOS는 Rust `cdylib` FFI 패턴으로 사용
- 기본 bundled SQLite는 C 컴파일러 필요
- bundled SQLCipher는 C 컴파일러와 Perl 필요

CI는 Windows·macOS·Linux, MSRV, backend별 feature 조합을 검사합니다. 크로스 빌드 명령은 [플랫폼과 크로스 빌드](docs/USAGE.md#플랫폼과-크로스-빌드)를 참고하세요.

## 기여하기

버그 보고에는 다음 정보를 포함해 주세요.

- roomrs와 Rust 버전
- 활성화한 feature
- OS와 SQLite/SQLCipher 링크 방식
- 재현 가능한 최소 코드
- 실제 오류와 기대 동작

개발 환경과 전체 검증 게이트는 [기여 가이드](docs/USAGE.md#기여와-개발)를 참고하세요. 기능 방향은 구현 전에 [ROADMAP.md](ROADMAP.md)와 [개발계획서](roomrs-개발계획서.md)를 확인합니다.

## 라이선스

다음 라이선스 중 하나를 선택해 사용할 수 있습니다.

- [Apache License 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)
