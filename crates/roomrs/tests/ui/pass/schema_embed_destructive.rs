// [명세 §8.4/결정 21d] 파괴적 구간(컬럼 타입 변경)은 auto_migrate여도 자동 실행
// 금지 = 명확한 에러. 같은 구간에 수동 스텝을 등록하면 등록 스텝이 우선한다.
// 스키마 디렉토리(ROOMRS_SCHEMA_DIR)에 gadget_db.1.json(c: TEXT) /
// gadget_db.2.json(c: INTEGER) 이 있다.
use roomrs::{DatabaseSpec, Migration, database, entity};

#[entity(table = "gadgets")]
struct Gadget {
    #[pk(autoincrement)]
    id: i64,
    c: i64,
}

#[database(entities(Gadget), version = 2)]
struct GadgetDb;

/// DB 파일 + WAL 부산물 삭제
fn remove_db_files(path: &std::path::Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut os = path.as_os_str().to_owned();
        os.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(os));
    }
}

/// assert 실패(panic unwind) 경로에서도 임시 DB 파일을 정리하는 가드 (L-20)
struct DbCleanup(std::path::PathBuf);

impl Drop for DbCleanup {
    /// drop 시 DB 파일 + WAL 부산물 삭제
    fn drop(&mut self) {
        remove_db_files(&self.0);
    }
}

/// v1 스키마 DB 생성 + 시드 (user_version = 1)
fn seed_v1(path: &std::path::Path) {
    remove_db_files(path);
    let conn = roomrs::rusqlite::Connection::open(path).expect("v1 DB 오픈");
    let v1 = <GadgetDb as DatabaseSpec>::EMBEDDED_SCHEMAS[0]
        .snapshot()
        .expect("v1 스냅샷");
    for t in &v1.tables {
        for ddl in &t.ddl {
            conn.execute_batch(ddl).expect("v1 DDL 실행");
        }
    }
    conn.execute("INSERT INTO gadgets (c) VALUES ('42')", [])
        .expect("v1 시드");
    conn.pragma_update(None, "user_version", 1).expect("user_version=1");
}

fn main() {
    let es = <GadgetDb as DatabaseSpec>::EMBEDDED_SCHEMAS;
    assert_eq!(es.len(), 2, "내장 스냅샷 2개");
    let path = std::env::temp_dir().join(format!("roomrs_ui_gap_{}.db", std::process::id()));
    // 이후 assert가 실패해도 drop 순서(선언 역순)상 db가 먼저 닫히고 정리된다
    let _cleanup = DbCleanup(path.clone());

    // 1) 파괴적 구간 — 자동 실행 금지, 구체적 안내 에러
    seed_v1(&path);
    match GadgetDb::builder().sqlite(&path).auto_migrate(true).build() {
        Err(roomrs::Error::Migration(msg)) => {
            assert!(msg.contains("v1->v2 자동 마이그레이션 불가"), "구간 표시: {msg}");
            assert!(msg.contains("파괴적 변경 포함"), "사유 표시: {msg}");
            assert!(
                msg.contains("fallback_to_destructive_migration"),
                "대안 안내: {msg}"
            );
        }
        Err(other) => panic!("Migration 에러 기대, 결과: {other}"),
        Ok(_) => panic!("파괴적 구간이 자동 실행되면 안 된다"),
    }

    // 2) 등록 스텝 우선 — 수동 재작성 스텝으로 성공
    seed_v1(&path);
    let v2_ddl: Vec<String> = es[1].snapshot().expect("v2 스냅샷").tables[0].ddl.clone();
    let manual = format!(
        "ALTER TABLE \"gadgets\" RENAME TO \"gadgets_old\";\n{};\n\
         INSERT INTO \"gadgets\" (\"id\", \"c\") SELECT \"id\", CAST(\"c\" AS INTEGER) FROM \"gadgets_old\";\n\
         DROP TABLE \"gadgets_old\";",
        v2_ddl.join(";\n")
    );
    let db = GadgetDb::builder()
        .sqlite(&path)
        .auto_migrate(true)
        .migration(Migration::sql(1, 2, manual))
        .build()
        .expect("등록 스텝 우선 = 성공");
    {
        let h = db.run_sync();
        let v: i64 = h.query_scalar("PRAGMA user_version", []).expect("버전 조회");
        assert_eq!(v, 2, "v2 도달");
        let c: i64 = h
            .query_scalar("SELECT c FROM gadgets LIMIT 1", [])
            .expect("변환된 값 조회");
        assert_eq!(c, 42, "수동 스텝이 데이터 변환");
    }
    drop(db);

    // 3) auto_migrate 미사용 — 일반 체인 갭 에러 (기존 동작 유지)
    seed_v1(&path);
    match GadgetDb::builder().sqlite(&path).build() {
        Err(roomrs::Error::Migration(msg)) => {
            assert!(msg.contains("갭"), "일반 갭 에러: {msg}");
        }
        Err(other) => panic!("Migration 에러 기대, 결과: {other}"),
        Ok(_) => panic!("체인 갭이 통과되면 안 된다"),
    }
    // 파일 정리는 _cleanup 가드가 담당 (정상 종료·panic 공통)
}
