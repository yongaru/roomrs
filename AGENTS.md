# roomrs 개발 지침서

> 구현 작업 규칙. 기능 명세는 [roomrs-개발계획서.md](roomrs-개발계획서.md) = 유일 진실 소스(SSOT).
> 충돌 시 **명세 우선**, 이 지침 갱신.

---

## 0. caveman 모드 (항상 활성)

모든 응답 caveman 모드. 세션 시작 시 스킬 읽고 따른다.

| 스킬 | 경로 | 언제 |
|---|---|---|
| `caveman` | `.claude/skills/caveman/SKILL.md` | 항상. 기본 레벨 `full` |
| `caveman-commit` | `.claude/skills/caveman-commit/SKILL.md` | 커밋 메시지 |
| `caveman-review` | `.claude/skills/caveman-review/SKILL.md` | 코드/diff 리뷰 |

- 관사·필러·인사말·헤징 제거. 문장 조각 허용. 기술 용어·코드·에러 문자열 원문 그대로.
- 언어 유지 — 이 프로젝트는 **한국어**.
- 스타일만 압축, 내용 손실 금지. 자기 언급 금지.
- **평문 구간**: 보안 경고 · 비가역 작업 확인 · 순서 오해 위험한 절차 · 코드/커밋본문/rustdoc/명세 · 사용자 재질문.
- **README·CHANGELOG는 caveman 제외** — 외부 독자용 문서. 친절하고 완전한 문장으로 쓴다.

해제 `stop caveman`. 레벨 `/caveman lite|full|ultra`.

---

## 1. 절대 규칙

1. **결정 로그(명세 §0) = 변경 금지 기준선.** 바꾸려면 코드보다 먼저 §0 표 갱신 + 근거.
2. **주석·에러 메시지 한국어.** 공개 rustdoc(`///`)만 영어(crates.io 공개).
3. **함수 단위 주석** — 함수마다 역할 1줄 이상.
4. **마일스톤 게이트 엄수(명세 §15).** 현 마일스톤 검증 전부 통과 전 다음 단계 코드 금지.
5. **작업 완료 = 버전 갱신 + 커밋**(§7). 작업 중간 커밋 금지, push는 명시 요청 시에만.
6. **SQLite 전용.** 다른 DB 백엔드용 추상화·제네릭 여지 금지.
7. **응답은 caveman**(§0). 코드·커밋·rustdoc·보안 경고는 평문.

## 2. 기술 기준선 (상세 = 명세 §0)

| 항목 | 값 |
|---|---|
| 코어 | rusqlite (`bundled`). r2d2 금지 |
| 풀 | 자체 미니 풀: **통합 커넥션 N — 전 커넥션 read/write 가능**. write 직렬화 = WAL + `BEGIN IMMEDIATE` + busy_timeout |
| 비동기 | 런타임 무관 std `Future`(futures-channel/core). tokio는 선택 feature. 생성 Future `+ Send` |
| MSRV | **1.85 (Edition 2024)** — CI 매트릭스 포함 |
| 에러 | thiserror `roomrs::Error` 단일 타입 |
| 라이선스 | MIT OR Apache-2.0 듀얼 |
| 매크로 | proc-macro(`roomrs-macros`), SQL 파서 `sqlparser` SQLite 방언 |

## 3. 워크스페이스

명세 §3 구조 그대로. 크레이트 추가/개명 금지(필요 시 명세 먼저).

```
crates/roomrs           파사드(재수출 전용 — 예외: feature 스위치 매크로 __if_async! 1종, 명세 §3)
crates/roomrs-core      Database · 풀 · 에러 · SqlType · Tracker · 노티파이어 · 마이그레이션 런너 ·
                        SQL/DDL 렌더 · update_hook · 멀티인스턴스 소스 (구 roomrs-sqlite 통합, 명세 결정 22)
crates/roomrs-async     비동기 파사드
crates/roomrs-macros    proc-macro 전부
crates/roomrs-migrate   SchemaSnapshot · diff · 압축 · 코드젠 (매크로와 공유)
crates/roomrs-cli       CLI (migrate diff / check / check-dir)
examples/  xtask/
```

- 의존 방향: `roomrs → {core, async, macros}`, `macros → migrate`, `async → core`, `core → migrate`. 역방향·순환 금지.
  (core → migrate는 스냅샷 모델을 매크로와 런타임이 공유하기 위해 필요)
- 공유 타입은 `roomrs-migrate`(스냅샷)·`roomrs-core`(에러·코어)에만.

## 4. 코딩 컨벤션

- Rust 2024, `cargo fmt` 기본, `cargo clippy -- -D warnings` 통과 필수.
- `unsafe` 금지. 불가피 시(FFI) `// SAFETY:` 한국어 주석.
- `unwrap()`/`expect()` 는 테스트 + "논리적 불가능" 지점만. 후자는 expect 메시지에 불가능 근거.
- panic이 라이브러리 경계 넘지 않는다 — 공개 API 전부 `Result<T, roomrs::Error>`.
- 스레드 이름: `roomrs-notifier`, `roomrs-mi-poller`, `roomrs-worker-{n}`. (구 `roomrs-writer` 전담 스레드는 통합 풀 전환으로 소멸)
- feature 조합 컴파일 보장(CI 전부): `default` · `--no-default-features --features bundled` · `--features "tokio,multi-instance"` · `cargo test --workspace`(default — 비-tokio 실행기 테스트 포함). `--all-features` 불가 — `bundled`×`cipher` 상호 배타(compile_error!).
- 매크로 에러는 원인 span에 `syn::Error`(한국어). `panic!` 금지.
- **로깅**: 라이브러리는 `log` 파사드(`error!`~`trace!`), **로그 메시지는 영어**. subscriber 초기화 금지(소비자 몫). 파라미터 값 등 민감정보 로그 금지. 예제는 `tracing` + `tracing-log` 브리지, debug 필터.

## 5. 테스트

- 마일스톤 검증 항목(명세 §15) = 최소 테스트 셋. 산출물의 일부.
- 매크로 컴파일 실패 = **trybuild**(`tests/ui/`), 성공 = 통합 테스트.
- 동시성(풀·큐·트래커) = 스트레스 + 가능 시 loom.
- 비동기 = 실행기 3종(tokio, async-std|smol, `futures::executor`) 매트릭스.
- 테스트 DB = `:memory:` 또는 `tempfile`. 리포 안 `.db` 생성 금지.
- 테스트 함수명 영어(러스트 관례) + 위에 한국어 주석 1줄.

## 6. 문서화

- 공개 rustdoc: 영어 + 예제(`cargo test --doc` 통과).
- 알려진 제약은 rustdoc에도 전파 — 예: async 클로저 트랜잭션 미지원 사유를 `run_async().transaction` 문서에.
- 비기본 feature 필요 예제는 feature 주석 명시.
- `CHANGELOG.md` keep-a-changelog 형식, 0.1.0부터. **모든 버전 갱신마다 반드시 기록**(Added/Changed/Fixed/Removed) — 작업 완료 커밋에 CHANGELOG 누락 금지.
- **README·CHANGELOG 문체**: caveman 미적용 — 외부 독자용. 친절하고 완전한 문장, 예제·맥락 충분히.
- **README 이중 언어**: `README.md`(한국어) ↔ `README-en.md`(영문) 동일 내용 유지, 상단 상호 스위칭 링크. 한쪽 수정 = 양쪽 동시 갱신.

## 7. 작업 절차 (매 작업 공통)

1. **명세 확인** — 해당 § 번호, 결정 로그와 충돌하는지.
2. 지시서 `docs/task/new` → `progress` 이동(§8).
3. 기존 코드 Read 후 수정(안 읽고 Write 금지).
4. 구현 → `cargo fmt` → `cargo clippy -- -D warnings` → 테스트 통과.
4b. **수정부 적대적 리뷰 루프**: 지시서 한 건의 수정이 끝나면
    해당 수정 부분을 적대적(비판) 코드 리뷰 → 발견 수정 → 재리뷰 → 수정 … 을
    **최대 5회**까지 반복한다. 리뷰가 깨끗해지면 즉시 종료.
    5회 후에도 발견이 남았지만 **기능상 문제가 없는 경우**(스타일·문서·개선 제안 수준)는
    일단 통과 — 잔여 발견은 지시서/리뷰 문서에 기록해 둔다.
    기능 결함(Critical/High)이 5회 후에도 남으면 통과 금지 — 보고 후 결정 받는다.
5. **버전 갱신**(§9) → **커밋**(§9) → 지시서 `progress` → `done` 이동.
6. 완료 보고. 마일스톤 완료면 명세 §15 검증 체크리스트를 증거(테스트명·출력)와 함께.

명세에 없는 설계 판단 필요 시: **임의 구현 금지.** 선택지+권고안 보고 후 결정 받는다. 결정되면 명세 §0 또는 §18에 반영.

## 7b. 작업 중지·핸드오프

- 작업을 중간에 멈출 때(사용자 중지 요청·세션 종료 임박)는 **`docs/task/HANDOFF.md`** 에 상태를 남긴다.
- 내용은 간단하게: ① 완료된 것 ② 진행 중이던 것(미커밋 변경 포함) ③ 다음 할 일 순서 ④ 검증 방법(게이트 명령) ⑤ 참조 문서.
- **새 세션/에이전트는 작업 시작 전 이 파일 존재 여부부터 확인**하고, 있으면 그대로 이어서 진행한다.
- 이어받은 작업이 완료되면 HANDOFF.md 를 삭제(또는 다음 중지 시점 내용으로 갱신)한다.

## 8. 작업지시서

파일 하나 = 작업 하나. **디렉터리 위치가 곧 상태.**

| 디렉터리 | 상태 |
|---|---|
| `docs/task/new` | 실행 전 |
| `docs/task/progress` | 진행 중 |
| `docs/task/done` | 완료 |

- 시작 시 `new` → `progress`, 완료 후 `progress` → `done`.
- 이동은 `git mv` — 복사 후 원본 삭제 금지(이력 끊김).
- `progress` 에 2개 이상 = 작업 분기. 정리하거나 이유 남긴다.
- **지시서 추가 요청 = 논블로킹**: 작업 중 지시서 추가 요청이 오면 진행 중인 작업을 중단하지 않고 `docs/task/new` 에 지시서만 작성해 큐잉한다. 착수는 현재 파이프라인 순서에 따라.

### 8.1 작업지시서 분할·에이전트 컨텍스트 제한

- 큰 작업을 한 지시서에 몰아넣지 않는다. 서로 독립 검증 가능한 작은 작업으로 나눠 `docs/task/{상태}/NNN-짧은이름.md` 파일을 각각 만든다.
- 중간 규모 작업은 구현 지시서 3~8개를 기준으로 한다. 한 지시서에 변경 이유가 둘 이상이거나, 먼 모듈·무관한 테스트를 함께 읽어야 하면 더 나눈다.
- 각 지시서는 `목표 · 범위 · 수정 허용 파일 · 읽기 전용 문맥 · 제외 범위 · 선행 작업 · 완료 기준 · 검증 명령 · 산출물`을 반드시 적는다.
- 공통 규칙·배경은 `docs/task/agent-common.md`, 전체 순서·의존성·통합 전략은 `docs/task/implementation-plan.md`에 한 번만 적는다. 개별 지시서에 긴 공통 문맥을 복제하지 않는다.
- 에이전트는 자기 지시서, `agent-common.md`, 직접 선행 작업만 읽는다. 전체 계획서·전체 코드베이스·무관한 리뷰 문서는 필요할 때만 부분 조회한다.
- 에이전트 하나는 한 번에 지시서 하나만 맡는다. 지정 범위 밖 수정이 필요하면 임의 확장하지 않고 후속 지시서를 추가한다.
- 병렬 작업은 수정 파일이 겹치지 않게 나눈다. `progress`가 2개 이상이면 각 지시서에 병렬 사유와 충돌 없는 파일 경계를 기록한다. 공유 파일(`Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, 명세, 파사드)은 한 작업에 직렬 배정한다.
- 구현, 통합, 적대적 리뷰, 전체 게이트 검증은 독립 검증 가능할 때 별도 지시서로 분리한다.
- 상위 지시서는 실행 단위가 아닌 인덱스다. 하위 지시서 링크, 의존 순서, 전체 완료 조건만 담는다. 실제 상태 이동은 하위 지시서별로 기록한다.
- 하위 지시서 파일 하나를 §9의 작업 하나로 취급한다. 각 지시서마다 버전 정책 적용, CHANGELOG 기록, 검증, 커밋, `done` 이동을 수행한다. 상위 인덱스에는 별도 버전·커밋을 만들지 않는다.
- 각 에이전트 완료 보고는 `변경 파일 · 핵심 결정 · 검증 명령/결과 · 남은 위험 · 다음 작업 전달사항`만 남긴다.

## 9. 버전 · 커밋

**작업 하나 완료 = 버전 갱신 1회 + 커밋 1개.** 순서 고정: 버전 → CHANGELOG → 커밋.

### 9.0 기본 완료 파이프라인

사용자가 구현·수정·개발 작업을 지시하면, 별도 제한이 없는 한 다음 전체 파이프라인까지 한 번에 수행한다.

1. TDD 기반 구현과 관련 문서 갱신
2. `cargo fmt` · clippy · 기능/워크스페이스/feature 게이트 검증
3. 적대적 리뷰 루프
4. 버전·CHANGELOG·README·명세·작업지시서 갱신
5. 커밋·기본 브랜치 push·릴리스 태그 push
6. `cargo package`/`cargo publish --dry-run` 검증 후 crates.io 순차 배포

사용자가 검증 전용·문서 전용·배포 제외·push 금지처럼 범위를 제한하면 그 제한을 따른다. crates.io 배포 권한 또는 registry 인증이 없으면, 배포 직전까지 완료하고 정확한 차단 사유를 보고한다.

### 9.1 시맨틱 버전 (v1.0 이전: `0.MINOR.PATCH`)

워크스페이스 전 크레이트 **버전 동일**(root `Cargo.toml` 의 `[workspace.package] version`).

| 변경 | 올릴 자리 |
|---|---|
| 공개 API 파괴적 변경(시그니처·타입·동작·제거), 마이그레이션 필요 | **MINOR** (`0.3.7` → `0.4.0`) |
| 공개 API 추가(하위 호환), 버그 수정, 성능·내부 리팩터, 문서 | **PATCH** (`0.3.7` → `0.3.8`) |
| 문서·CI·테스트만 변경(배포물 무영향) | 버전 유지 |

- **0.x 에서 MINOR = 파괴적 변경 신호**(cargo가 `0.x` 를 그렇게 취급). MAJOR 는 v1.0 전까지 항상 0.
- **v1.0 이후**:
  - 기능 추가 = **두 번째 자리(서브버전) 증가** (`1.0.5` → `1.1.0`)
  - 버그 패치·오류 패치 = **세 번째 자리 증가** (`1.0.5` → `1.0.6`)
  - **메이저 버전(첫째 자리)은 절대 건드리지 않는다** — 파괴적 변경이 있어도 올리지 않는다.
    파괴적 변경은 CHANGELOG 에 명확히 기록하는 것으로 갈음.
- 기능 마일스톤 완료 = 최소 MINOR bump(`0.1.0` → `0.2.0`).
- 버전 올리면 `CHANGELOG.md` 에 해당 항목 기록(Added/Changed/Fixed/Removed).
- crates.io publish 는 별도 명시 요청 시에만.

### 9.2 커밋

- 형식: **Conventional Commits**, caveman-commit 스킬 적용(subject ≤ 50자, 명령형, 마침표 없음).
- 타입: `feat` `fix` `docs` `test` `refactor` `perf` `build` `ci` `chore`
- 파괴적 변경: `feat!:` 또는 푸터 `BREAKING CHANGE:` — MINOR bump와 반드시 짝.
- 스코프는 크레이트명: `feat(core): …`, `fix(macros): …`
- 본문은 "왜"가 자명하지 않을 때만. **평문**(caveman 미적용).
- 버전 bump·CHANGELOG·지시서 이동은 **같은 커밋에** 포함.
- 커밋 전 `cargo fmt --check` + `clippy -D warnings` + 관련 테스트 통과 확인. 실패 상태 커밋 금지.
- 기본 브랜치 **`main`**. 작업은 `main` 직접 커밋(솔로 프로젝트). 큰 실험만 브랜치.
- `push` 는 명시 요청 시에만.

### 9.3 커밋 메시지 금지 — 도구 서명

커밋 메시지에 **AI/도구 서명을 절대 넣지 않는다.**

- ❌ `Co-Authored-By: Claude …`
- ❌ `🤖 Generated with Claude Code`
- ❌ 기타 생성 도구 언급·이모지 서명

PR 본문도 동일.

```
feat(core): 통합 풀 커넥션 상태 복구 추가

checkout 반납 시 PRAGMA 오염을 복구해야 풀 재사용이 안전하다. 명세 §10 통합 풀 구조.
```

## 10. 금지 사항

- ❌ r2d2 / deadpool / bb8 등 외부 풀
- ❌ 다른 DB 지원용 추상화
- ❌ 마일스톤 건너뛰기·병행 구현
- ❌ `#[ignore]` 필드 속성(→ `#[column(ignore)]`), `id: 0` 센티널 의미 부여
- ❌ commit_hook 사용(무효화는 commit API 성공 반환 후 방출)
- ❌ 커넥션 보유 중 같은 DB 재획득(중첩 checkout — 풀 교착 위험, 명세 §10)
- ❌ 명세 갱신 없는 공개 API 변경
- ❌ 테스트·clippy 실패 상태 커밋, 작업 중간 커밋, 무단 push
- ❌ 커밋 메시지·PR 본문에 AI/도구 서명(`Co-Authored-By: Claude`, `Generated with …`)

## 11. 현재 상태

- 현재 버전: **0.1.0**(최초 공개 준비)
- crates.io publish 미실행 — 명시 요청 시(§9.1)
- 이 절은 마일스톤마다 갱신.
