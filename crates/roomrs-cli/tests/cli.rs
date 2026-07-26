//! roomrs CLI 통합 테스트 (M-24) — 빌드된 바이너리를 프로세스로 실행해 검증.
//! 픽스처는 전부 tempfile — 리포 안에 파일을 만들지 않는다.

use std::path::Path;
use std::process::{Command, Output};

/// 빌드된 cargo-roomrs 바이너리 실행 (CARGO_BIN_EXE — 통합 테스트 전용 env)
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-roomrs")).env("RUST_LOG", "info").args(args).output().expect("cargo-roomrs 실행")
}

/// 임시 소비자 프로젝트에서 cargo-roomrs 실행.
fn run_cargo_roomrs(project: &Path, target: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-roomrs")).current_dir(project).env("CARGO_TARGET_DIR", target).env("CARGO_NET_OFFLINE", "true").env("RUST_LOG", "info").args(args).output().expect("cargo-roomrs 실행")
}

/// src/main.rs만 있는 RoomRS 소비자 프로젝트를 만든다.
fn write_binary_only_consumer(project: &Path) {
    let roomrs_path = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates 디렉터리").join("roomrs").to_string_lossy().replace('\\', "/");
    let manifest = format!(
        r#"[package]
name = "binary-only-roomrs"
version = "0.1.0"
edition = "2024"

[workspace]

[dependencies]
roomrs = {{ path = "{roomrs_path}" }}
"#
    );
    std::fs::create_dir_all(project.join("src")).expect("src 생성");
    std::fs::write(project.join("Cargo.toml"), manifest).expect("manifest 기록");
    std::fs::write(
        project.join("src/main.rs"),
        r#"use roomrs::{database, entity};

#[entity(table = "notes", primary_key(id))]
struct Note {
    id: i64,
    body: String,
}

#[database(entities(Note), version = 1)]
struct AppDb;

#[entity(table = "audit_log", primary_key(id))]
struct AuditLog {
    #[pk]
    id: i64,
}

#[database(entities(AuditLog), version = 1)]
struct AuditDb;

fn main() {
    let _ = AppDb::builder;
    let _ = AuditDb::builder;
}
"#,
    )
    .expect("main 기록");
}

/// CLI 로그 출력을 UTF-8(lossy)로 변환한다.
/// subscriber 구현·실행 환경에 따라 로그 sink가 stdout/stderr로 달라질 수 있다.
fn stderr(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
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
    assert!(stdout.contains("ALTER TABLE \"users\" ADD COLUMN \"name\" TEXT"), "ADD COLUMN 초안 포함: {stdout}");
    assert!(stdout.contains("PRAGMA user_version = 2"), "user_version 갱신 포함: {stdout}");
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
    assert!(stderr(&out).contains("migration draft saved"), "저장 안내 메시지");
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
    assert!(stderr(&out).contains("already exists"), "덮어쓰기 거부 메시지: {}", stderr(&out));
    // 기존 초안 내용 보존
    assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "-- 수동 검토 중 초안", "기존 파일 무손상");
}

/// check 동일 스냅샷 — 일치, exit 0
#[test]
fn check_identical_snapshots_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_fixture(dir.path(), "a.json", snapshot_v1());
    let b = write_fixture(dir.path(), "b.json", snapshot_v1());

    let out = run(&["migrate", "check", &a, &b]);
    assert!(out.status.success(), "exit 0 기대: {out:?}");
    assert!(stderr(&out).contains("snapshots match"), "일치 메시지");
}

/// check 상이 스냅샷 — 불일치, exit 1
#[test]
fn check_different_snapshots_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_fixture(dir.path(), "a.json", snapshot_v1());
    let b = write_fixture(dir.path(), "b.json", snapshot_v2());

    let out = run(&["migrate", "check", &a, &b]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("snapshots differ"), "불일치 메시지");
}

/// 존재하지 않는 파일 — exit 1 + 한국어 메시지
#[test]
fn missing_file_exit_one_with_korean_message() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_fixture(dir.path(), "a.json", snapshot_v1());
    let missing = dir.path().join("없음.json");

    let out = run(&["migrate", "check", &a, missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("failed to read snapshot"), "스냅샷 읽기 에러 메시지: {}", stderr(&out));
}

/// 잘못된 사용법 — exit 2 + 사용법 출력
#[test]
fn bad_usage_exit_two() {
    let out = run(&["migrate", "unknown"]);
    assert_eq!(out.status.code(), Some(2), "exit 2 기대: {out:?}");
    assert!(stderr(&out).contains("usage:"), "사용법 안내");

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
    assert!(err.contains("checked 2 snapshots"), "요약 메시지: {err}");
    assert!(err.contains("v1..v2"), "버전 범위: {err}");
}

/// check-dir 파괴적 구간 — 경고 출력하되 exit 0 (검토용 보고)
#[test]
fn check_dir_reports_destructive_as_warning() {
    let dir = tempfile::tempdir().unwrap();
    // v1(id,name) -> v2(id) 로 뒤집어 컬럼 삭제 = 파괴적
    write_fixture(dir.path(), "app.1.json", &snapshot_v2().replace("\"version\": 2", "\"version\": 1"));
    write_fixture(dir.path(), "app.2.json", &snapshot_v1().replace("\"version\": 1", "\"version\": 2"));

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert!(out.status.success(), "경고는 실패 아님: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("destructive change"), "파괴적 경고: {err}");
    assert!(err.contains("checked 2 snapshots"), "{err}");
}

/// check-dir 버전 갭 (v1 다음 v3) — 경고 출력하되 exit 0 (M-16)
#[test]
fn check_dir_version_gap_warns_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    // v2 없이 v3 — 중간 스냅샷 누락
    write_fixture(dir.path(), "app.3.json", &snapshot_v2().replace("\"version\": 2", "\"version\": 3"));

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert!(out.status.success(), "경고는 실패 아님: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("snapshot version gap: v1 followed by v3"), "갭 경고: {err}");
    assert!(err.contains("checked 2 snapshots"), "요약 유지: {err}");
}

/// check-dir 버전 갭 + --strict — 경고가 exit 1로 승격 (M-16)
#[test]
fn check_dir_version_gap_strict_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    write_fixture(dir.path(), "app.3.json", &snapshot_v2().replace("\"version\": 2", "\"version\": 3"));

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app", "--strict"]);
    assert_eq!(out.status.code(), Some(1), "--strict = exit 1: {out:?}");
    let err = stderr(&out);
    assert!(err.contains("snapshot version gap: v1 followed by v3"), "갭 경고: {err}");
    assert!(err.contains("strict mode"), "승격 안내: {err}");
}

/// check-dir 파괴적 구간 + --strict — 경고가 exit 1로 승격 (M-16)
#[test]
fn check_dir_destructive_strict_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    // v1(id,name) -> v2(id) 로 뒤집어 컬럼 삭제 = 파괴적
    write_fixture(dir.path(), "app.1.json", &snapshot_v2().replace("\"version\": 2", "\"version\": 1"));
    write_fixture(dir.path(), "app.2.json", &snapshot_v1().replace("\"version\": 1", "\"version\": 2"));

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app", "--strict"]);
    assert_eq!(out.status.code(), Some(1), "--strict = exit 1: {out:?}");
    assert!(stderr(&out).contains("destructive change"), "{}", stderr(&out));
}

/// check-dir 경고 없음 + --strict — 정상 exit 0 (승격할 경고 없음)
#[test]
fn check_dir_clean_strict_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", snapshot_v1());
    write_fixture(dir.path(), "app.2.json", snapshot_v2());

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app", "--strict"]);
    assert!(out.status.success(), "경고 없음 = exit 0: {out:?}");
    assert!(stderr(&out).contains("checked 2 snapshots"), "{}", stderr(&out));
}

/// check-dir 빈 디렉토리 — 스냅샷 없음, exit 1
#[test]
fn check_dir_empty_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("no snapshots found"), "{}", stderr(&out));
}

/// check-dir 파손 파일 — exit 1 + 한국어 메시지
#[test]
fn check_dir_corrupt_file_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "app.1.json", "{ 이건 JSON 아님");

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("failed to read snapshot"), "{}", stderr(&out));
}

/// check-dir 파일명 버전 ↔ 내부 version 불일치 — exit 1
#[test]
fn check_dir_version_mismatch_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    // 파일명은 v3인데 내부 version = 1
    write_fixture(dir.path(), "app.3.json", snapshot_v1());

    let out = run(&["migrate", "check-dir", dir.path().to_str().unwrap(), "app"]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("filename version"), "{}", stderr(&out));
}

/// binary-only package도 source 수정 없이 export/check 가능.
#[test]
fn schema_export_supports_binary_only_package() {
    let dir = tempfile::tempdir().expect("임시 프로젝트");
    let project = dir.path().join("consumer");
    let target = dir.path().join("target");
    write_binary_only_consumer(&project);

    let export = run_cargo_roomrs(&project, &target, &["schema", "export"]);
    assert!(export.status.success(), "binary-only export 실패: {}", stderr(&export));
    let snapshot = project.join("migrations/schema/app_db.1.json");
    assert!(snapshot.is_file(), "snapshot 생성");
    assert!(project.join("migrations/schema/audit_db.1.json").is_file(), "같은 binary target의 두 번째 snapshot 생성");
    assert!(!project.join("src/lib.rs").exists(), "lib.rs 자동 생성 금지");

    let before = std::fs::read(&snapshot).expect("snapshot 읽기");
    let check = run_cargo_roomrs(&project, &target, &["schema", "check"]);
    assert!(check.status.success(), "binary-only check 실패: {}", stderr(&check));
    assert_eq!(std::fs::read(&snapshot).expect("check 후 snapshot"), before, "check는 snapshot 무쓰기");

    let normal_test = Command::new("cargo").current_dir(&project).env("CARGO_TARGET_DIR", &target).env("CARGO_NET_OFFLINE", "true").arg("test").output().expect("일반 cargo test");
    assert!(normal_test.status.success(), "일반 test 실패: {}", stderr(&normal_test));
    assert_eq!(std::fs::read(&snapshot).expect("test 후 snapshot"), before, "일반 test는 snapshot 무쓰기");

    std::fs::write(
        project.join("src/lib.rs"),
        r#"use roomrs::{database, entity};

#[entity(table = "library_notes")]
struct LibraryNote {
    #[pk]
    id: i64,
}

#[database(entities(LibraryNote), version = 1)]
struct LibraryDb;
"#,
    )
    .expect("lib 기록");
    let mixed_export = run_cargo_roomrs(&project, &target, &["schema", "export"]);
    assert!(mixed_export.status.success(), "lib+bin export 실패: {}", stderr(&mixed_export));
    assert!(project.join("migrations/schema/library_db.1.json").is_file(), "library target snapshot 생성");
}

/// entity-level PK와 필드 PK가 다르면 export/check 모두 쓰기 전에 실패.
#[test]
fn schema_export_rejects_conflicting_primary_key_declarations() {
    let dir = tempfile::tempdir().expect("임시 프로젝트");
    let project = dir.path().join("consumer");
    let target = dir.path().join("target");
    write_binary_only_consumer(&project);
    std::fs::write(
        project.join("src/main.rs"),
        r#"use roomrs::{database, entity};

#[entity(table = "payments", primary_key(store_id, payment_id))]
struct Payment {
    #[pk]
    store_id: String,
    payment_id: String,
    #[pk]
    sequence: i64,
}

#[database(entities(Payment), version = 1)]
struct AppDb;

fn main() {
    let _ = AppDb::builder;
}
"#,
    )
    .expect("충돌 엔티티 기록");

    let export = run_cargo_roomrs(&project, &target, &["schema", "export"]);
    assert!(!export.status.success(), "충돌 export 성공 금지");
    assert!(stderr(&export).contains("PRIMARY KEY 선언 불일치"), "{}", stderr(&export));
    assert!(!project.join("migrations").exists(), "충돌 시 schema 파일 쓰기 금지");

    let check = run_cargo_roomrs(&project, &target, &["schema", "check"]);
    assert!(!check.status.success(), "충돌 check 성공 금지");
    assert!(stderr(&check).contains("한쪽을 제거하거나 목록과 순서를 같게 맞추세요"), "{}", stderr(&check));
}

/// 유효하지 않은 JSON — exit 1
#[test]
fn invalid_json_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let bad = write_fixture(dir.path(), "bad.json", "{ 이건 JSON 아님");
    let ok = write_fixture(dir.path(), "ok.json", snapshot_v1());

    let out = run(&["migrate", "check", &bad, &ok]);
    assert_eq!(out.status.code(), Some(1), "exit 1 기대: {out:?}");
    assert!(stderr(&out).contains("failed to read snapshot"), "스냅샷 읽기 에러 메시지: {}", stderr(&out));
}

/// 도움말은 단일 cargo subcommand 형식만 안내한다.
#[test]
fn help_uses_single_cargo_roomrs_command() {
    let out = run(&["--help"]);
    assert!(out.status.success(), "도움말 실행 실패: {}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("cargo roomrs schema export"), "{stdout}");
    assert!(stdout.contains("cargo roomrs migrate diff"), "{stdout}");
    assert!(!stdout.lines().any(|line| line.trim_start().starts_with("roomrs migrate")), "직접 roomrs binary 안내 금지: {stdout}");
}
