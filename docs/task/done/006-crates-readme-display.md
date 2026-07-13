# crates.io README 표시 수정

## 목표

0.2.1의 외부 README 경로가 crates.io `readme_file`에 인식되지 않은 문제를 package 내부 README로 수정하고 0.2.2로 배포한다.

## 범위

- 공개 6개 crate의 package 내부 `README.md`
- 각 manifest `readme`
- workspace version·CHANGELOG
- package·crates.io API 검증

## 완료 기준

- 공개 6개 crate 0.2.2의 crates.io README endpoint가 `200 OK`와 README 본문을 반환한다.

## 검증 명령

- `cargo package --workspace`
- crates.io README endpoint 조회
