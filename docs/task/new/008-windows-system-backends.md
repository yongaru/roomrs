# 008 Windows system backend validation

## 목표

Windows MSVC에서 vcpkg 정적 SQLite/SQLCipher에 링크하고 실제 SQL, live hook, 암호화 fail-closed 동작을 검증한다.

## 범위

- system backend 통합 테스트 추가
- SQLite `session` port 설치 및 실제 링크 검증
- SQLCipher `SQLITE_ENABLE_PREUPDATE_HOOK` overlay port 추가·설치·검증
- libsqlite3-sys feature graph 자동 검사

## 수정 허용 파일

- `crates/roomrs/tests/backend_system.rs`
- `vcpkg/ports/sqlcipher/**`
- `docs/task/008-windows-system-backends.md` 상태 경로

## 읽기 전용 문맥

- `docs/task/agent-common.md`
- `docs/task/done/007-backend-linkage-features.md`
- 공식 vcpkg sqlite3/sqlcipher port와 로컬 vcpkg baseline
- 기존 roomrs live/DatabaseBuilder 테스트

## 제외 범위

- consumer 저장소 수정
- CI·README 수정
- bundled backend 구현 변경

## 선행 작업

- 007 backend feature contract

## 완료 기준

- `sqlite3[session]:x64-windows-static-md` 링크로 SQLite version, preupdate compile option, live 테스트가 통과한다.
- overlay SQLCipher 링크로 cipher version, preupdate compile option, 암호화 round-trip, wrong/no-key fail-closed, live 테스트가 통과한다.
- system feature graph에 bundled C build와 `cc`가 없다.
- 실행 불가능한 검증은 성공 처리하지 않고 blocker와 재현 명령을 기록한다.

## 검증 명령

- vcpkg SQLite/SQLCipher install 명령
- system backend별 `cargo tree -e features -i libsqlite3-sys`
- system backend별 focused integration test
- system backend별 cargo check/test/clippy/doc

## 산출물

- system backend 통합 테스트
- 최소 SQLCipher overlay port와 upstream 기준 기록
- Windows 실제 링크·동작 증거
