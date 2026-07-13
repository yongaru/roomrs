# crates.io README metadata

## 목표

공개 6개 크레이트의 crates.io 페이지가 README를 표시하도록 package metadata를 추가하고 0.2.1로 배포한다.

## 범위

- 공개 crate manifest의 `readme`
- workspace version·CHANGELOG
- package/dry-run·crates.io 게시 검증

## 제외 범위

- README 본문 개편
- 공개 API 변경

## 완료 기준

- `roomrs`, `roomrs-core`, `roomrs-async`, `roomrs-macros`, `roomrs-migrate`, `roomrs-cli` package가 README를 포함한다.
- crates.io API의 각 0.2.1 version `readme_file`이 비어 있지 않다.

## 검증 명령

- `cargo package --workspace`
- `cargo publish --dry-run` (의존 순서)
