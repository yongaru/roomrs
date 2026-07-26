# 다중 프로세스 LiveQuery 무효화 메모

> 상태: 현재 지원하지 않음. 공개 로드맵 범위 밖의 설계 보관 문서.

## 현재 동작

roomrs LiveQuery는 같은 프로세스 안에서 roomrs가 연 SQLite 연결의 `preupdate_hook`과 commit 성공 뒤의 Tracker 이벤트만 관찰한다. 다른 프로세스가 같은 DB 파일을 수정해도 현재 프로세스의 LiveQuery는 자동 갱신되지 않는다.

## trigger·변경 로그·poller를 사용하지 않는 이유

- 모든 write에 trigger와 로그 테이블 쓰기 비용이 추가된다.
- 사용자 스키마에 보조 테이블·trigger가 남아 migration·삭제·이름 충돌을 관리해야 한다.
- poller의 주기, 로그 정리, rollback, 대량 변경, 동시 프로세스 경합까지 별도 정합성 계약이 필요하다.
- 연결 로컬 함수나 설정에 의존하는 trigger는 raw SQLite writer에서 write 실패를 만들 수 있다.

따라서 단일 프로세스 roomrs 사용에는 hook 기반 무효화만 유지한다.

## 재검토 조건

다음 요구가 실제로 생길 때만 별도 설계를 시작한다.

- 하나의 SQLite 파일을 여러 roomrs 프로세스가 동시에 열고, 다른 프로세스의 commit 직후 LiveQuery 갱신이 필요함
- roomrs 밖의 raw SQLite writer까지 관찰해야 함

첫 경우에는 commit 성공 뒤 table/row 변경 이벤트를 전송하는 IPC 브로커와 수신 Tracker 연동을 검토한다. 두 번째 경우에는 IPC 참여 계약, trigger·로그·poller, WAL 감시 등 대안을 다시 비교하고 성능·데이터 보존·업그레이드 계약을 먼저 확정한다.

이 문서는 구현 약속이나 공개 로드맵이 아니다.
