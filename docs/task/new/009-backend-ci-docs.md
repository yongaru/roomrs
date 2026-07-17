# 009 backend CI and documentation

## 목표

backend 4종 검증을 CI에 고정하고 한영 공개 문서를 실제 설치·링크 계약과 일치시킨다.

## 범위

- backend 4종 CI matrix와 충돌 6종 검사
- 한영 README backend 선택·vcpkg·배포 문서
- AGENTS 기준선 갱신
- 전체 게이트, 적대적 리뷰, package/publish dry-run

## 수정 허용 파일

- `.github/workflows/ci.yml`
- `README.md`, `README-en.md`, `AGENTS.md`
- 필요 시 `CHANGELOG.md`, `roomrs-개발계획서.md`
- `docs/task/009-backend-ci-docs.md` 상태 경로

## 읽기 전용 문맥

- `docs/task/agent-common.md`
- `docs/task/done/007-backend-linkage-features.md`
- `docs/task/done/008-windows-system-backends.md`
- 기존 CI와 cross-build 정책

## 제외 범위

- consumer 저장소 수정
- crates.io 실제 배포
- git push와 tag push

## 선행 작업

- 007 backend feature contract
- 008 Windows system backend validation

## 완료 기준

- 네 backend별 check/test/clippy/fmt/doc/feature graph 경로가 CI에 있다.
- 충돌 6종 모두 기대한 compile error로 실패한다.
- README 한영 내용이 동등하고 실제 vcpkg 명령·환경·제약을 설명한다.
- 전체 기본 gate와 package/publish dry-run이 통과한다.
- 범위 밖 변경이 없다.

## 검증 명령

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo test --workspace --doc`
- backend matrix 명령 전체
- `cargo package -p roomrs --allow-dirty`
- `cargo publish -p roomrs --dry-run --allow-dirty`

## 산출물

- CI matrix
- 한영 backend 문서
- 최종 검증·리뷰 증거
