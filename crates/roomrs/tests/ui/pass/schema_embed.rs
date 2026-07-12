// [명세 §8.4/결정 21c·21d] 내장 스냅샷 + 자동 마이그레이션 (안전 연산).
// 스키마 디렉토리(ROOMRS_SCHEMA_DIR)에 embed_db.1.json / embed_db.2.json 이 있고
// v1 -> v2 diff = nullable ADD COLUMN + CREATE INDEX (전부 안전 연산).
use roomrs::{DatabaseSpec, database, entity};

#[entity(table = "embed_items")]
struct EmbedItem {
    #[pk(autoincrement)]
    id: i64,
    #[column(index)]
    name: String,
    note: Option<String>,
}

#[database(entities(EmbedItem), version = 2)]
struct EmbedDb;

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

fn main() {
    // 내장 스냅샷 — 전 버전이 오름차순으로 압축 임베드된다 (결정 21c)
    let es = <EmbedDb as DatabaseSpec>::EMBEDDED_SCHEMAS;
    assert_eq!(<EmbedDb as DatabaseSpec>::DB_NAME, "embed_db", "snake_case db 이름");
    assert_eq!(es.len(), 2, "내장 스냅샷 2개");
    assert_eq!((es[0].version, es[1].version), (1, 2), "버전 오름차순");
    assert!(
        <EmbedDb as DatabaseSpec>::SNAPSHOT_HASH.is_some(),
        "현재 버전 스냅샷 해시 임베드"
    );
    for e in es {
        let s = e.snapshot().expect("압축 해제 + 파스 왕복");
        assert_eq!(s.version, e.version, "내부 version 일치");
    }

    // v1 스키마 DB를 수동 구성 (user_version = 1) — 빌드 간 유지되도록 파일 DB 사용
    let path = std::env::temp_dir().join(format!("roomrs_ui_embed_{}.db", std::process::id()));
    remove_db_files(&path);
    // 이후 assert가 실패해도 drop 순서(선언 역순)상 db가 먼저 닫히고 정리된다
    let _cleanup = DbCleanup(path.clone());
    {
        let conn = roomrs::rusqlite::Connection::open(&path).expect("v1 DB 오픈");
        let v1 = es[0].snapshot().expect("v1 스냅샷");
        for t in &v1.tables {
            for ddl in &t.ddl {
                conn.execute_batch(ddl).expect("v1 DDL 실행");
            }
        }
        conn.execute("INSERT INTO embed_items (name) VALUES ('기존행')", [])
            .expect("v1 시드");
        conn.pragma_update(None, "user_version", 1).expect("user_version=1");
    }

    // 자동 마이그레이션 옵트인 — 등록 스텝 없이 내장 diff로 v1 -> v2 (결정 21d)
    let db = EmbedDb::builder()
        .sqlite(&path)
        .auto_migrate(true)
        .build()
        .expect("안전 연산 자동 마이그레이션 성공");
    {
        let h = db.run_sync();
        let v: i64 = h.query_scalar("PRAGMA user_version", []).expect("버전 조회");
        assert_eq!(v, 2, "v2 도달");
        // 신규 컬럼·기존 행 확인
        h.execute("INSERT INTO embed_items (name, note) VALUES ('a', 'b')", [])
            .expect("신규 컬럼 insert");
        let n: i64 = h
            .query_scalar("SELECT COUNT(*) FROM embed_items", [])
            .expect("행 수 조회");
        assert_eq!(n, 2, "기존 행 보존 + 신규 행");
        // v2 인덱스가 실제로 생성됐는지
        let idx: i64 = h
            .query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_embed_items_name'",
                [],
            )
            .expect("인덱스 조회");
        assert_eq!(idx, 1, "CREATE INDEX 자동 실행");
    }
    drop(db);
    // 파일 정리는 _cleanup 가드가 담당 (정상 종료·panic 공통)
}
