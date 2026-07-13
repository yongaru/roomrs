# InvalidationFilter 공통 문맥

## 목표

단일 프로세스 LiveQuery 무효화를 `preupdate_hook` 기반 행 필터로 확장한다. 교차 프로세스 변경 로그·폴러는 제거한다.

## 공통 제약

- `InvalidationFilter`는 쿼리 빌더가 아니다. `eq`, `neq`, `is_null`, `is_not_null`, AND/OR 그룹만 지원한다.
- hook callback은 SQLite API를 재진입하지 않고, 소유한 값만 transaction pending에 기록한다.
- 통지는 최상위 transaction commit 성공 반환 뒤에만 한다.
- filter 없는 기존 LiveQuery는 테이블 단위 무효화를 유지한다.
- 복잡 SQL 분석은 하지 않는다. filter는 사용자가 명시적으로 제공한다.
- 멀티프로세스는 이번 범위에서 제거한다. 향후 IPC는 로드맵만 기록한다.

## 공통 검증

```powershell
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```
