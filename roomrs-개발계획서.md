# roomrs — 0.3.0 기능 명세 (Agent Spec) · **SQLite 전용 · 동기+비동기**

> Android **Room** 과 동일한 개발 경험을 목표로 하는 Rust용 **로컬 SQLite 퍼시스턴스** 라이브러리.
> 백엔드: **SQLite 전용 — SQLite/SQLCipher bundled/system 링크 선택, 기본은 bundled SQLite**. 다른 DB 지원 없음. API: **동기 1급 + 비동기(런타임 무관 std Future + tokio 통합)**. 기반: **rusqlite** 동기 코어 + **자체 통합 미니 풀(N)**.
>
> 본 문서는 **에이전트/구현자 실행용 명세**다. 사람용 개요는 `README.md` 참조.
> 제품 버전: **0.3.0**.
>
> **범위 확정**: roomrs는 **SQLite만** 지원한다. 다른 DB 백엔드는 지원하지 않으며 계획에도 없다.
>
> **기반 선택 근거**: SQLite는 C 레벨에서 동기라 어떤 라이브러리든 "async"는 워커 오프로드일 뿐 → **동기 코어(rusqlite) + 비동기 파사드**. 일반 커넥션은 모두 read/write 가능하게 해 SQL 의미 추측과 `RETURNING`·CTE·쓰기 PRAGMA 예외를 제거하고, SQLite WAL·busy handler가 실제 잠금을 조정한다. 비동기 파사드는 **런타임 무관(std `Future`)**이며 tokio 통합은 기본 on, 선택 해제 가능하다(§2.4).

---

## 0. 결정 로그 (Decision Log) — 확정 사항

이 표는 구현 시 **변경 금지 기준선**이다. 변경 시 이 표를 먼저 갱신하고 근거를 남긴다.

| # | 항목 | 결정 | 비고 |
|---|------|------|------|
| 1 | 기반 스택 | **rusqlite 동기 코어 + 자체 미니 풀** | SQLite/SQLCipher 각각 bundled/system 링크 선택. 기본은 bundled SQLite |
| 1b | 쓰기 전략 | **통합 풀 N — 모든 일반 커넥션 read/write 가능, SQL 라우팅 없음** | `query`/`execute`는 반환 형태만 구분. WAL·`busy_timeout`으로 잠금 경합 처리 |
| 2 | API 모델 | **동기 1급 + 비동기 파사드(런타임 무관 std Future, tokio 통합 기본)** | 어떤 실행기에서도 await 가능. `default-features = false`로 tokio 제외 가능 |
| 2b | API 네임스페이스 | **`db.run_sync()` / `db.run_async()` 핸들 분리** — 양쪽 메서드명 동일 | 완전 대칭 쌍 + 예약어 `async` 충돌 회피(§5.0) |
| 3 | 쿼리 정의 | **Room식 raw SQL 매크로** (`#[query("...")]`) | 동적 쿼리는 쿼리빌더/런타임 경로 |
| 3b | 직접 쿼리 | **DAO/엔티티 없이 raw 쿼리 1급** (execute/단건/Option/스칼라/Vec + 라이브) | §5.7 |
| 4 | SQL 검증 | **하이브리드**: 정적=**커밋된 스키마 스냅샷 파일 대조**, 동적=런타임(prepare) | 메커니즘 §7. 파서 실패=스킵+경고, `unchecked` 해치 |
| 5 | 라이브 쿼리 | **핵심 필수** — 단일 구체 타입 `LiveQuery<T>` | 주 경로=문장 기반 무효화, 보조=`preupdate_hook` 행 필터(§9.2) |
| 5b | 멀티 프로세스 무효화 | **현재 미지원**, 공개 로드맵 제외 | 단일 프로세스 `preupdate_hook` 필터만 제공. 보관 설계는 `docs/MULTI_PROCESS_INVALIDATION.md` |
| 5c | 알림 콜백 모델 | **recv + subscribe 콜백 + Iterator / Stream** — 동기·비동기 핸들 양쪽 | 전용 노티파이어 스레드(§9.6) |
| 6 | 마이그레이션 | **자동 + 반자동(직접 SQL) + 수동 코드** 3경로 | diff 초안 · `Migration::sql` · trait(§8) |
| 6b | 마이그레이션 보조 | **rename 힌트**(`renamed_from`) + **`fallback_to_destructive_migration`**(옵트인) | |
| 7 | 관계 매핑 | **풀지원** (1:1/1:N/N:M, embedded) | 관계 로딩은 자동 트랜잭션 래핑 |
| 7b | 부가 데이터 기능 | **`#[transaction]`**(tx-바운드 DAO 방식) | §5.9 |
| 7c | 텍스트 검색 | **`LIKE` 검색** (별도 검색 엔진 없음) | raw SQL + 동적 쿼리빌더 `like`/`like_escaped` 지원(§5.8) |
| 8 | 백엔드 범위 | **SQLite 전용** | 다른 DB 지원 없음(계획에도 없음) |
| 9 | 스키마 정의 | **코드퍼스트 + SQL퍼스트 둘 다** | 코드퍼스트 우선 |
| 10 | 트랜잭션 | 동기=클로저+RAII / **비동기=클로저형만** — checkout한 같은 커넥션에서 `BEGIN IMMEDIATE`부터 종결까지 실행 | async RAII는 취소-롤백 문제로 제공하지 않음 |
| 11 | 커넥션 풀 | **자체 통합 미니 풀 N**: checkout guard가 커넥션 하나를 독점, 모든 일반 커넥션 read/write 가능 | `query_only`와 SQL 첫 키워드 라우팅 없음 |
| 12 | 타입 컨버터 | **trait 구현 + serde(`#[json]`) 병행** | rusqlite `ToSql`/`FromSql` 위임 |
| 12b | 필드 속성 표기 | 무시 필드는 **`#[column(ignore)]`** | `#[pk]`/`#[json]`과 내장 속성 충돌 방지 |
| 12c | autoincrement PK | insert 시 **PK 컬럼 항상 생략 + 새 id는 반환값으로** | `id:0` 센티널 금지. 명시 삽입은 `#[insert(keep_pk)]` |
| 13 | 플랫폼 | 데스크톱(Win/Mac/Linux) + 모바일(Android/iOS) | 전 타깃 SQLite |
| 13b | MSRV | **1.85 (Edition 2024)** | Rust 1.88 안정화 let-chain 금지. CI `cargo +1.85 build`를 릴리스 게이트로 포함 |
| 14 | 암호화 | SQLCipher opt-in backend | `sqlcipher-bundled` 또는 `sqlcipher-system` 선택. 암호화 키 미설정 시 평문 DB도 사용 가능 |
| 15 | SQLite 링크 | canonical backend feature 4종 중 최대 하나: `sqlite-bundled`, `sqlite-system`, `sqlcipher-bundled`, `sqlcipher-system` | 기본은 `sqlite-bundled`. `bundled`/`cipher` alias와 core의 backend 없는 빌드는 유지 |
| 15b | 기본 feature | `sqlite-bundled`, `async`, `tokio`, `live`, `time`, `uuid`, `json` | backend alias와 다른 canonical backend는 기본 목록에서 제외 |
| 16 | 에러 모델 | thiserror 단일 `roomrs::Error` + 구조화 `ErrorPath`·`ErrorAdvice` | `Error` variant 호환 유지. 모든 오류는 원인 하위 시스템과 권장 조치 제공(§12) |
| 16b | 운영 훅 | `.on_create/.on_open/.query_logger` | Room Callback/QueryCallback 대응 |
| 17 | 라이선스 | MIT / Apache-2.0 듀얼 | |
| 18 | 배포 | crates.io 공개 | 최초 공개 버전은 `0.1.0` |
| 19 | 문서 범위 | 풀 기능 아키텍처 설계서 | 0.1.0 전체 기능 기준 |
| 20 | 이름 | **roomrs** | |
| 21 | 스냅샷 저장 모델 | **버전별 파일 `[db이름].[버전].json`**(고정 디렉토리 `migrations/schema/`) | db이름 = `#[database]` 구조체명 snake_case(§7.2) |
| 21b | 스냅샷 생성·검증 | custom `cfg(roomrs_export)` harness는 명시 CLI에서만 활성화 | 생성은 명시 export API·registry가 담당한다. proc-macro 제약은 §7.1 참조 |
| 21c | 스냅샷 내장 | 매크로가 존재하는 전 버전 스냅샷을 **압축(miniz_oxide)해 바이너리에 내장** + `include_bytes!`로 rustc 파일 의존성 등록 | §7.4/§8.4 |
| 21d | 내장 자동 마이그레이션 | builder `.auto_migrate(true)`(옵트인) — 등록 스텝 없는 구간을 내장 스냅샷 diff의 **안전 연산**(CREATE TABLE/ADD COLUMN/CREATE INDEX)으로 자동 실행. 파괴적 구간 = 에러(destructive fallback 별도) | §8.4 |
| 22 | 크레이트 구조 | SQL/DDL 렌더·hook·MI 소스는 **roomrs-core**에 통합 | §3 |
| 23 | MI dedupe | **로그에 인스턴스 식별자(src) 컬럼을 두고 폴러가 자기 src를 제외해 소비** | §9.5 |
| 24 | DDL 무효화 정책 | DML(SELECT/INSERT/UPDATE/DELETE)·무해 문장 외 = **전체 무효화**(None) | §9.2 |
| 25 | 트랜잭션 시작 | **`BEGIN IMMEDIATE`** | 통합 풀의 checkout한 커넥션에서 실행. §10 |
| 26 | CLI 명령 | `cargo roomrs migrate diff / check / check-dir` | §3/§8 |
| 27 | feature 범위 | `verify-live`·`decimal`·`migrate` feature는 제공하지 않음 | 구현된 feature만 공개. §7.5 |
| 28 | export 후 재빌드 | 명시 export가 새 snapshot을 만들면 사용자는 후속 `cargo build`를 실행한다 | 신규 파일은 이전 매크로 전개에 등록되지 않았으므로 재전개·내장이 필요하다. §7.4 |
| 29 | 0행 insert rowid | rowid insert는 실제 1행 성공만 허용: `changes()==0`이면 `NotFound`; do-nothing 가능 insert의 `Result<i64>`는 **컴파일 에러** | Optional rowid API는 범위 밖. §5.2/§12c |
| 30 | LiveQuery 값 적재 | recv/Iterator/Stream 값 소비는 **keep-latest 단일 슬롯**; subscribe 콜백은 순차 호출 유지 | 느린 값 소비자는 중간 상태를 건너뛰고 최신 상태를 받음. §5.6 |
| 31 | in-memory live 잠금 | shared-cache in-memory의 일반 풀 연결은 **`read_uncommitted=1`** | in-memory 전용 절충. 전용 live 연결 없음(결정 51). §10 |
| 31b | in-memory write 직렬화 | in-memory DB의 **일반 풀은 1커넥션으로 고정** (live worker 도 동일 풀 checkout) | `SQLITE_LOCKED` 경합 방지. 전용 live 연결 없음(결정 51). §10 |
| 32 | view watch 의존성 | 추출 이름을 `sqlite_master`의 실제 table과 대조; view·미존재 객체면 **`UnknownDependencies`**, 사용자가 `.watching(...)`으로 명시 | §5.6 |
| 33 | 생성 SQL 식별자 | 생성 SQL 식별자에 U+0022(`"`)가 있으면 **원인 span 컴파일 에러** | 자동 escaping은 범위 밖 |
| 34 | 생성 async Future 수명 | DAO 생성 Future는 소유값만 캡처해 **`Send + 'static`** 보장 | `tokio::spawn(dao.find(...))` 직접 지원. §2.4/§5.2 |
| 35 | 스냅샷 모델 파사드 | `SchemaSnapshot`·`TableSnapshot`·`ColumnSnapshot`을 `roomrs`에서 재수출 | 내부 크레이트 직접 의존 불필요 |
| 36 | 쿼리빌더 LIKE ESCAPE | `like_escaped(pattern, escape: char)` — 패턴과 단일 Unicode escape 문자를 모두 파라미터 바인딩 | `%`·`_` 리터럴 검색 지원. §5.8 |
| 37 | 풀 API | `connections`·`with_connection`만 제공 | 통합 풀의 단일 커넥션 모델을 노출. §10 |
| 38 | 행 필터 무효화 | `InvalidationFilter`: 단일 테이블·AND/OR 그룹·`eq`/`neq`/NULL predicate | 단일 프로세스 `preupdate_hook`가 OLD/NEW를 매칭. 복잡 query 자동 분석 금지. §5.6/§9.2 |
| 39 | 스냅샷 불변성·생성 권한 | 확정 version snapshot은 **덮어쓰지 않는다**. snapshot JSON 생성 권한은 `cargo roomrs schema export`에만 둔다 | `cargo test`·`cargo build`·매크로는 파일을 생성·수정하지 않는다. 수동 version은 불일치 시 version 증가가 필요하다. §7.4 |
| 40 | 복합 PRIMARY KEY | 복수 `#[pk]` = 필드 선언 순서 table-level `PRIMARY KEY (…)` | 단일 `#[pk]`는 컬럼-level 유지(INTEGER PK affinity). `#[pk(autoincrement)]`와 다른 `#[pk]` 동시 사용 = 컴파일 에러 |
| 41 | table-level UNIQUE | `#[entity(unique(col, …))]` 반복 가능 | 단일 필드 `#[column(unique)]` 유지. 기존 데이터 중복 시 생성 실패 가능 → 기존 DB는 수동 migration |
| 42 | 복합·정렬·partial INDEX | `#[entity(index(name=…, columns(c, d desc), where=…))]` | 컬럼 순서 보존, ASC/DESC, partial `where` SQL 문자열. 일반 CREATE INDEX = auto 후보, UNIQUE INDEX = 수동 |
| 43 | 복합 FOREIGN KEY | `#[entity(foreign_key(columns(…), references="t(c…)", on_delete=…, on_update=…))]` | 기존 table 추가는 대개 재작성 → 수동 migration |
| 44 | CHECK 제약 | `#[entity(check = "expr")]` 반복 가능 | SQL 식 원문. 변경 시 수동 migration |
| 45 | custom SQL type | `#[column(sql_type = "…")]` — DDL type name 오버라이드 | ToSql/FromSql은 사용자 책임. `"` 금지 |
| 46 | trigger SQL file 훅 | `#[entity(trigger = "path.sql")]` 반복 가능 | 경로+내용 hash를 snapshot에 포함. 적용은 수동 forward migration만 |
| 47 | 프로젝트 schema export 진입점 | 설치형 Cargo subcommand `cargo roomrs schema export` | 소비자 package의 schema target을 자동 준비·실행해 등록된 모든 `#[database]`를 export. build.rs 호출 금지. §7.4 |
| 48 | `version = auto` | export 명령이 DB별 최신 snapshot hash와 엔티티 hash를 비교해 revision을 결정 | 일치면 no-op, 변경이면 다음 정수 version snapshot과 `{from}_{to}_roomrs_auto.sql` forward migration 초안을 함께 생성. 수동 `version = N`은 유지. §7.4 |
| 49 | LiveQuery debounce | DB 전역 `live_debounce(Duration)` 기본값은 **250ms**. observer는 `.debounce(Duration)`로 개별 override 가능 | observer에 개별값이 없으면 DB 전역값을 복사한다. 첫 무효화가 고정 coalesce 창을 시작하며, 창 안의 추가 무효화는 만료를 연장하지 않고 병합만 한다. 초기 emit·rebind·watching은 즉시. §9.3 |
| 50 | LiveQuery invalidation 집계 | commit/문장 단위 이벤트를 observer debounce 창에서 병합 | 행 필터는 preupdate OLD/NEW 값을 사용하고 재조회 예약은 결정 49를 따른다. §9.2 |
| 51 | LiveQuery reader worker pool | notifier는 이벤트 병합·예약만, 재조회는 bounded worker가 기존 통합 read/write 풀에서 checkout해 담당 | 전용 read-only connection은 만들지 않는다. 기본 `notifier_readers = min(2, connections)`, `.notifier_readers(N)`로 override. §9.3 |
| 52 | LiveQuery 다중 filter | `watch_*_filtered`는 복수 `InvalidationFilter`를 받고, 하나라도 변경 행에 매칭되면 재조회 | OR만 지원한다. filter 객체는 계속 단일 테이블·AND/OR group 모델을 유지한다. §5.6 |
| 53 | SQLite schema DSL 정합 | 기존·신규 schema DSL은 `DSL → DDL → snapshot → hash → diff → auto migration 분류`를 모두 통과 | 불완전한 메타는 자동 migration 금지·수동 migration 안내. §7/§8 |
| 54 | SQLite 고급 schema DSL | `collate`, generated column, `strict`, `without_rowid`, index column `COLLATE` 지원 | VIEW·FTS5·R*Tree 전용 DSL은 제공하지 않는다. VIEW는 직접 query struct + 수동 SQL로 충분. §5.1 |
| 55 | schema export target 탐색 | `#[database]`가 custom `cfg(roomrs_export)`용 `#[roomrs_export]` 진입점을 자동 생성하고 CLI가 lib·bin target을 test harness로 실행 | 사용자 source 추가·main hook 불필요. 일반 build/test는 export 코드를 컴파일·실행하지 않고 파일을 쓰지 않는다. §7.4 |
| 56 | entity-level PRIMARY KEY | `#[entity(primary_key(field, ...))]`로 단일·복합 PK 선언 가능 | 필드 `#[pk]`와 함께 쓰면 필드 선언 순서 목록과 정확히 같아야 한다. 다르면 일반 컴파일은 유지하고 `schema export/check`에서 오류와 수정 조언을 반환한다. §5.1 |
| 57 | DB open 스키마 보존 | DB open은 선언된 migration 외 schema 객체를 자동 탐색하거나 DROP하지 않는다 | 기존 DB 정리는 사용자 forward migration 책임이다. §8·§9.5 |
| 58 | CLI binary | 설치형 `cargo-roomrs` 하나만 제공 | schema와 migration 명령을 `cargo roomrs ...` 아래 통합해 설치·컴파일·문서 경로를 하나로 유지한다. §3·§8 |

---

## 1. 목표 / 비목표

### 1.1 목표
- Room 수준의 **선언적 개발 경험**: 엔티티=구조체, DAO=trait, SQL=매크로 문자열.
- **동기·비동기 둘 다 1급**: async 런타임 없는 코드부터 tokio/async-std/smol까지(런타임 무관 std `Future`).
- **컴파일 타임 안전성**: 정적 SQL을 커밋된 스키마 스냅샷과 대조(§7) + 파라미터 정합성 검증.
- **라이브 쿼리** + **멀티 인스턴스 무효화**(옵트인): 다른 프로세스의 write까지 알림(§9.5).
- **예측 가능한 SQLite 동시성**: 통합 풀의 커넥션 독점 checkout + WAL + `busy_timeout`(§10).
- 버전드 마이그레이션(3경로), 관계 자동 로딩, 데스크톱·모바일 단일 코드.

### 1.2 비목표 (v1)
- WASM 타깃(향후 별도 트랙).
- 파괴적 마이그레이션 자동 실행(초안 생성까지만).
- diesel식 완전 타입세이프 쿼리빌더로 raw SQL 대체.
- 비동기 트랜잭션의 **async 클로저**(`|tx| async {...}`) — v1은 동기 클로저형만(§5.5)[A-5].

---

## 2. 배경 분석

### 2.1 Android Room → roomrs 개념 매핑

| Room | roomrs | 구현 |
|---|---|---|
| `@Entity` | `#[entity]` | 구조체 → 테이블 |
| `@PrimaryKey`, `@ColumnInfo` | `#[pk]`, `#[column(...)]` | |
| `@Ignore` | `#[column(ignore)]` | 내장 `ignore` 속성 충돌 회피[A-4] |
| `@Dao` | `#[dao]` | trait → 동기/비동기 구현 생성 |
| `@Query/@Insert/@Update/@Delete` | `#[query]/#[insert]/#[update]/#[delete]` | |
| `@Transaction` | `#[transaction]` | tx-바운드 DAO 방식(§5.9) |
| `@Database` | `#[database(entities(...), version=N)]` | |
| `@TypeConverter` | `impl SqlType` / `#[json]` | |
| `@Relation`, `@Embedded` | `#[relation]`, `#[embedded]` | |
| `Flow<List<T>>` | `LiveQuery<Vec<T>>` (단일 구체 타입) | §5.6 |
| `Migration(1,2){execSQL}` | `Migration::sql(1,2,"…")` / SQL 파일 | §8.2 |
| `@RenameColumn/@RenameTable` | `renamed_from = "…"` | §8.3 |
| `fallbackToDestructiveMigration()` | `.fallback_to_destructive_migration(true)` | 기본 off |
| `Callback`/`QueryCallback` | `.on_create/.on_open/.query_logger` | |
| `suspend fun` | 비동기 파사드 `async fn` | 런타임 무관 Future |
| KSP | proc-macro | |

### 2.2 왜 rusqlite 코어 + 자체 풀인가
- SQLite에는 진짜 async I/O가 없다 → 동기 코어 + 비동기 파사드가 정석.
- rusqlite는 hook·`ToSql/FromSql`·bundled/bundled-sqlcipher를 직접 제공.
- **자체 미니 풀**: 통합 커넥션 N의 독점 checkout·PRAGMA 초기화·hook 재설치 요구에 맞춘다.
- 컴파일 타임 SQL 검증은 roomrs 자체 매크로 + 스키마 스냅샷 파일(§7).

### 2.3 생태계 포지셔닝 (2026-07)
- **rusqlite** — 코어. **diesel/SeaORM** — 범용 ORM(라이브·멀티인스턴스·Room DX 없음). **tokio-rusqlite** — SQLite 동기 작업을 비동기 실행기 밖으로 오프로드하는 선례.

### 2.4 비동기 파사드 — 런타임 무관 기본 + tokio 통합
- **기본(feature `async`)**: 모든 쿼리는 **자체 블로킹 워커 풀**에서 통합 커넥션을 checkout해 실행하고, 완료를 런타임 중립 oneshot 채널(futures-channel)로 알리는 순수 std `Future` 반환. `Stream`도 futures-core 기반 → tokio·async-std·smol·`futures::executor` 어디서든 await 가능.
- **선택(feature `tokio`)**: 블로킹 작업을 `tokio::task::spawn_blocking` 풀로 대체한다. `Handle::try_current()` 실패 시 **자체 워커 풀로 폴백**한다.
- **생성 async 메서드는 `+ Send` Future를 명시 생성**한다[B-3] — `tokio::spawn` 등에 바로 넘길 수 있어야 하므로.
- 순수 동기: `default-features=false` — async 코드 전무.

---

## 3. 크레이트 워크스페이스

```
roomrs/
├─ crates/
│  ├─ roomrs/                # 파사드(공개 API)
│  ├─ roomrs-core/           # Database · 자체 통합 풀 · 에러 · SqlType ·
│  │                         #   InvalidationTracker · 노티파이어 · 마이그레이션 런너
│  ├─ roomrs-async/          # 비동기 파사드: 런타임 무관 Future/Stream + tokio 통합(선택)
│  ├─ roomrs-macros/         # proc-macro: #[entity] #[dao] #[query] #[database] …
│  ├─ roomrs-migrate/        # SchemaSnapshot 모델 · diff · 압축 · 코드젠 (매크로와 **공유 크레이트**)
│  └─ roomrs-cli/            # cargo roomrs schema / migrate
├─ examples/  todo-sync · todo-async · live-query · multi-process · mobile-ffi
└─ xtask/
```
- `roomrs-migrate`의 스냅샷 모델은 **매크로(컴파일 타임)와 런타임이 공유**한다 — §7의 스냅샷 대조가 이 공유 타입 위에서 동작.
- SQL/DDL 렌더·preupdate_hook 소스는 roomrs-core에 통합.
- 파사드 예외(결정 로그 외 명기): `roomrs` 크레이트는 재수출 전용이되, **feature 스위치 매크로 `__if_async!` 1종만 예외**로 정의한다(자기 feature 상태로 생성 코드를 분기해야 하므로 파사드에만 둘 수 있음).

---

## 4. 레이어 아키텍처

```
┌──────────────────────────────────────────────────────────────┐
│ 사용자 코드: #[entity] · #[dao] trait · #[database]            │
└──────────────┬───────────────────────────────────────────────┘
               │ proc-macro 전개 (스냅샷 파일 대조 포함 §7)
┌──────────────▼───────────────────────────────────────────────┐
│ 생성 코드: 동기 DAO + 비동기 DAO(+Send) · 스키마 메타 ·        │
│            FromRow · 라이브 배선 · DEPENDS_ON                 │
└──────┬────────────────────────────┬──────────────────────────┘
┌──────▼──────────────┐  ┌──────────▼──────────┐  ┌────────────┐
│ roomrs-core (동기)   │  │ roomrs-async         │  │ roomrs-    │
│ Database · Tx ·      │  │ 런타임 무관 Future/  │  │ migrate    │
│ Tracker · 노티파이어 │  │ Stream · tokio(선택) │  │ Snapshot · │
└──────┬──────────────┘  └──────────┬──────────┘  │ Diff · Gen │
       │   (SQL 렌더 · 무효화 소스)  │              └────────────┘
┌──────▼────────────────────────────▼──────────┐
│ 자체 통합 미니 풀: 커넥션 N                    │
│   전부 read/write 가능 · checkout 독점         │
│   전부 preupdate_hook 설치                     │
└──────┬────────────────────────────────────────┘
   rusqlite → libsqlite3-sys (SQLite/SQLCipher × bundled/system)
```

---

## 5. 공개 API 설계

> 목표 인터페이스 계약. `#[dao]` trait는 매크로 입력 DSL이다. 생성물은 동기 trait `UserDao`와 비동기 trait `UserDaoAsync`다. 두 trait는 RPITIT 사용으로 dyn 비호환이며 목/테스트는 제네릭 또는 별도 파사드를 사용한다.

### 5.0 핸들 네임스페이스 — `db.run_sync()` / `db.run_async()`
`run_sync/run_async`는 대칭 API다. 순수 동기 빌드에는 `run_async()`가 없다.

### 5.1 엔티티 (코드퍼스트)

```rust
#[entity(table = "users")]
#[derive(Debug, Clone)]
pub struct User {
    #[pk(autoincrement)]
    pub id: i64,                       // insert 시 생략됨, 새 id는 반환값으로(§12c)
    #[column(unique, index)]
    pub email: String,
    #[column(name = "display_name")]
    pub name: String,
    #[column(default = "now")]
    pub created_at: OffsetDateTime,    // feature `time` — 기본 on(§14)
    #[json]
    pub prefs: UserPrefs,
    #[column(ignore)]                  // Room @Ignore 대응
    pub transient: Option<String>,
}

// 복합 PK · table UNIQUE · index · FK · CHECK · sql_type · trigger (결정 40–46)
#[entity(
    table = "t_payment",
    unique(store_id, external_payment_id),
    index(name = "idx_payment_store_created", columns(store_id, created_at desc)),
    index(name = "idx_payment_active", columns(store_id), where = "deleted_at IS NULL"),
    foreign_key(
        columns(store_id, customer_id),
        references = "customers(store_id, customer_id)",
        on_delete = "CASCADE",
        on_update = "NO ACTION"
    ),
    check = "amount >= 0",
    trigger = "migrations/triggers/t_payment_audit.sql",
)]
struct Payment {
    #[pk]
    store_id: String,
    #[pk]
    payment_id: String,
    customer_id: String,
    external_payment_id: String,
    #[column(sql_type = "DECIMAL(12,2)")]
    amount: Money,
    created_at: String,
    deleted_at: Option<String>,
}
```
- **속성 표기 규칙**: 무시 필드는 `#[column(ignore)]`로 표기한다. `#[pk]`/`#[json]`은 짧은 표기를 유지한다.
- **PRIMARY KEY[결정 40·56]**: 필드 `#[pk]` 또는 `#[entity(primary_key(field, ...))]`로 선언한다. 복수 필드 PK는 선언 목록 순서 table-level `PRIMARY KEY`, 단일 PK는 컬럼-level이다. 두 표기를 함께 쓰면 필드 선언 순서의 `#[pk]` 목록과 entity-level 목록이 정확히 같아야 하며, 다르면 `schema export/check`에서 오류를 반환한다. `#[pk(autoincrement)]`와 다른 PK 동시 = 컴파일 에러.
- **table UNIQUE / INDEX / FK / CHECK / trigger[결정 41–44,46]**: 엔티티 수준 속성. UNIQUE·FK·CHECK·trigger 변경 및 UNIQUE INDEX = 수동 migration. 일반 CREATE INDEX = auto 후보.
- **custom sql_type[결정 45]**: DDL type name만 오버라이드. 직렬화는 `ToSql`/`FromSql`.
- **autoincrement PK 의미론**: `#[pk(autoincrement)]` 필드는 생성 SQL에서 항상 생략되고 새 rowid가 반환된다. PK를 명시 삽입해야 하면 `#[insert(keep_pk)]`를 사용한다.
- **0행 insert 반환**: 0행 성공이 가능한 충돌 정책과 `Result<i64>` 반환을 함께 선언하면 컴파일 에러다.
- **JSON Option**: `#[json] Option<T>`의 `None`은 SQL NULL로 저장한다. JSON text `null`도 읽을 때 `None`으로 해석한다. 따라서 `T = ()`처럼 JSON 표현 자체가 `null`인 `Some(T)`는 `None`으로 읽힌다.
- **collate / generated / STRICT / WITHOUT ROWID / index COLLATE[결정 54]**: `#[column(collate)]`, `#[column(generated, stored)]`, `#[entity(strict)]`, `#[entity(without_rowid)]`, index `columns(name collate nocase)`. generated 는 DEFAULT·PK·autoincrement 와 비호환. 기존 테이블의 advanced 기능 변경은 자동 migration 금지·수동 안내.

### 5.2 DAO — 매크로가 동기/비동기 둘 다 생성

```rust
#[dao]
pub trait UserDao {
    #[query("SELECT * FROM users WHERE id = :id")]
    fn find(&self, id: i64) -> Result<Option<User>>;

    #[query("SELECT * FROM users ORDER BY name")]
    fn watch_all(&self) -> LiveQuery<Vec<User>>;      // 구체 타입(§5.6)

    #[insert]                                          // PK 생략 삽입, 새 id 반환
    fn add(&self, user: &User) -> Result<i64>;

    #[insert(on_conflict = "replace")]
    fn upsert(&self, user: &User) -> Result<i64>;

    #[delete("DELETE FROM users WHERE id = :id")]
    fn delete(&self, id: i64) -> Result<u64>;
}
```
- 반환 타입 규칙: `Result<T>`=정확히 1건(0건→`NotFound`) / `Result<Option<T>>`=0~1건 / `Result<Vec<T>>`=N건 / 스칼라 동일. 라이브는 `LiveQuery<T|Option<T>|Vec<T>>`.
- 파라미터: `:name` ↔ 인자 매칭(누락/오타/미사용 = 컴파일 에러) — 이 검증은 스냅샷 무관(로컬).
- 비동기 생성물: 동일 메서드명, 반환 Future는 `+ Send + 'static` 명시[B-3, 결정 34]. 호출 전에 핸들·인자를 소유값으로 분리해 `&self` 수명을 과캡처하지 않는다.

### 5.3 동적 쿼리 (쿼리빌더) — 실행 수신자는 핸들로 대칭[C-6]
```rust
let q = Query::select::<User>().and_where(col(User::id).in_list(ids));
let a: Vec<User> = q.clone().fetch_all(db.run_sync())?;          // 동기
let b: Vec<User> = q.fetch_all(db.run_async()).await?;           // 비동기 — 동일 메서드명
```

### 5.4 데이터베이스 열기
```rust
#[database(entities(User, Post), daos(UserDao, PostDao), version = 3)]
pub struct AppDb;

let db = AppDb::builder()
    .sqlite("app.db")                     // WAL 기본
    .connections(4)                       // 통합 풀 크기
    .live_debounce(Duration::from_millis(250)) // LiveQuery 전역 기본값(생략해도 250ms)
    .notifier_readers(2)                  // LiveQuery 재조회 worker 수(기본 min(2, connections))
    .busy_timeout(Duration::from_secs(5)) // 프로세스 내·외부 writer 경합 대기
    .queue_timeout(Duration::from_secs(2)) // 통합 풀 checkout 제한
    .migrate(MigrationPolicy::Auto)
    .on_create(|conn| Ok(()))             // 최초 생성 1회
    .on_open(|conn| Ok(()))               // 오픈 시마다
    .query_logger(|sql, dur| log::debug!("{sql} ({dur:?})"))
    .build()?;                            // build_async().await 도 제공
```
- `build()` 는 **런타임 스키마 검증**도 수행: 매크로에 임베드된 스냅샷 해시 vs 엔티티 메타 재계산 해시 비교 → 불일치 시 "스냅샷 스테일" 에러(§7.4).

### 5.5 트랜잭션 [A-5 반영]

**동기 — 클로저 + RAII (checkout한 커넥션 점유)**
```rust
db.run_sync().transaction(|tx| {          // tx: &mut Tx — 같은 커넥션에 바인딩
    tx.user_dao().upsert(&u)?;            // tx-바운드 DAO(§5.9와 동일 메커니즘)
    tx.post_dao().insert(&p)?;
    Ok(())
})?;                                       // 에러/panic 시 롤백
let mut tx = db.run_sync().begin()?;       // RAII — drop 시 미커밋이면 롤백
```

**비동기 — v1은 "동기 클로저형"만**
```rust
// 클로저 전체가 블로킹 워커에서 같은 커넥션으로 원자 실행된다. 클로저 안에서 await 불가.
db.run_async().transaction(|tx| {         // |tx: &mut Tx| — 동기 클로저
    tx.user_dao().upsert(&u)?;
    Ok(())
}).await?;                                 // 반환 Future만 async
```
- **비동기 트랜잭션이 동기 클로저형인 이유**: Future 취소 시 열린 트랜잭션의 비동기 롤백 문제를 피하고, 같은 checkout에서 항상 commit 또는 rollback으로 종결한다.
- **비동기 RAII `begin()` 은 v1 제외** — 같은 취소-롤백 문제.

### 5.6 라이브 쿼리 — 단일 구체 타입 `LiveQuery<T>` [C-2 반영]

라이브 쿼리는 **구체 타입 하나로 제공**한다:

```rust
pub struct LiveQuery<T> { /* … */ }
impl<T> LiveQuery<T> {
    // 소비 (동기)
    pub fn recv(&self) -> Result<T>;                       // 블로킹
    pub fn recv_timeout(&self, d: Duration) -> Result<Option<T>>;
    pub fn try_recv(&self) -> Result<Option<T>>;
    pub fn iter(&self) -> impl Iterator<Item = Result<T>> + '_;
    // 소비 (콜백 — 동기/비동기 공용, 노티파이어 스레드에서 호출)
    pub fn subscribe(&self, f: impl FnMut(T) + Send + 'static) -> SubscriptionGuard;
    // 소비 (비동기)
    pub fn into_stream(self) -> impl Stream<Item = Result<T>> + Send;   // feature `async`
    // 제어
    pub fn rebind(&self, params: &[&dyn ToSql]) -> Result<()>; // 같은 SQL, 바인딩 교체(§5.6b)
    pub fn watching(self, tables: &[&str]) -> Self;        // 의존 명시(직접 쿼리용)
    pub fn debounce(self, delay: Duration) -> Self;        // observer별 재조회 지연(기본 250ms, 결정 49)
}

pub struct InvalidationFilter { /* 단일 테이블 행 필터 */ }
impl InvalidationFilter {
    pub fn table(name: &str) -> Self;
    pub fn where_group(self, f: impl FnOnce(FilterGroup) -> FilterGroup) -> Self;
    pub fn or_where_group(self, f: impl FnOnce(FilterGroup) -> FilterGroup) -> Self;
    pub fn build(self) -> Result<Self>;
}

impl FilterGroup {
    pub fn eq(self, column: &str, value: impl Into<Value>) -> Self;
    pub fn neq(self, column: &str, value: impl Into<Value>) -> Self;
    pub fn is_null(self, column: &str) -> Self;
    pub fn is_not_null(self, column: &str) -> Self;
}
```
- DAO의 `watch_*`, 직접 쿼리의 `db.run_*().watch_*` 모두 이 타입을 반환. `run_async()` 경로는 관례상 `into_stream()` 소비를 안내하지만 타입은 동일.
- `rebind` 는 SQL이 고정된 DAO watch에도 유효(바인딩 파라미터가 있는 경우).
- **수명 계약**: guard drop은 신규 콜백 시작을 차단한다. 이미 실행 중인 콜백은 최대 1건 완료될 수 있다. 재조회는 노티파이어 스레드로 라우팅되며 epoch로 이전 세대 결과를 폐기한다.
- **값 전달 계약[결정 30]**: 수신 대기열은 keep-latest 단일 슬롯이다. 생산자가 소비자보다 빠르면 미소비 중간 상태를 최신 상태로 덮어쓴다. `recv`/`try_recv`/`into_stream` 모두 같은 계약을 따른다.
- **DB 전역·observer별 debounce[결정 49]**: Builder의 `live_debounce(Duration)` 전역 기본값은 생략 시 250ms다. `LiveQuery`는 생성 시 이 값을 사용하고, `.debounce(Duration)`가 있으면 해당 observer 값이 전역값을 override한다. 첫 무효화가 observer의 고정 coalesce 창을 시작하며, 창 안의 추가 무효화는 만료 시각을 연장하지 않고 병합만 한다. notifier는 가장 이른 만료 시각까지 대기해 만료 observer만 재조회한다.
- `let _ = q.subscribe(…)` 는 즉시 drop되어 구독이 끝난다. 구독 guard를 변수나 구조체 필드에 보관해야 한다.
- **행 필터 구독**: 직접 쿼리와 DAO는 `watch_*_filtered(..., filters)` 경로로 하나 이상의 `InvalidationFilter`를 받는다. filter 객체 중 하나라도 변경 행에 매칭되면 재조회(OR)한다. filter 객체 내부의 group은 AND/OR 모델을 유지한다. `eq`/`neq`의 NULL 의미는 SQL과 같아 `is_null`/`is_not_null`을 별도로 쓴다. INSERT는 NEW, DELETE는 OLD, UPDATE는 OLD 또는 NEW가 filter에 매칭될 때만 재조회한다. filter 객체 간 AND·중첩 boolean expression은 지원하지 않는다.
- filter 없는 기존 `watch_*`는 의존 테이블 단위 무효화를 유지한다. JOIN·subquery·함수 등 복잡 query 자동 분석은 하지 않는다. 필요한 테이블마다 명시 filter를 제공하지 않으면 해당 테이블은 보수적으로 전체 무효화한다.

### 5.6b 페이지네이션 패턴
guard를 화면 struct 필드에 보관하고, 페이지 이동은 `rebind`, 쿼리 변경은 재구독, 총건수는 `watch_scalar(COUNT)`를 사용한다. 앱 수명 구독은 `guard.detach()`를 사용한다.

### 5.7 직접 쿼리 (DAO/엔티티 없이)
```rust
let n: u64 = db.run_sync().execute("UPDATE users SET name=?1 WHERE id=?2", params![name, id])?;
let row: (i64, String) = db.run_sync().query_one("SELECT id, email FROM users WHERE id=?1", params![id])?;
let opt: Option<(i64, String)> = db.run_sync().query_optional("…", params![id])?;
let cnt: i64 = db.run_sync().query_scalar("SELECT COUNT(*) FROM users", params![])?;
let all: Vec<UserView> = db.run_sync().query_all("…", params![])?;   // #[derive(FromRow)] 구조체/튜플/동적 Row

// 라이브 — 반환 타입은 구체 타입 LiveQuery<T>
let live: LiveQuery<Vec<UserView>> = db.run_sync().watch_all("SELECT … FROM users WHERE done=?1", params![false]);
let live_one: LiveQuery<Option<UserView>> = db.run_sync().watch_optional("… WHERE id=?1", params![id]);
let live_cnt: LiveQuery<i64> = db.run_sync().watch_scalar("SELECT COUNT(*) FROM users", params![]);
let guard = live_cnt.subscribe(|n| { ui_tx.send(n).ok(); });
```
- 런타임 의존 추출 실패 또는 view 참조 시[결정 32] 구독은 `Error::UnknownDependencies` — `.watching(&[…])` 또는 `.watching_all()` 로 기저 테이블을 명시해 해소.

### 5.8 텍스트 검색 — `LIKE` (결정 로그 7c)

텍스트 검색은 별도 기능 없이 raw SQL `LIKE` 로 처리한다(라이브 쿼리도 동일하게 동작).
동적 쿼리 빌더는 `col("title").like(pattern)`과
`col("title").like_escaped(pattern, escape_char)`를 제공한다. 후자는 패턴과 단일
Unicode escape 문자를 모두 SQLite 파라미터로 바인딩해 `%`, `_`, escape 문자 자체를
리터럴로 검색한다. 기존 `like()`는 ESCAPE 절을 추가하지 않는다.

```rust
#[dao]
trait SearchDao {
    #[query("SELECT * FROM posts WHERE title LIKE :pat OR body LIKE :pat ORDER BY id DESC")]
    fn search(&self, pat: &str) -> Result<Vec<Post>>;   // pat = format!("%{q}%")
}
```
- 성능 참고 문서화: `LIKE '검색어%'`(전방 일치)는 인덱스 활용 가능(`COLLATE NOCASE` 인덱스 권장), `'%검색어%'`(중간 일치)는 풀스캔 — 수만 행 수준의 로컬 데이터에서는 일반적으로 문제 없음. 대소문자: ASCII는 `NOCASE`, 한글 등 비ASCII는 케이스 개념 없음.

### 5.9 `#[transaction]` — tx-바운드 DAO 방식 [A-2 반영]

**메커니즘 확정**: 매크로는 DAO 구현을 **커넥션 컨텍스트 제네릭**으로 생성한다 — 풀-바운드(`PoolDao`: 호출마다 통합 풀 체크아웃)와 **tx-바운드(`TxDao<'tx>`: 주어진 트랜잭션 커넥션 사용)** 두 형태. `#[transaction]` 메서드는:
1. 통합 커넥션을 checkout해 `BEGIN IMMEDIATE`로 트랜잭션 시작,
2. **본문을 tx-바운드 DAO 문맥에서 실행** — 본문의 `self.withdraw(...)` 호출은 매크로 재작성에 의해 `TxDao` 메서드 호출로 컴파일되므로 **같은 트랜잭션 커넥션을 사용**한다(풀 재체크아웃 없음 → 자기 락 경합·데드락 불가),
3. 성공 시 commit / 에러·panic 시 rollback.
- tx-바운드 DAO가 checkout guard의 커넥션을 직접 빌리므로 "현재 트랜잭션 커넥션 찾기"용 스레드로컬이 필요 없다.
- 중첩(`#[transaction]` 안에서 `#[transaction]` 호출)은 savepoint.
- `db.run_sync().transaction(|tx| ...)` 의 `tx.user_dao()` 도 동일한 `TxDao` 를 반환한다 — 한 메커니즘.

### 5.10 관계 로딩 일관성

`with_relations`는 부모와 자식의 여러 SELECT를 하나의 일관된 스냅샷에서 읽기 위해 checkout한 같은 커넥션에서 트랜잭션을 유지한다(결정 7). 풀 재체크아웃 없이 한 스냅샷을 공유한다.

---

## 6. SQL 렌더 & 타입 매핑
(SQLite 단일 — SQL/DDL 렌더는 `roomrs-core`에 통합한다. `time`/`uuid`는 기본 feature로 제공한다.)

---

## 7. SQL 검증 — 스키마 스냅샷 파일 대조

### 7.1 왜 스냅샷 파일인가
Rust proc-macro는 **자신이 붙은 아이템의 토큰만** 볼 수 있다. `#[dao]` 전개 시 다른 파일의 `#[entity]` 구조체 정보를 언어 차원에서 얻을 수 없고, OUT_DIR 사이드채널은 전개 순서·증분 컴파일에서 보장이 없다. 따라서 **검증의 진실 소스는 리포에 커밋된 스키마 스냅샷 파일**로 한다(diesel의 `schema.rs` 단일 소스 접근과 동계열, 우리는 마이그레이션용 스냅샷 §8.1을 재활용).

### 7.2 정적 경로 (컴파일 타임)
1. `#[entity]` 는 자기 구조체의 스키마 메타(`Entity` impl)를 생성한다 — 로컬 정보만.
2. 스냅샷은 **버전별 파일**로 커밋된다: `CARGO_MANIFEST_DIR/migrations/schema/[db이름].[버전].json`
   (db이름 = `#[database]` 구조체명 snake_case, 예: `AppDb` v3 → `app_db.3.json`).
   디렉토리 재지정은 `ROOMRS_SCHEMA_DIR` env. `current.json` 단일 파일 방식은 폐기.
3. `#[query("...")]` / `#[database]` 는 대상 DB의 **현재 버전 스냅샷 파일**(`[db].[VERSION].json`)을 읽어
   SQL의 참조 테이블·컬럼을 대조 → 없으면 **컴파일 에러**(span 표시).
   매크로는 읽은 파일마다 `include_bytes!`를 방출해 **rustc 파일 의존성을 등록**한다 —
   스냅샷 갱신은 매크로 재전개를 보장한다.
4. 파라미터 `:name` ↔ 인자 정합성은 스냅샷 무관 로컬 검증.
5. **결과 컬럼 ↔ 반환 타입 정합성**: 명시적 컬럼 나열의 단순 SELECT에 한정한다. 그 외 결과는 런타임에 검증한다.

### 7.3 파서 실패 모드
- `sqlparser`(SQLite 방언)가 파싱하지 못하는 유효한 SQLite 관용구가 존재할 수 있다.
- 정책: 파싱 실패 = **컴파일 에러가 아니라 "검증 스킵 + 경고"**(사용자를 막지 않음). 명시적 스킵은 `#[query(unchecked, "…")]`.

### 7.4 스냅샷 생성·스테일 대응
- **생성 권한[결정 39]**: snapshot JSON을 생성·수정할 수 있는 명령은 `cargo roomrs schema export` 하나다.
  `cargo test`, `cargo build`, proc-macro, `build.rs`와 런타임 API는 파일을 만들거나 고치지 않는다.
  auto version의 forward migration SQL 초안도 이 명령만 생성한다. `cargo test`·`cargo build`는 JSON·SQL 어느 파일도 쓰지 않는다.
- **프로젝트 schema target[결정 47·55]**: `cargo roomrs`는 설치형 `cargo-roomrs` 실행파일이다. `schema export/check`는
  Cargo metadata로 대상 package와 lib·일반 bin target을 정하고, `roomrs_export` custom cfg를 켠 test harness를 실행한다.
  `#[database]`는 이 cfg에서만 `#[roomrs_export]` 진입점을 자동 생성하며, 진입점은 해당 target에 링크된
  `inventory`의 모든 `SchemaExportEntry`를 순회한다. 사용자 `lib.rs`나 `main()` 변경 없이 동작한다.
  일반 `cargo build`·`cargo test`에는 custom cfg가 없으므로 export 진입점이 컴파일·실행되지 않고 파일을 쓰지 않는다.
  workspace는 현재 package가 기본이며 `--package <name>` 또는 `--workspace`로 범위를 고른다. workspace 모드에서
  `#[database]`가 없는 package는 건너뛰고, 명시 선택한 단일 package에 DB가 없으면 오류를 반환한다.
- **자동 등록**: `#[database]`는 `DatabaseSpec` export entry와 custom cfg 전용 진입점을 함께 생성한다.
  CLI가 선택한 target의 진입점은 링크된 모든 entry를 순회한다. 파라미터 검증·attribute 검증은 항상 유지한다.
- **수동 version**: `#[database(version = N)]`은 `[db].N.json`이 없으면 export가 생성한다. 파일 hash가 같으면 no-op이다.
  hash가 다르거나 파일이 파손되면 아무 파일도 덮어쓰지 않고 실패한다. 사용자는 `N`을 올린 뒤 다시 export한다.
- **자동 version[결정 48]**: `#[database(version = auto)]`은 package에 저장된 해당 DB의 최대 version을 현재 version으로 사용한다.
  snapshot이 없으면 `1`을 만든다. 최대 version snapshot hash가 엔티티 hash와 같으면 no-op이고, 다르면 다음 정수 version JSON과 `migrations/{from}_{to}_roomrs_auto.sql` forward migration 초안을 만든다.
  export 뒤 `cargo build`가 새 파일을 읽어 version·내장 snapshot을 갱신한다. 여러 DB는 모두 preflight한 뒤 오류가 없을 때만 누락·다음 version 파일과 SQL 초안을 쓴다.
  같은 이름의 migration SQL이 있으면 덮어쓰지 않고 실패한다. `diff_sql`이 파괴적·모호 변경을 표시하면 SQL에 TODO를 남기고 export는 비성공 상태로 끝내 사용자의 검토·보완을 요구한다.
- **검사와 CI**: `cargo roomrs schema check`는 registry의 모든 DB와 snapshot을 비교하지만 쓰지 않는다. CI 순서는
  `cargo roomrs schema check` → `cargo test --workspace` → `cargo build --workspace`다. test는 snapshot 부재·불일치면 실패할 수 있지만 파일 시스템을 변경하지 않는다.
- **불변성**: 과거 스냅샷 수정은 기존 배포 DB와 migration 이력을 분기시키므로 금지한다.
- **유효성 체크(빌드 시)**: `#[database]` 전개가 존재하는 전 버전 스냅샷을 로드해
  버전 단조성·파스 유효성을 검증. **파일이 존재하는데 파손 = 컴파일 하드 에러** —
  "부재 = 스킵"과 구분한다.
- 스테일 3중 방어(유지):
  (a) `cargo roomrs schema check` + `cargo roomrs migrate check`(CLI/CI — registry·파일 기반 해시 비교·디렉토리 검사).
  (b) `Database::build()` 런타임 검증 — 매크로가 임베드한 현재 버전 스냅샷 해시 vs 런타임 엔티티 메타 해시 비교, 불일치 시 명확한 에러(§5.4).
  (c) 스냅샷 파일 부재 시: 정적 스키마 대조는 경고와 함께 스킵(신규 프로젝트 온보딩 마찰 방지), 파라미터 검증은 항상 수행.
  단, `Database::build()`는 현재 버전 스냅샷이 매크로 전개 때 부재했다면 `SnapshotStale` 오류로
  실행을 중단하고 `cargo roomrs schema export` 후 재빌드를 안내한다.

### 7.5 동적 경로
- 쿼리빌더/직접 쿼리는 prepare 시 런타임 검증한다.
- `verify-live`는 제공하지 않는다.

---

## 8. 마이그레이션 (자동 · 반자동 · 수동)
(§8.1 자동 diff, §8.2 반자동 SQL 스텝(`Migration::sql(from, to, …)`/`sql_batch`/`migrations_dir!`), rename 힌트, destructive fallback.)

### 8.3 수동 코드 스텝 — 버전 모델 통일 [C-3]
```rust
pub trait Migration {
    fn from_version(&self) -> u32;
    fn to_version(&self) -> u32;       // Migration::sql(from, to, …) 과 동일 모델
    fn up(&self, tx: &mut MigrationTx) -> Result<()>;
    fn down(&self, tx: &mut MigrationTx) -> Result<()> { unimplemented!() }  // 선택
}
```
- 세 소스(자동 .sql / 인라인 SQL / 코드)는 (from,to) 체인으로 병합·순서 실행, 같은 구간 중복 = 에러.

### 8.4 스냅샷 내장 + 자동 마이그레이션 [결정 21c/21d 신설]
- **내장**: `#[database]` 전개가 `migrations/schema/[db].*.json` 전 버전을 읽어
  압축(miniz_oxide) 바이트 상수로 바이너리에 내장하고,
  `DatabaseSpec::EMBEDDED_SCHEMAS: &'static [EmbeddedSchema]`(버전 오름차순)로 노출한다.
  각 원본 파일은 `include_bytes!`로 의존성 등록(§7.2) — 사장 상수는 링커가 제거.
- **자동 마이그레이션(옵트인)**: builder `.auto_migrate(true)`.
  `plan_chain`에 갭이 있는 구간을 내장 스냅샷의 연속 버전 diff로 메운다.
  - **안전 연산만 자동 실행**: CREATE TABLE · ADD COLUMN(nullable, 또는 NOT NULL + `default_sql` 보유) · CREATE INDEX · 유효 rename 힌트.
  - 컬럼 `default_sql`은 스냅샷·hash에 보존된다(결정 53). DEFAULT 변경·제약 변경·삭제·타입 변경·추정 rename은 자동 실행 금지.
  - 파괴적 연산(DROP/타입·DEFAULT 변경/제약 재작성/이름 추정 rename)이 필요한 구간 = **명확한 에러**
    (수동 스텝 등록 또는 `fallback_to_destructive_migration` 유도). 자동 실행 금지 원칙(§1.2) 유지.
  - 구형 스냅샷(필드 부재)은 `default_sql = None`으로 읽고, 자동 판정 불가면 재생성(`cargo roomrs schema export`) 또는 수동 migration을 안내한다.
  - 등록 스텝이 있는 구간은 항상 등록 스텝이 우선 — 내장 diff는 갭 폴백.
- **CLI**: `cargo roomrs migrate diff <old.json> <new.json> [out.sql]`(초안),
  `cargo roomrs migrate check <a.json> <b.json>`(해시 비교),
  `cargo roomrs migrate check-dir <schema_dir> <db이름>`(버전 파일 스캔 — 단조성·파스·연속 diff 검증).

---

## 9. 라이브 쿼리 / 무효화 엔진

### 9.1 요구
(동일 — 구독 즉시 1회 emit, 의존 테이블 write 시 재조회 emit.)

### 9.2 인-프로세스 무효화 — **주 경로: 문장 기반, 보조: preupdate_hook**

모든 일반 커넥션에 동일한 무효화 상태와 hook을 설치한다. SQL의 read/write 의미를 라우팅에 사용하지 않는다.
- **주 경로 — 문장 기반 무효화**: 각 실행 문장의 대상 테이블을 (정적 쿼리=`DEPENDS_ON` 메타, 동적/직접 쿼리=런타임 파싱, 빌더=엔티티 메타) 로 확정 → **트랜잭션 commit이 성공적으로 반환된 후** 영향 테이블 집합을 방출. hook의 알려진 누락(WHERE 없는 `DELETE`의 truncate 최적화, WITHOUT ROWID)과 무관하게 정확하다.
- **비-DML 문장 = 전체 무효화**: DML(SELECT/INSERT/UPDATE/DELETE)과 확실한 읽기 전용 문장 외에는 대상 테이블 특정을 시도하지 않고 **전체 무효화**로 처리한다.
- **보조 경로 — preupdate_hook(모든 일반 커넥션에 설치)**: 어느 checkout에서 발생하든 사용자 SQL의 **간접 write**(사용자 정의 트리거가 다른 테이블 수정 등)를 포착. filter가 참조한 OLD/NEW 컬럼값만 소유 복사해 transaction pending에 기록하고, filter 매칭 구독만 재조회한다. filter 없는 구독은 테이블 단위 집합에 병합한다.
- **hook callback 경계**: hook 안에서 SQLite API 재진입·재조회·통지를 하지 않는다. 최상위 transaction commit API 성공 반환 뒤에만 pending 변경을 Tracker에 방출한다. rollback·savepoint rollback은 해당 pending 변경을 폐기한다.
- **commit_hook 미사용**: commit_hook은 디스크 확정 전에 호출되므로 **commit API가 성공 반환한 뒤** 방출한다. 롤백 시 수집분은 폐기한다.
- **탈출구 경고[B-7]**: `db.run_sync().with_connection()` 으로 사용자가 자기 preupdate_hook을 등록하면 그 커넥션의 roomrs hook이 **교체**되어(커넥션당 1개) 행 필터 감지가 조용히 죽는다 — API 문서에 굵은 경고 + 전체 일반 커넥션에 hook을 재설치하는 `db.rearm_hooks()` 복구 제공.

### 9.3 InvalidationTracker
테이블→구독 역인덱스와 정적 `DEPENDS_ON`·동적 런타임 의존 수집을 사용한다. notifier는 DB당 하나이며 이벤트 수신, observer별 고정 coalesce 예약, worker 작업 제출만 한다. DB 전역 debounce는 Builder `live_debounce`로 정하고, observer `.debounce`가 있으면 그것을 우선한다. 재조회는 bounded `roomrs-live-worker-{n}`이 기존 통합 read/write 풀에서 checkout하여 수행하고, 완료 즉시 반납한다. 전용 read-only 연결·별도 DB 풀은 만들지 않는다.

### 9.4 백프레셔/정합성
(동일 — 최신값 우선, 최종 일관성 명시.)

### 9.5 멀티 프로세스 무효화

| 중요도 | 단계 | 상태 | 내용 |
|---|---|---|---|
| P0 | 단일 프로세스 테이블 무효화 | **구현됨** | commit 성공 뒤 영향 테이블을 Tracker에 방출 |
| P0 | 단일 프로세스 행 필터 | **구현됨** | `preupdate_hook` OLD/NEW와 `InvalidationFilter`로 관련 변경만 재조회 |
| P1 | Filter API 대칭 | **구현됨** | `watch_all/optional/scalar_filtered` sync·async + DAO `InvalidationFilter` 인자 |
| P1 | 다중 LiveQuery filter | **구현됨** | 복수 `InvalidationFilter` OR 매칭. filter 간 AND·중첩 boolean expression은 제외(결정 52). |
| P1 | 필터 스키마 검증 | **구현됨** | 등록 시 table/column 존재 검증. 실패 = `Error::InvalidationFilter` (`path=live_query`, `advice=check_configuration`) |
| P1 | observer debounce | **구현됨** | DB 전역 `.live_debounce`(기본 250ms) + observer `.debounce` override, 고정 coalesce (결정 49) |
| P1 | live worker pool | **구현됨** | notifier 스케줄 전용 + 통합 풀 checkout worker, `.notifier_readers(N)` (결정 51) |
| P2 | 무효화 관측성 | **구현됨** | `Database::live_metrics()` → `LiveMetrics`(events/coalesce/queue/refresh_ok·err/stale). 파라미터·행 데이터 미포함 |
| — | 다중 프로세스 LiveQuery 무효화 | **현재 미지원** | 단일 프로세스 roomrs 연결만 관찰한다. trigger·변경 로그·poller는 사용하지 않는다. |

다중 프로세스·raw SQLite writer 관찰은 공개 로드맵 범위 밖이다. 배경, trigger 방식의 제외 이유, 재검토 조건은 `docs/MULTI_PROCESS_INVALIDATION.md`에 보관한다.

### 9.6 노티파이어·재조회 worker
DB당 notifier 스레드는 1개다. notifier는 수신→고정 coalesce→worker 제출만 수행한다. 재조회 worker 수 기본값은 `min(2, connections)`이고 Builder `.notifier_readers(N)`로 변경한다. worker는 기존 통합 read/write 풀에서만 커넥션을 checkout하며 재조회 후 즉시 반납한다. worker가 느려도 notifier와 다른 observer 작업이 막히지 않아야 하며, 해제·종료·generation 변경 뒤 도착한 결과는 폐기한다. rebind 재조회도 이 경로로 라우팅한다.

---

## 10. 커넥션 / 풀 / 동시성 — 자체 통합 미니 풀

- **일반 커넥션 N**: 전부 read/write 가능. `query_only`와 SQL 첫 키워드 기반 read/write 라우팅을 두지 않는다. `query`/`execute` API 차이는 결과 행 소비 여부뿐이며 `INSERT ... RETURNING`, CTE, writable PRAGMA도 같은 규칙으로 실행한다. 단, shared-cache in-memory DB는 동시 write 트랜잭션의 `SQLITE_LOCKED`를 구조적으로 막기 위해 일반 풀을 1커넥션으로 고정한다(결정 31b).
- **독점 checkout**: guard 하나가 커넥션 하나를 소유해 한 시점에 한 작업만 사용한다. 풀 공유는 같은 `Connection`의 동시 접근을 뜻하지 않는다. FIFO 티켓 + 큐 타임아웃 + 공정성을 적용하고, 사용 가능한 커넥션이 없으면 반납까지 기다린다.
- **트랜잭션**: checkout한 같은 커넥션을 commit/rollback까지 고정하고 **`BEGIN IMMEDIATE`** 로 시작한다.
- 공통 PRAGMA: `journal_mode=WAL`, `foreign_keys=ON`, `synchronous=NORMAL`, 설정된 `busy_timeout`. 여러 프로세스는 각자 풀을 열며 `busy_timeout` 만료 뒤 `SQLITE_BUSY`가 반환될 수 있다.
- 모든 일반 커넥션 오픈·재오픈 시 암호화 키·공통 PRAGMA·`on_open`·live preupdate_hook을 동일하게 설치한다. 반납 복구(열린 트랜잭션 rollback 등)가 실패한 커넥션은 즉시 폐기하고 동일 factory로 **1회** 재오픈한다. 재오픈도 실패하면 풀을 fatal 상태로 닫고 이후 checkout에 명시 오류를 반환한다.
- 탈출구: `db.run_sync().with_connection(|c| …)`. 해당 커넥션의 hook 교체 경고는 §9.2. **주의: 클로저 안에서 같은 DB 를 재획득(중첩 checkout)하지 말 것 — 풀 교착 위험.**

## 11. 타입 컨버터
(동일 — rusqlite `ToSql`/`FromSql` 위임, `#[json]`, `#[derive(SqlType)]`. `time`/`uuid` 기본 on.)

## 12. 에러 모델

- 공개 fallible API는 단일 `roomrs::Result<T, roomrs::Error>`를 반환한다.
- `Error`는 `thiserror`로 Display와 source chain을 유지한다. 기존 variant(`Sqlite`, `Migration`, `Config` 등)는 호환을 위해 유지한다.
- 모든 `Error`는 `error.path() -> ErrorPath`와 `error.advice() -> ErrorAdvice`를 제공한다.
  - `ErrorPath`는 `database`, `query`, `connection_pool`, `migration`, `schema_snapshot`, `live_query`, `configuration`, `serialization`, `internal` 중 오류 발생 하위 시스템을 나타낸다.
  - `ErrorAdvice`는 retry, DB·쿼리·설정 점검, migration 검토, snapshot 재생성, live dependency 선언, 로그 확인, DB 재오픈 중 권장 다음 조치를 나타낸다.
- 호출자는 display 문자열을 파싱하지 않고 `ErrorPath`·`ErrorAdvice`로 로그, UI, 재시도 정책을 분기한다.
- `Error::SnapshotStale`(§7.4), `Error::QueueTimeout`(풀 checkout), `Error::UnknownDependencies`를 포함한다.
- `Drop`, 전용 스레드, callback, FFI처럼 `Result`를 반환할 수 없는 경로의 복구·로그 정책은 [반환 불가 경로와 크래시 정책](docs/RETURN_UNAVAILABLE.md)을 따른다.

## 13. 플랫폼
(동일 — 데스크톱 1급 + 모바일 FFI. MSRV **1.85/Edition 2024** 를 CI 매트릭스에 포함.)

## 14. 기능 플래그

| feature | 기본 | 설명 |
|---|---|---|
| `sqlite-bundled` | on | rusqlite가 SQLite를 함께 빌드 |
| `sqlite-system` | off | OS package 또는 vcpkg SQLite에 링크 |
| `sqlcipher-bundled` | off | vendored OpenSSL을 포함한 SQLCipher를 함께 빌드 |
| `sqlcipher-system` | off | OS package 또는 vcpkg SQLCipher에 링크 |
| `bundled` | off | `sqlite-bundled` 하위 호환 alias |
| `cipher` | off | `sqlcipher-bundled` 하위 호환 alias |
| `async` | on | 런타임 무관 비동기 파사드. 끄면 순수 동기 |
| `tokio` | on | tokio 통합 최적화(`async` 포함) |
| `live` | on | 라이브 쿼리/무효화 |
| `time`, `uuid` | **on** | 기본 예시가 기본 설정에서 컴파일되도록 승격[C-4] |
| `json` | on | `#[json]` |

backend canonical feature는 동시에 둘 이상 활성화할 수 없다. 기본 빌드는 bundled SQLite만 컴파일하며 OpenSSL을 의존하지 않는다. SQLCipher는 명시적으로 선택한 경우에만 OpenSSL과 함께 빌드된다. `roomrs-core`를 backend feature 없이 빌드하는 구성은 유지한다. Windows MSVC system backend는 vcpkg `x64-windows-static-release` community triplet의 release-only 정적 라이브러리·정적 CRT를 사용하고 Rust도 `-C target-feature=+crt-static`으로 CRT를 일치시킨다. `live`에는 `SQLITE_ENABLE_PREUPDATE_HOOK`가 포함된 port가 필요하다. CI는 기본 feature graph, 네 backend 각각의 check/test/clippy/fmt/doc/feature graph와 canonical backend 충돌 6쌍을 검증한다.


## 15. 0.1.0 기능 범위

- 크레이트 골격 · CI · MSRV 1.85 · 듀얼 라이선스
- 동기 CRUD · 통합 풀 · 직접 쿼리 API
- 실행기 독립 비동기 Future/Stream
- 스키마 스냅샷 기반 정적 SQL 검증
- 클로저·RAII·DAO 트랜잭션과 savepoint
- 자동·SQL·코드 마이그레이션
- LiveQuery와 멀티 인스턴스 무효화
- 관계 매핑과 동적 쿼리빌더
- 운영 훅 · 모바일 FFI · 선택적 SQLCipher

## 16. 테스트 전략
실행기 3종 매트릭스 · MSRV 잡 · 통합 풀 checkout 공정성/타임아웃·동시 독점 · 모든 일반 커넥션 hook 설치 · truncate/트리거 간접 write 무효화 · 스냅샷 스테일 시나리오 · `busy_timeout` · LIKE 검색 라이브 쿼리를 검증한다.

## 17. 리스크 & 완화
| 리스크 | 완화 |
|---|---|
| 자체 풀 구현 버그 | 요구 최소화(동일 권한 커넥션 N + 독점 checkout) · 동시성 스트레스 테스트 |
| 스냅샷 스테일로 잘못된 검증 | 3중 방어(§7.4) · check CLI를 CI 필수로 |
| sqlparser 커버리지 구멍 | 실패=스킵+경고(막지 않음) · `unchecked` · SQLite 관용구 테스트 |
| 동시 write 경합·풀 점유(장시간 tx) | `BEGIN IMMEDIATE` · `busy_timeout` · 짧은 tx 권고 · checkout 타임아웃/진단 로그 · 동시성 스트레스 테스트 |
| 사용자 hook 교체로 보조 경로 사망 | 문서 경고 + `rearm_hooks()` · 주 경로(문장 기반)는 영향 없음 |
| 비동기 클로저 미지원 | 0.1.0 제약 명시 · 후속 액터 설계 검토 |
| system SQLite/SQLCipher ABI·compile option 차이 | 지원 버전과 compile option을 애플리케이션 acceptance 대상으로 검증 · Windows CI는 고정 vcpkg triplet와 feature graph 검사 |

## 18. 미해결 이슈
- 디바운스/폴링 기본값, N:M `through`, 타임스탬프·Uuid 저장 표준.

- 비동기 async-클로저 트랜잭션(후속): 커넥션 액터 + 커맨드 채널 설계.

---

## 부록 A. 최소 예시

```rust
#[entity(table="todos")]
struct Todo { #[pk(autoincrement)] id: i64, title: String, done: bool }

#[dao]
trait TodoDao {
    #[insert] fn add(&self, t: &Todo) -> Result<i64>;          // PK 생략 삽입, 새 id 반환
    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> Result<Vec<Todo>>;
    #[query("SELECT * FROM todos")] fn watch(&self) -> LiveQuery<Vec<Todo>>;
}

#[database(entities(Todo), daos(TodoDao), version=1)]
struct Db;

fn main() -> roomrs::Result<()> {
    let db = Db::builder().sqlite("todo.db")
        .migrate(MigrationPolicy::Auto).build()?;
    let id = db.run_sync().todo_dao().add(&Todo{ id: 0 /*무시됨*/, title: "spec".into(), done: false })?;
    for t in db.run_sync().todo_dao().by_done(false)? { println!("- {}", t.title); }
    Ok(())
}
```

**비동기 (런타임 무관 — tokio 예시, async-std/smol 동일)**
```rust
#[tokio::main]
async fn main() -> roomrs::Result<()> {
    let db = Db::builder().sqlite("todo.db").migrate(MigrationPolicy::Auto).build_async().await?;
    let id = db.run_async().todo_dao().add(&Todo{ id: 0, title: "async".into(), done: false }).await?;
    let list = db.run_async().todo_dao().by_done(false).await?;
    Ok(())
}
```
