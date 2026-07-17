# backend linkage implementation plan

## 순서

1. `007-backend-linkage-features.md`: 공개 feature 계약, 충돌 차단, 명세·버전
2. `008-windows-system-backends.md`: vcpkg overlay, 실제 Windows system 링크·동작 검증
3. `009-backend-ci-docs.md`: CI matrix, 한영 문서, 전체 게이트·적대적 리뷰

각 작업은 직렬 실행한다. Cargo manifest와 공개 feature 계약은 007만 수정하고, system 테스트·overlay는 008만 수정하며, CI·README는 009만 수정한다.

## 통합 전략

- 기본 `bundled`와 `cipher`는 canonical bundled feature alias로 유지한다.
- system backend는 vcpkg 정적 triplet로 실제 링크한 증거가 있어야 완료한다.
- 마지막 작업에서 네 backend와 기존 alias, 충돌 6종을 다시 통합 검증한다.

## 전체 완료 조건

- 007~009 모두 `docs/task/done` 상태
- Windows SQLite/SQLCipher system backend 실제 검증 완료
- backend 4종 CI matrix와 문서 일치
- 커밋 완료, push·tag·실제 publish 미실행
