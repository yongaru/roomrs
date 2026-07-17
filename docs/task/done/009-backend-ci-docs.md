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

## 버전 정책

- CI·문서·명세 동기화만 변경하므로 0.2.4를 유지한다.

## 검증 결과

- CI YAML parse 및 bundled/system matrix 각 2종, 충돌 6쌍 목록 정적 검증 통과
- `sqlite-bundled` check/test/clippy/doc와 legacy `bundled` check 통과
- Windows vcpkg `sqlite-system`, `sqlcipher-system` 전체 runtime/check/test/clippy/doc/feature graph는 008에서 통과
- `sqlcipher-bundled`와 legacy `cipher`는 Ubuntu CI에서 Perl 포함 환경으로 검증하도록 고정; 사용자 지시에 따라 WSL·Windows Perl 검증은 생략
- workspace fmt/clippy/test/doc 통과
- `cargo package --workspace --allow-dirty` 통과
- `cargo publish --workspace --dry-run --allow-dirty` 통과(실제 업로드 없음)

## 적대적 리뷰

- 1차: vcpkg checkout 경로가 저장소 overlay와 충돌하는 문제 발견 — `vcpkg-toolchain`으로 분리
- 2차: bundled SQLCipher graph의 잘못된 `sqlcipher` 기대와 alias 회귀 누락 발견 — 실제 feature명 및 alias check로 수정
- 3차: system graph가 역방향 tree만 검사해 `cc`를 놓치는 문제와 static-md CRT 설명 오류 발견 — 전체 tree 검사와 문구 수정
- 4차: CI 경로·한영 문서 동등성·명세/지침 기준선 재검토 — 추가 기능 결함 없음
