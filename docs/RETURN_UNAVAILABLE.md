# 반환 불가 경로와 크래시 정책

이 문서는 `Result<_, roomrs::Error>`를 반환할 수 없는 실행 경로와 처리 정책을 기록한다.

## 보장 범위

roomrs가 소유한 일반 실행 경로는 `panic!`, `unwrap`, `expect`로 프로세스를 중단하지 않는다. 실패는 `roomrs::Error`와 `ErrorPath`·`ErrorAdvice`로 반환한다. 반환값을 만들 수 없는 정리·백그라운드·FFI 경계는 안전한 복구 또는 격리 후 `log::error!`로 남긴다.

다음 외부 조건까지 Rust 라이브러리가 크래시 0%를 보장할 수는 없다.

- OS 강제 종료, 전원 손실, SIGKILL/TerminateProcess
- allocator OOM의 abort 정책
- SQLite·OpenSSL·사용자 FFI·외부 C 코드의 abort 또는 정의되지 않은 동작
- FFI 호출자가 문서 계약을 어긴 잘못된 포인터·이중 해제

이 조건은 roomrs 오류 모델 밖이며, 모바일 FFI 사용자는 유효 포인터·단일 해제 규약을 지켜야 한다.

## 반환 불가 경로

| 경로 | 반환 불가 이유 | 현재 처리 | 기록 |
|---|---|---|---|
| `ConnectionGuard::drop` | `Drop`은 `Result`를 반환할 수 없음 | rollback·PRAGMA 복구 실패 커넥션 격리, 1회 재오픈, 재오픈 실패면 풀 fatal | `error` |
| `Tx::drop` | 미완료 트랜잭션 RAII 정리 | rollback 시도. 실패하거나 커넥션을 얻지 못하면 이후 풀 반납을 계속 | `error` |
| 비동기 worker loop | 스레드 진입점이 호출자에게 `Result`를 돌려줄 수 없음 | 사용자 job panic 격리, oneshot 종료로 호출 Future에 `Error::Internal` 전달 | `error` |
| live notifier thread | 전용 스레드 진입점 | 스케줄 전용. 채널 disconnect/shutdown 시 worker 큐 close | `debug`/`info` |
| live worker thread | 전용 스레드 진입점 | 통합 풀 checkout 실패는 `error` 로그 후 다음 job. refresh/callback panic 격리 | `warn`/`error` |
| `LiveQuery` callback | 사용자 callback 반환 타입이 없음 | panic 격리 후 나머지 callback·notifier 계속 실행 | `warn` |
| 모바일 `extern "C"` | C ABI는 `Result`를 표현하지 않음 | 음수 상태 코드 또는 null 포인터 반환. DB·mutex 오류는 복구 또는 오류 코드 | `error` |
| CLI 시작 전 tracing 초기화 | `main`은 `Result` API가 아님 | `log::error!` 후 비영 exit code 반환 | `error` |
| Cargo build script | build script는 Cargo 지시문만 반환 가능 | `cargo::error=`를 출력하고 실패 exit | Cargo diagnostic |

## 구현 규칙

1. 새 공개 fallible API는 `roomrs::Result<T>`를 반환한다.
2. 반환 불가 경로는 오류를 삼키지 않는다. 안전 복구가 가능하면 수행하고 `log::error!`를 남긴다.
3. 사용자 callback panic은 `catch_unwind`로 격리한다. panic payload는 재전파하지 않는다.
4. mutex poison은 데이터 불변식이 검증된 경우에만 복구한다. 그렇지 않으면 `Error::Internal`으로 반환한다.
5. `Error::path()`와 `Error::advice()`를 사용해 호출자 로그·UI·재시도 정책을 결정한다.
