# filtered watch API와 통합 테스트

## 목표

공개 `InvalidationFilter` builder와 filtered watch API를 추가하고 변경 매칭을 통합 테스트한다.

## 수정 허용 파일

- `crates/roomrs-core/src/live.rs`
- `crates/roomrs-core/src/handle.rs`
- `crates/roomrs-core/src/lib.rs`
- `crates/roomrs/src/lib.rs`
- `crates/roomrs/tests/`

## 제외 범위

- 멀티프로세스 제거

## 선행 작업

- `002-preupdate-tracker.md`

## 완료 기준

- AND/OR·`eq`/`neq`/NULL·UPSERT·rollback 테스트가 통과한다.
- 기존 watch API 회귀가 없다.

## 검증 명령

- `cargo test --workspace`
- `cargo fmt --check`
- `cargo clippy --workspace -- -D warnings`
