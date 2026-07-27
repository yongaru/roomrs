# 변경 이력

이 프로젝트의 주요 변경 사항은 이 문서에 기록합니다.

형식은 [Keep a Changelog](https://keepachangelog.com/ko/1.1.0/)를 따르며,
버전은 [Semantic Versioning](https://semver.org/lang/ko/)을 따릅니다.

## [Unreleased]

## [0.4.1] - 2026-07-27

### Changed

- README, 상세 사용 가이드, 동기 Todo 예제에 `query_one`, `query_optional`, `query_all`, `query_scalar`, `execute`와 `UPDATE ... RETURNING`으로 SQL 문자열을 직접 실행하는 방법을 추가했습니다.

### Fixed

- trigger SQL 파일의 CRLF/CR 줄바꿈을 LF로 정규화해 Windows와 Unix 계열 환경에서 동일한 schema snapshot hash를 사용합니다.
- schema export registry 테스트의 전역 환경변수 경합을 제거했습니다.
- LiveQuery worker 병렬성 테스트의 800만 행 재귀 SQL을 결정적 채널 동기화로 교체해 CPU 성능에 따른 타임아웃을 제거했습니다.
- fixed debounce 테스트의 scheduler 허용 범위를 넓혀 macOS CI의 시간 경합을 제거했습니다.

## [0.4.0] - 2026-07-27

### Added

- `#[database]`에서 DB-level SQLite trigger를 inline SQL 또는 package 기준 SQL 파일로 선언할 수 있습니다.
- trigger 이름·전체 SQL·source를 schema snapshot에 보존하고, 추가·수정·삭제를 안전한 forward migration으로 자동 합성합니다.

### Changed

- README와 상세 사용 가이드에 `#[entity]` 없이 일반 구조체와 `FromRow`로 JOIN·집계·projection SELECT 결과를 받는 DAO·직접 조회 예제를 추가했습니다.
- 신규 DB와 파괴적 fallback은 table·index 생성 뒤 DB-level trigger를 생성합니다.

### Removed

- 미공개 `#[entity(trigger = "path")]`, `TriggerMeta`, `TriggerSnapshot` API를 제거했습니다. DB-level `#[database(trigger(name = "...", sql = "..." | file = "..."))]`를 사용해야 합니다.

## [0.3.0] - 2026-07-27

### Added

- 복합 `#[pk]`, `#[entity(primary_key(...))]`, table `UNIQUE`, 복합·정렬·partial index, 복합 foreign key, `CHECK`, custom SQL column type, trigger file hook을 포함하는 SQLite schema DSL을 제공합니다.
- `collate`, generated column, `STRICT`, `WITHOUT ROWID`, index column `COLLATE`를 DDL·snapshot·hash·diff에 일관되게 반영합니다.
- `#[column(default = "...")]`의 SQL을 snapshot에 보존하고, `NOT NULL DEFAULT` 신규 컬럼을 안전한 forward migration으로 분류합니다.
- `cargo roomrs schema export`와 `cargo roomrs schema check`가 workspace의 library·binary target에서 `#[database]`를 자동 탐색합니다. 일반 `cargo build`와 `cargo test`는 schema 파일을 쓰지 않습니다.
- `#[database(version = auto)]`가 최신 snapshot과 entity hash를 비교해 변경 시 다음 revision JSON과 검토용 forward migration SQL 초안을 생성합니다.
- sync·async·DAO LiveQuery에 행 필터 API를 대칭 제공하며, 여러 `InvalidationFilter`를 OR 조건으로 결합할 수 있습니다.
- LiveQuery에 DB 전역 및 observer별 debounce 설정, 기본 250ms 고정 coalesce 창, 통합 connection pool을 사용하는 bounded 재조회 worker pool을 제공합니다.
- `Database::live_metrics()`와 filter schema 검증을 제공해 LiveQuery 수신·병합·재조회 상태와 설정 오류를 확인할 수 있습니다.
- 모든 `roomrs::Error`에서 발생 영역 `ErrorPath`와 권장 조치 `ErrorAdvice`를 조회할 수 있습니다.
- SQLCipher vcpkg overlay와 Windows system backend 검증·설치 스크립트를 제공합니다.

### Changed

- 기본 feature는 bundled SQLite, async, tokio, live, time, uuid, json입니다. SQLCipher는 명시적으로 선택합니다.
- CLI 실행파일을 `cargo-roomrs` 하나로 통합하고 schema·migration 명령을 `cargo roomrs ...` 형식으로 제공합니다.
- README를 빠른 시작 중심의 일반 오픈소스 구조로 재구성하고, 설치부터 스키마 변경·운영까지 설명하는 한·영 상세 사용 가이드를 추가했습니다.
- 라이브러리 로그는 `log` 파사드만 사용하며, 예제와 CLI는 `RUST_LOG`를 지원하는 `tracing-log` 브리지를 사용합니다.
- `#[pk]`를 여러 필드에 지정하면 필드 선언 순서대로 복합 PRIMARY KEY를 생성합니다. `#[pk(autoincrement)]`는 단일 정수 키에서만 허용합니다.
- schema snapshot export는 기존 같은 revision을 덮어쓰지 않으며, hash 불일치·파손·파괴적 diff를 구조화된 오류와 수정 조언으로 보고합니다.
- LiveQuery notifier는 이벤트 병합과 작업 제출만 담당하고, 재조회는 `roomrs-live-worker-{n}`이 통합 read/write pool에서 connection을 빌려 수행합니다.

### Fixed

- 비테스트 실행 경로의 panic 가능 연산을 구조화된 오류 반환 또는 안전 복구와 `log::error!` 기록으로 전환했습니다.
- callback, `Drop`, background thread, FFI처럼 `Result`를 반환할 수 없는 경계에서 panic을 격리하고 종료·복구 정책을 명시했습니다.
- LiveQuery callback 안에서 `Database`를 drop할 때 worker 종료 대기가 교착되는 문제를 수정했습니다.
- trigger SQL 파일 내용 변경이 macro 재전개와 schema content hash에 반영됩니다.
- SQLCipher overlay의 host Tcl, header 설치 경로, 정적 OpenSSL link와 Windows CRT 구성을 정합화했습니다.

## [0.2.4] - 2026-07-17

### Added

- Windows MSVC에서 vcpkg의 정적 SQLite 및 SQLCipher를 사용하는 system backend 통합 검증을 추가했습니다.
- SQLCipher 공식 vcpkg port에 preupdate hook을 활성화하는 최소 overlay port를 제공합니다.

### Fixed

- canonical SQLCipher feature를 직접 선택해도 암호화 키가 모든 연결에 먼저 적용되도록 수정했습니다.
- 정적 SQLCipher의 OpenSSL 의존 라이브러리가 Windows MSVC 링크 단계에 전달되도록 수정했습니다.

## [0.2.3] - 2026-07-17

### Added

- SQLite와 SQLCipher에 각각 bundled/system canonical backend feature를 추가했습니다. 기존 `bundled`와 `cipher`는 동일한 bundled 동작을 유지하는 하위 호환 alias입니다.
- 서로 다른 backend feature 두 개 이상이 동시에 활성화되면 명확한 컴파일 오류로 차단합니다.

## [0.2.2] - 2026-07-14

### Fixed

- 공개 6개 crate의 README를 package 내부 파일로 제공해 crates.io 표시 metadata가 인식되도록 수정했습니다.

## [0.2.1] - 2026-07-14

### Fixed

- 공개 6개 crate의 package metadata에 README를 지정해 crates.io 페이지에 문서를 표시합니다.

## [0.2.0] - 2026-07-14

### Added

- 단일 프로세스 라이브 쿼리용 `InvalidationFilter`와 AND/OR 조건 그룹, `eq`, `neq`, `is_null`, `is_not_null` 조건을 추가했습니다.
- `watch_scalar_filtered`가 SQLite `preupdate_hook`의 OLD/NEW 행을 이용해 관련 변경만 재조회합니다. commit 전 변경은 보류하고 rollback 변경은 버립니다.

### Removed

- `multi-instance` feature, `#[entity(multi_instance)]`, 멀티프로세스 trigger·변경 로그·poller와 관련 예제를 제거했습니다. 교차 프로세스 무효화는 향후 IPC 브로커 로드맵으로 보류합니다.

## [0.1.2] - 2026-07-14

### Added

- `InvalidationFilter`의 AND/OR 그룹, `eq`, `neq`, `is_null`, `is_not_null` 조건과 `watch_scalar_filtered`를 추가했습니다.

### Changed

- 라이브 쿼리 행 무효화를 SQLite `preupdate_hook`의 OLD/NEW 값으로 판정하도록 변경했습니다. 커밋 전 변경은 보류하고 rollback 변경은 버립니다.

## [0.1.1] - 2026-07-14

### Added

- 단일 프로세스 라이브 쿼리용 `InvalidationFilter` 공개 계약과 `preupdate_hook` 기반 행 필터 무효화 설계를 문서화했습니다.

## [0.1.0] - 2026-07-13

### Added

- SQLite 전용 로컬 퍼시스턴스용 `#[entity]`, `#[dao]`, `#[database]` proc-macro와 CRUD 매크로(`#[query]`, `#[insert]`, `#[update]`, `#[delete]`)를 제공합니다.
- 모든 일반 커넥션이 읽기와 쓰기를 수행할 수 있는 자체 통합 풀, FIFO checkout, 큐 타임아웃, WAL, `busy_timeout`, `BEGIN IMMEDIATE` 트랜잭션을 제공합니다.
- 동기 핸들과 실행기 독립 `Future + Send` 비동기 핸들을 제공하며 tokio, async-std, smol, `futures::executor`를 지원합니다.
- 클로저·RAII·DAO `#[transaction]` 트랜잭션과 중첩 savepoint를 제공합니다.
- 동기 수신·콜백 및 비동기 `Stream` 소비가 가능한 `LiveQuery<T>`와 commit 이후 테이블 무효화를 제공합니다.
- 테이블별 옵트인 멀티 인스턴스 무효화로 같은 SQLite 파일을 사용하는 다른 프로세스의 변경을 감지합니다.
- 버전별 스키마 스냅샷, 컴파일 타임 SQL·파라미터 검증, 스냅샷 export·스테일 검증을 제공합니다.
- 인라인 SQL, 코드 스텝, SQL 디렉터리, 스냅샷 diff 기반 자동 마이그레이션과 destructive fallback을 제공합니다.
- `roomrs migrate diff`, `check`, `check-dir` CLI를 제공합니다.
- 1:1, 1:N, N:M 관계 매핑과 관계 뷰용 `#[embedded]`를 제공합니다.
- 스키마 인지 동적 쿼리 빌더와 직접 SQL 실행·조회 API를 제공합니다.
- rusqlite `ToSql`/`FromSql`, `#[json]`, `#[derive(SqlType)]`, `time`, `uuid` 타입 매핑을 제공합니다.
- `on_create`, `on_open`, query logger, `log` 파사드 기반 운영 훅을 제공합니다.
- bundled SQLite를 기본 제공하고 선택적 SQLCipher(`cipher`) 및 데스크톱·모바일 크로스 빌드를 지원합니다.
- Rust 1.85와 Edition 2024를 지원하며 MIT OR Apache-2.0 듀얼 라이선스로 배포합니다.

[Unreleased]: https://github.com/yongaru/roomrs/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/yongaru/roomrs/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/yongaru/roomrs/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/yongaru/roomrs/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/yongaru/roomrs/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/yongaru/roomrs/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/yongaru/roomrs/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/yongaru/roomrs/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/yongaru/roomrs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/yongaru/roomrs/releases/tag/v0.1.0
