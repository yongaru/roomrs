# InvalidationFilter 구현 순서

1. `001-invalidation-filter-spec.md` — 공개 API·명세 확정
2. `002-preupdate-tracker.md` — hook 수집·Tracker 매칭
3. `003-filtered-watch-api-tests.md` — 핸들 API·통합 테스트
4. `004-remove-multi-instance.md` — 교차 프로세스 구현 제거·IPC 로드맵

공유 파일(`Cargo.toml`, `CHANGELOG.md`, `roomrs-개발계획서.md`)은 각 작업을 직렬 수행한다.
