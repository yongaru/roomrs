//! roomrs CLI 통합 테스트 (M-24) — 빌드된 바이너리를 프로세스로 실행해 검증.
//! 픽스처는 전부 tempfile — 리포 안에 파일을 만들지 않는다.

use std::path::Path;
use std::process::{Command, Output};

/// 빌드된 roomrs 바이너리 실행 (CARGO_BIN_EXE — 통합 테스트 전용 env)
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_roomrs"))
        .args(args)
        .output()
        .expect("바이너리 실행")
}

/// stderr를 UTF-8(lossy)로 변환
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// v1 스냅샷 JSON 픽스처 — users(id)
fn snapshot_v1() -> &'static str {
    r#"{
      "version": 1,
      "tables": [{
        "name": "users",
        "columns": [
          { "name": "id", "sql_type": "INTEGER", "not_null": true, "pk": true }
        ],
        "ddl": ["CREATE TABLE \"users\" (\"id\" INTEGER PRIMARY KEY)"]
      }]
    }"#
}

/// v2 스냅샷 JSON 픽스처 — users(id, name) 컬럼 추가
fn snapshot_v2() -> &'static str {
    r#"{
      "version": 2,
      "tables": [{
        "name": "users",
        "columns": [
          { "name": "id", "sql_type": "INTEGER", "not_null": true, "pk": true },
          { "name": "name", "sql_type": "TEXT", "not_null": false, "pk": false }
        ],
        "ddl": ["CREATE TABLE \"users\" (\"id\" INTEGER PRIMARY KEY, \"name\" TEXT)"]
      }]
    }"#
}

/// 임시 디렉터리에 픽스처 파일 기록 후 경로 반환
fn write_fixture(dir: &Path, name: &str, json: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, json).expect("픽스처 기록");
    p.to_str().expect("utf-8 경로").to_string()
}

/// diff 두 스냅샷 — stdout에 SQL 초안, exit 0
#[test]
fn diff_prints_sql_to_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let old = write_fixture(dir.path(), "v1.json", snapshot_v1());
    let new = write_fixture(dir.path(), "v2.json", snapshot_v2());

    let out = run(&["migrate", "diff", &old, &new]);
    assert!(out.status.success(), "exit 0 기대: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ALTER TABLE \"users\" ADD COLUMN \"name\" TEXT"),
        "ADD COLUMN 초안 포함: {stdout}"
    );
    assert!(
        stdout.contains("PRAGMA user_version = 2"),
        "user_version 갱신 포함: {stdout}"
    );
}

/// diff out 파일 지정 — 파일 생성 + exit 0
#[test]
fn diff_writes_out_file() {
    let dir = tempfile::tempdir().unwrap();
    let old = write_fixture(dir.path(), "v1.json", snapshot_v1());
    let new = write_fixture(dir.path(), "v2.json", snapshot_v2());
    let out_path = dir.path().join("draft.sql");

    let out = run(&["migrate", "diff", &old, &new, out_path.to_str().unwrap()]);
    assert!(out.status.success(), "exit 0 기대: {out:?}");
    assert!(stderr(&out).contains("초안 저장"), "저장 안내 메시지");
    let sql = std::fs::read_to_string(&out_path).expect("초안 파일 존재");
    assert!(sql.contains("ALTER TABLE"), "초안 SQL 내용: {sql}");
}

/// diff out 파일이 이미 존재 — 덮어쓰지 않고 exit 1 (L-18)
#[test]
fn diff_refuses_to_overwrite_existing_out_file() {
    let dir = tempfile::tempdir().unwrap();
    let old = write_fixture(dir.path(), "v1.json", snapshot_v1());
    let new = write_fixture(dir.path(), "v2.json", snapshot_v2());
    let out_path = dir.path().join("draft.sql");
    std::fs::write(&out_path, "-- 수동 검토 중 초안").unwrap();

    let out = run(&["migrate", "diff", &old, &new, out_path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(
        stderr(&out).contains("이미 존재"),
        "한국어 덮어쓰기 거부 메시지: {}",
        stderr(&out)
    );
    // 기존 초안 내용 보존
    assert_eq!(
        std::fs::read_to_string(&out_path).unwrap(),
        "-- 수동 검토 중 초안",
        "기존 파일 무손상"
    );
}

/// check 동일 스냅샷 — 일치, exit 0
#[test]
fn check_identical_snapshots_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_fixture(dir.path(), "a.json", snapshot_v1());
    let b = write_fixture(dir.path(), "b.json", snapshot_v1());

    let out = run(&["migrate", "check", &a, &b]);
    assert!(out.status.success(), "exit 0 기대: {out:?}");
    assert!(stderr(&out).contains("일치"), "일치 메시지");
}

/// check 상이 스냅샷 — 불일치, exit 1
#[test]
fn check_different_snapshots_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_fixture(dir.path(), "a.json", snapshot_v1());
    let b = write_fixture(dir.path(), "b.json", snapshot_v2());

    let out = run(&["migrate", "check", &a, &b]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("불일치"), "불일치 메시지");
}

/// 존재하지 않는 파일 — exit 1 + 한국어 메시지
#[test]
fn missing_file_exit_one_with_korean_message() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_fixture(dir.path(), "a.json", snapshot_v1());
    let missing = dir.path().join("없음.json");

    let out = run(&["migrate", "check", &a, missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(
        stderr(&out).contains("스냅샷을 읽을 수 없습니다"),
        "한국어 에러 메시지: {}",
        stderr(&out)
    );
}

/// 잘못된 사용법 — exit 2 + 사용법 출력
#[test]
fn bad_usage_exit_two() {
    let out = run(&["migrate", "unknown"]);
    assert_eq!(out.status.code(), Some(2), "exit 2 기대: {out:?}");
    assert!(stderr(&out).contains("사용법"), "사용법 안내");

    // 인자 없음도 동일
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2), "exit 2 기대: {out:?}");
}

/// check-dir 정상 경로 — 버전 파일 2개, 요약 출력 + exit 0
#[test]
fn check_dir_happy_path() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    write_fixture(dir.path(), "app.2.json", snapshot_v2());
    // 무관 파일 — 무시돼야 한다
    write_fixture(dir.path(), "other.1.json", snapshot_v1());

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert!(out.status.success(), "exit 0 기대: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("스냅샷 2개 확인"), "요약 메시지: {err}");
    assert!(err.contains("v1..v2"), "버전 범위: {err}");
}

/// check-dir 파괴적 구간 — 경고 출력하되 exit 0 (검토용 보고)
#[test]
fn check_dir_reports_destructive_as_warning() {
    let dir = tempfile::tempdir().unwrap();
    // v1(id,name) -> v2(id) 로 뒤집어 컬럼 삭제 = 파괴적
    write_fixture(
        dir.path(),
        "app.1.json",
        &snapshot_v2().replace("\"version\": 2", "\"version\": 1"),
    );
    write_fixture(
        dir.path(),
        "app.2.json",
        &snapshot_v1().replace("\"version\": 1", "\"version\": 2"),
    );

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert!(out.status.success(), "경고는 실패 아님: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("파괴적 변경"), "파괴적 경고: {err}");
    assert!(err.contains("스냅샷 2개 확인"), "{err}");
}

/// check-dir 버전 갭 (v1 다음 v3) — 경고 출력하되 exit 0 (M-16)
#[test]
fn check_dir_version_gap_warns_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    // v2 없이 v3 — 중간 스냅샷 누락
    write_fixture(
        dir.path(),
        "app.3.json",
        &snapshot_v2().replace("\"version\": 2", "\"version\": 3"),
    );

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert!(out.status.success(), "경고는 실패 아님: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("버전 갭: v1 다음 v3"), "갭 경고: {err}");
    assert!(err.contains("중간 스냅샷 누락"), "갭 사유: {err}");
    assert!(err.contains("스냅샷 2개 확인"), "요약 유지: {err}");
}

/// check-dir 버전 갭 + --strict — 경고가 exit 1로 승격 (M-16)
#[test]
fn check_dir_version_gap_strict_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    write_fixture(
        dir.path(),
        "app.3.json",
        &snapshot_v2().replace("\"version\": 2", "\"version\": 3"),
    );

    let out = run(&[
        "migrate",
        "check-dir",
        dir.path().to_str().unwrap(),
        "app",
        "--strict",
    ]);
    assert_eq!(out.status.code(), Some(1), "--strict = exit 1: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("버전 갭: v1 다음 v3"), "갭 경고: {err}");
    assert!(err.contains("--strict"), "승격 안내: {err}");
}

/// check-dir 파괴적 구간 + --strict — 경고가 exit 1로 승격 (M-16)
#[test]
fn check_dir_destructive_strict_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    // v1(id,name) -> v2(id) 로 뒤집어 컬럼 삭제 = 파괴적
    write_fixture(
        dir.path(),
        "app.1.json",
        &snapshot_v2().replace("\"version\": 2", "\"version\": 1"),
    );
    write_fixture(
        dir.path(),
        "app.2.json",
        &snapshot_v1().replace("\"version\": 1", "\"version\": 2"),
    );

    let out = run(&[
        "migrate",
        "check-dir",
        dir.path().to_str().unwrap(),
        "app",
        "--strict",
    ]);
    assert_eq!(out.status.code(), Some(1), "--strict = exit 1: {out:?}");
    assert!(stderr(&out).contains("파괴적 변경"), "{}", stderr(&out));
}

/// check-dir 경고 없음 + --strict — 정상 exit 0 (승격할 경고 없음)
#[test]
fn check_dir_clean_strict_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    write_fixture(dir.path(), "app.2.json", snapshot_v2());

    let out = run(&[
        "migrate",
        "check-dir",
        dir.path().to_str().unwrap(),
        "app",
        "--strict",
    ]);
    assert!(out.status.success(), "경고 없음 = exit 0: {out:?}");
    assert!(stderr(&out).contains("스냅샷 2개 확인"), "{}", stderr(&out));
}

/// check-dir 빈 디렉토리 — 스냅샷 없음, exit 1
#[test]
fn check_dir_empty_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(
        stderr(&out).contains("스냅샷이 없습니다"),
        "{}",
        stderr(&out)
    );
}

/// check-dir 파손 파일 — exit 1 + 한국어 메시지
#[test]
fn check_dir_corrupt_file_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", "{ 이건 JSON 아님");

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(
        stderr(&out).contains("스냅샷을 읽을 수 없습니다"),
        "{}",
        stderr(&out)
    );
}

/// check-dir 파일명 버전 ↔ 내부 version 불일치 — exit 1
#[test]
fn check_dir_version_mismatch_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    // 파일명은 v3인데 내부 version = 1
    write_fixture(dir.path(), "app.3.json", snapshot_v1());

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("파일명 버전"), "{}", stderr(&out));
}

/// 유효하지 않은 JSON — exit 1
#[test]
fn invalid_json_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let bad = write_fixture(dir.path(), "bad.json", "{ 이건 JSON 아님");
    let ok = write_fixture(dir.path(), "ok.json", snapshot_v1());

    let out = run(&["migrate", "check", &bad, &ok]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(
        stderr(&out).contains("스냅샷을 읽을 수 없습니다"),
        "한국어 에러 메시지: {}",
        stderr(&out)
    );
}
