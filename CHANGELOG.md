# 변경 이력

이 프로젝트의 주요 변경 사항은 이 문서에 기록합니다.

형식은 [Keep a Changelog](https://keepachangelog.com/ko/1.1.0/)를 따르며,
버전은 [Semantic Versioning](https://semver.org/lang/ko/)을 따릅니다.

## [Unreleased]

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

[Unreleased]: https://github.com/yongaru/roomrs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/yongaru/roomrs/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/yongaru/roomrs/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/yongaru/roomrs/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/yongaru/roomrs/releases/tag/v0.1.0
