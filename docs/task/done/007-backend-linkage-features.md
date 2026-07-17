# 007 backend feature contract

## 목표

SQLite와 SQLCipher의 bundled/system 링크 방식을 독립 feature로 제공하고 잘못된 backend 조합을 컴파일 단계에서 차단한다.

## 범위

- `sqlite-bundled`, `sqlite-system`, `sqlcipher-bundled`, `sqlcipher-system` feature 추가
- 기존 `bundled`, `cipher` feature를 하위 호환 alias로 유지
- backend feature 상호 배타 검증 추가
- 결정 로그와 feature 명세 선갱신
- 버전과 CHANGELOG 갱신

## 수정 허용 파일

- `Cargo.toml`, `Cargo.lock`
- `crates/roomrs-core/Cargo.toml`, `crates/roomrs-core/src/lib.rs`
- `crates/roomrs/Cargo.toml`
- `CHANGELOG.md`, `roomrs-개발계획서.md`
- `docs/task/007-backend-linkage-features.md` 상태 경로

## 읽기 전용 문맥

- `docs/task/agent-common.md`
- rusqlite/libsqlite3-sys feature 정의
- rusqlite/libsqlite3-sys 0.38/0.36 feature 정의

## 제외 범위

- Menumon POS 저장소 feature 선언 변경
- Windows system backend 실제 링크 검증
- vcpkg overlay와 CI
- README/AGENTS 갱신
- crates.io 실제 배포

## 선행 작업

- 없음

## 완료 기준

- 기본 빌드는 기존과 같이 bundled SQLite를 사용한다.
- 새 backend feature 4종이 facade에서 core로 전달된다.
- 기존 `bundled`, `cipher` 사용자는 각각 bundled SQLite/SQLCipher를 계속 사용한다.
- 서로 다른 backend 선택은 명확한 한국어 `compile_error!`로 실패한다.
- backend 없는 `roomrs-core` 빌드가 계속 허용된다.
- 결정 로그와 feature 명세가 구현 계약과 일치한다.

## 검증 명령

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo check -p roomrs --no-default-features --features sqlite-bundled`
- 기존 alias 2종 check
- 대표 충돌 조합 2종 focused 실패 검증(6종 전체 matrix는 009에서 확정)

## 산출물

- backend feature 계약과 compile-time 검증
- 명세와 CHANGELOG

## 검증 결과

- RED: `cargo check -p roomrs --no-default-features --features sqlite-bundled`가 미지원 feature로 실패
- GREEN: backend 없는 core, `sqlite-bundled`, 기존 `bundled` check 통과
- GREEN: canonical SQLite + 공통 기능 test/clippy/fmt 통과
- GREEN: `sqlite-bundled + sqlite-system`, `sqlite-bundled + sqlcipher-system`이 지정한 한국어 `compile_error!`로 실패
- 로컬 bundled SQLCipher는 Git for Windows Perl의 `Locale::Maketext::Simple` 누락으로 OpenSSL configure 전 중단. Ubuntu CI에서 검증하도록 009에 전달

## 적대적 리뷰

- 1회차: 기능 결함 없음. canonical feature 4종, alias 전달, 6개 pair cfg, backend 없는 core 유지 확인
