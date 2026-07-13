# 멀티프로세스 제거와 IPC 로드맵

## 목표

트리거·변경 로그·poller 기반 멀티프로세스 기능을 제거하고 IPC 후속 계획을 기록한다.

## 수정 허용 파일

- `Cargo.toml`
- `Cargo.lock`
- `crates/roomrs-core/`
- `crates/roomrs/`
- `roomrs-개발계획서.md`
- `CHANGELOG.md`
- 관련 테스트

## 제외 범위

- IPC 구현

## 선행 작업

- `003-filtered-watch-api-tests.md`

## 완료 기준

- `multi-instance` 공개 API·feature·테스트·구현이 없다.
- IPC 기반 후속 무효화 원칙이 계획서에 있다.

## 검증 명령

- `cargo test --workspace`
- `cargo fmt --check`
- `cargo clippy --workspace -- -D warnings`
