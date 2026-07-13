# InvalidationFilter 명세와 공개 모델

## 목표

`InvalidationFilter`의 테이블·AND/OR 그룹·기본 predicate 의미를 명세로 확정한다.

## 범위

- 개발계획서 결정 로그·라이브 쿼리 API 갱신
- 공개 타입과 builder의 API 계약 확정
- 기존 filter 없는 watch 호환성 명시

## 수정 허용 파일

- `roomrs-개발계획서.md`
- `CHANGELOG.md`
- `Cargo.toml`
- `docs/task/001-invalidation-filter-spec.md`

## 제외 범위

- Rust 구현
- 멀티프로세스 기능 삭제

## 완료 기준

- `eq`, `neq`, `is_null`, `is_not_null`, AND/OR의 의미가 확정된다.
- 구독·변경 매칭과 commit 경계가 명세화된다.

## 검증 명령

- `cargo fmt --check`
- `cargo clippy --workspace -- -D warnings`
