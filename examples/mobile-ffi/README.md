# Mobile FFI example / 모바일 FFI 예제

This `cdylib` demonstrates a minimal C ABI for Android JNI and iOS Swift bindings. Returned row IDs are positive; `0` means success for commands; negative values are stable errors.

| Code | Meaning |
|---|---|
| `-1` | Null input pointer |
| `-2` | Input is not valid UTF-8 |
| `-3` | Database is not open, or `roomrs_open` could not build the database |
| `-4` | Database operation failed |

`-3` means that no usable database handle exists after the call. Callers may report “database unavailable” and inspect platform-side context to distinguish an open failure from a missing prior `roomrs_open` call. This example does not expose detailed errors; production bindings should keep a thread-local error message or provide a caller-owned error buffer.

이 `cdylib`는 Android JNI와 iOS Swift 바인딩을 위한 최소 C ABI를 보여준다. 반환 row ID는 양수이며, 명령의 `0`은 성공, 음수는 안정적인 에러 코드다.

| 코드 | 의미 |
|---|---|
| `-1` | 입력 포인터가 null |
| `-2` | 입력이 유효한 UTF-8이 아님 |
| `-3` | DB가 열리지 않았거나 `roomrs_open`의 DB 빌드 실패 |
| `-4` | DB 작업 실패 |

`-3`은 호출 뒤 사용할 수 있는 DB 핸들이 없다는 뜻이다. 호출자는 “DB 사용 불가”로 처리하고, open 실패와 `roomrs_open` 미호출 구분은 플랫폼 문맥으로 판단한다. 이 예제는 상세 에러를 노출하지 않는다. 실제 바인딩은 스레드 로컬 에러 메시지나 호출자 소유 에러 버퍼를 제공해야 한다.
