# preupdate hook Tracker 구현

## 목표

pool connection의 preupdate 이벤트를 transaction pending 변경으로 수집하고 Tracker가 filter를 매칭하게 한다.

## 수정 허용 파일

- `crates/roomrs-core/Cargo.toml`
- `crates/roomrs-core/src/live.rs`
- `crates/roomrs-core/src/database.rs`
- `crates/roomrs-core/src/handle.rs`
- `crates/roomrs-core/src/lib.rs`
- 관련 단위 테스트

## 제외 범위

- 공개 watch API
- 멀티프로세스 제거

## 선행 작업

- `001-invalidation-filter-spec.md`

## 완료 기준

- hook이 OLD/NEW filter 컬럼 값을 안전하게 소유한다.
- commit·rollback·savepoint가 pending 변경을 정확히 처리한다.

## 검증 명령

- `cargo test -p roomrs-core`
- `cargo fmt --check`
- `cargo clippy -p roomrs-core -- -D warnings`
