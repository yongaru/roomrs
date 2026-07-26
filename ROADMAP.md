# roomrs 로드맵

이 문서는 공개 기능 방향과 우선순위를 설명합니다. 확정된 구현 계약은
[개발계획서](roomrs-개발계획서.md)를, 이미 배포된 변경은
[CHANGELOG](CHANGELOG.md)를 참고하세요.

상태: ✅ 구현됨 · ⬜ 미구현/탐색 · 🚫 의도적으로 제외

**한눈에 보기:** [P1 SQLite 스키마 DSL](#우선순위-1--sqlite-스키마-dsl-확장) ✅ · [P2 스키마 스냅샷 안전망](#우선순위-2--스키마-스냅샷-안전망) ✅ · [P3 스키마 생성·마이그레이션](#우선순위-3--스키마-생성마이그레이션-흐름) ✅

## 우선순위 1 — SQLite 스키마 DSL 확장

아래 문법은 목표 사용 경험을 보여 주는 예시이며, 실제 공개 API는 구현 전 개발계획서의 결정 로그로 확정합니다.

### ✅ 복합 기본 키

여러 필드에 `#[pk]`를 붙이거나 `#[entity(primary_key(...))]`에 필드 목록을 쓰면 지정 순서대로 SQLite table-level primary key를 생성합니다.

```rust
#[entity(table = "t_payment")]
struct Payment {
    #[pk]
    store_id: String,
    #[pk]
    payment_id: String,
}
```

같은 키를 엔티티 수준에서 선언할 수도 있습니다.

```rust
#[entity(
    table = "t_payment",
    primary_key(store_id, payment_id)
)]
struct Payment {
    store_id: String,
    payment_id: String,
}
```

```sql
PRIMARY KEY ("store_id", "payment_id")
```

`#[pk(autoincrement)]`는 SQLite의 단일 `INTEGER PRIMARY KEY` 전용 기능이므로 다른 `#[pk]`와 함께 사용하면 컴파일 오류입니다.

두 PK 표기를 함께 쓰면 필드와 순서가 정확히 같아야 합니다. 다르면 `schema export/check`가 파일을 쓰기 전에 configuration 오류와 수정 조언을 반환합니다.

### ✅ Table-level UNIQUE

단일 필드 `#[column(unique)]` 외에 여러 컬럼 조합의 유일성을 선언합니다.

```rust
#[entity(unique(store_id, external_payment_id))]
struct Payment { /* ... */ }
```

기존 데이터에 중복이 있으면 UNIQUE 생성은 실패할 수 있으므로, 기존 DB 적용은 수동 migration을 우선합니다.

### ✅ 복합·정렬·partial index

조회 조건과 정렬 순서를 반영하는 index를 엔티티 수준에서 선언합니다.

```rust
#[entity(
    index(name = "idx_payment_store_created", columns(store_id, created_at desc)),
    index(name = "idx_payment_active", columns(store_id), where = "deleted_at IS NULL")
)]
struct Payment { /* ... */ }
```

- 복합 index는 컬럼 순서를 보존합니다.
- `ASC`/`DESC` 정렬을 지원합니다.
- partial index의 `where`는 SQLite SQL 조건 문자열입니다.
- 일반 SQLite index의 저장 구조는 B-tree로 고정입니다. index 알고리즘 선택 DSL은 제공하지 않습니다.
- 일반 index 생성은 자동 migration 후보지만, 기존 데이터를 제한하는 UNIQUE index는 수동 migration이 기본입니다.

전문 검색 FTS5와 공간 검색 R*Tree는 일반 index가 아닌 virtual table입니다. 별도 DSL을 설계하기 전에는 수동 SQL migration으로 관리합니다.

### ✅ 복합 foreign key

복수 자식 컬럼과 부모 컬럼의 순서를 1:1로 연결합니다.

```rust
#[entity(
    foreign_key(
        columns(store_id, customer_id),
        references = "customers(store_id, customer_id)",
        on_delete = "CASCADE",
        on_update = "NO ACTION"
    )
)]
struct Payment { /* ... */ }
```

부모 참조 컬럼은 primary key 또는 UNIQUE 제약으로 보장돼야 합니다. 기존 SQLite table에 foreign key를 추가하려면 대개 table 재생성이 필요하므로 수동 migration 범위입니다.

### ✅ CHECK 제약

행 값 규칙을 SQLite가 강제하도록 table-level CHECK를 선언합니다.

```rust
#[entity(check = "amount >= 0")]
struct Payment { /* ... */ }
```

CHECK 본문은 SQLite SQL 표현식입니다. 기존 table에 추가·변경하려면 table 재생성이 필요할 수 있으므로 수동 migration을 사용합니다.

### ✅ Custom SQL column type

Rust 타입의 `ToSql`/`FromSql` 구현은 유지하면서 DDL의 SQLite type name을 명시합니다.

```rust
struct Money(i64);

#[entity]
struct Payment {
    #[column(sql_type = "DECIMAL(12,2)")]
    amount: Money,
}
```

SQLite에서는 type name이 affinity에 영향을 주지만 고정소수점 저장 의미를 보장하지 않습니다. 실제 직렬화·역직렬화 책임은 `ToSql`/`FromSql` 구현에 있습니다.

### ✅ Trigger SQL file hook

trigger 본문은 attribute 문자열 대신 versioned SQL 파일로 관리합니다. 엔티티는 hook 파일을 참조하고, 파일 내용은 schema snapshot·diff 입력에 포함합니다.

```text
migrations/triggers/t_payment_audit.sql
```

trigger 생성·변경·삭제는 데이터 동작에 직접 영향을 주므로 수동 forward migration으로만 적용합니다. 파일 경로와 내용 hash를 snapshot에 포함해 SQL 검증과 변경 감지를 일치시킵니다.

### ✅ 공통 구현 원칙

각 DSL 항목은 다음을 함께 지원해야 합니다.

- proc-macro 입력 검증과 원인 span 컴파일 오류
- SQLite DDL 렌더
- schema snapshot 직렬화·바이너리 내장·hash 검증
- `cargo roomrs migrate diff`의 안전/수동/파괴 변경 분류
- 신규 DB 생성과 기존 DB forward migration 테스트

자동 migration은 CREATE TABLE, 안전한 ADD COLUMN, 일반 CREATE INDEX처럼 데이터 의미를 바꾸지 않는 연산으로 제한합니다. rename, type 변경, PK/FK/CHECK/UNIQUE/trigger 변경과 데이터 변환은 명시적 SQL 또는 code migration이 필요합니다.

## 우선순위 2 — 스키마 스냅샷 안전망

- ✅ 버전별 스키마 snapshot을 불변으로 관리합니다. 확정된 snapshot은 덮어쓰지 않습니다.
- ✅ 엔티티 DDL이 바뀌면 `#[database(version = N)]`의 version을 올리고 새 snapshot을 생성합니다.
- ✅ `cargo test`와 `cargo build`는 schema JSON·migration SQL을 생성하거나 수정하지 않습니다. snapshot 부재·불일치는 읽기 전용 검사로 실패합니다.
- ✅ custom cfg harness의 `cargo roomrs schema check`로 모든 DB snapshot을 읽기 전용 검사합니다.
- ✅ 현재 version snapshot이 누락됐거나 내장 hash가 엔티티 메타와 다르면 앱 시작을 실패시킵니다.
- ✅ CI는 `cargo roomrs schema check`를 먼저 실행한 뒤 `cargo test --workspace`와 build를 실행합니다.
- ✅ Rust 필드 또는 SQL column rename은 `#[column(renamed_from = "old_name")]`를 명시해야 합니다. 힌트가 없으면 삭제 후 추가로 오인해 데이터 보존 migration이 깨질 수 있습니다.

## 우선순위 3 — 스키마 생성·마이그레이션 흐름

- ✅ `Migration::sql`, `Migration::code`, `migrations_dir!` 기반의 수동 forward migration을 유지합니다.
- ✅ 기본 version 모드는 사람이 명시하는 `version = N`입니다.
- ✅ `version = auto`를 수동 version 모드와 함께 제공합니다. 명시 version 모드는 제거하지 않습니다. 변경 시 다음 snapshot과 forward migration SQL 초안을 함께 생성합니다.
- ✅ 프로젝트 전체 `#[database]`를 자동 등록·탐색하는 `cargo roomrs schema export`를 제공합니다.
- ✅ 이 JSON은 컴파일 타임 SQL 검증, 실행파일 내 과거 schema 내장, snapshot diff 기반 migration 판단의 공통 입력입니다.
- ✅ schema export는 Cargo 빌드 재진입을 일으키므로 소비자 앱의 `build.rs`에서 실행하지 않습니다.
- ✅ 안전한 CREATE TABLE, ADD COLUMN, CREATE INDEX는 `.auto_migrate(true)`에서 계속 opt-in으로 자동 처리합니다.
- 🚫 down migration과 파괴적 변경의 자동 실행은 제공하지 않습니다.

### ✅ 프로젝트 schema export·`version = auto` 구현 계약

사용 흐름은 다음 두 명령으로 고정합니다.

```powershell
cargo roomrs schema export
cargo build
```

`export`는 명시적으로 파일 쓰기를 허용한 유일한 명령입니다. `cargo test`·`cargo build`·앱 실행은 파일을 쓰지 않습니다.

- **대상 탐색:** `#[database]` 매크로가 export registry entry와 custom `cfg(roomrs_export)` 진입점을 생성합니다. CLI는 lib·일반 bin target을 stable test harness로 실행해 등록 DB를 탐색합니다. 사용자 source 추가나 DB 타입 수동 나열은 필요하지 않습니다.
- **package 범위:** 현재 package가 기본입니다. workspace에서는 `--package <name>`으로 하나를, `--workspace`로 `#[database]`가 있는 모든 package를 선택합니다. DB가 없는 package는 건너뜁니다. 앱의 `main()`은 실행하지 않습니다.
- **수동 version:** `version = N`의 `[db].N.json`이 없으면 생성합니다. hash가 같으면 no-op입니다. hash가 다르거나 파일이 파손되면 아무 파일도 덮어쓰지 않고 `version` 증가를 요구합니다.
- **자동 version:** `#[database(version = auto)]`는 DB별 마지막 JSON과 엔티티 hash가 같으면 no-op입니다. 다르면 다음 정수 version JSON과 `migrations/{from}_{to}_roomrs_auto.sql` forward migration 초안을 만듭니다. JSON이 하나도 없으면 version 1부터 시작하며 migration SQL은 만들지 않습니다. export 뒤 `cargo build`가 새 version을 내장합니다.
- **원자성:** 모든 DB·기존 snapshot·생성할 migration 파일명을 먼저 검사합니다. 하나라도 파손·수동 version 충돌·migration 파일 충돌이면 새 JSON·SQL을 하나도 쓰지 않습니다. 검사를 통과한 뒤에만 누락·다음 version 파일과 SQL 초안을 생성합니다.
- **검증:** `cargo roomrs schema check`는 같은 registry를 사용하되 읽기만 합니다. CI는 check 후 test·build를 실행합니다.
- **migration:** auto version은 `diff_sql` 기반 forward migration SQL 초안을 자동 생성합니다. 안전 SQL은 포함하고 파괴적·모호 변경은 TODO로 남깁니다. TODO가 있으면 export는 비성공으로 끝나며 사용자가 보완·검토해야 합니다. 자동 실행은 하지 않습니다. 수동 version의 migration은 기존 `Migration::sql`, `Migration::code`, `migrations_dir!`, `cargo roomrs migrate diff` 경로를 유지합니다.
- **export 전용 compile:** 새 column을 쓰는 `#[query]`가 이전 snapshot 때문에 export 자체를 막지 않도록, export subcommand가 설정한 tracked 환경에서 snapshot 기반 SQL 대조만 건너뜁니다. SQL 파라미터·attribute 검증은 계속 실패합니다.

## 완료 항목

- ✅ LiveQuery filter API 대칭·DB 전역 `.live_debounce` / observer `.debounce` 250ms 고정 coalesce
- ✅ LiveQuery 재조회 `roomrs-live-worker` pool (통합 풀 checkout, `.notifier_readers`)
- ✅ LiveQuery 필터 스키마 검증 (`Error::InvalidationFilter`)
- ✅ LiveQuery 공개 관측성 metrics (`Database::live_metrics`)
- ✅ LiveQuery 다중 `InvalidationFilter` OR 매칭 (filter 간 AND·중첩 boolean expression 제외)
- ✅ 기존 schema DSL의 snapshot·hash·diff·auto migration 정합 (`default_sql`, `NOT NULL DEFAULT` 안전 ADD)
- ✅ SQLite schema DSL: `collate`, generated column, `strict`, `without_rowid`, index column `COLLATE`

## 제외 범위

- 🚫 기존 DB의 table·column을 자동 삭제하지 않습니다.
- 🚫 다른 데이터베이스 백엔드를 지원하지 않습니다.
- 🚫 VIEW·FTS5·R*Tree 전용 DSL은 제공하지 않습니다. VIEW는 직접 query와 결과 struct, 필요 시 수동 migration SQL을 사용합니다.
