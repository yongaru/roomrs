// [명세 §7.4] write_schema_snapshot / check_schema_snapshot / export_schema_snapshot.
// export API는 명시 호출에서만 파일을 쓴다. cargo test는 생성 코드를 전개하지 않는다.

use roomrs::{DatabaseSpec, Entity, SchemaDef, TableMeta, entity};

#[entity(table = "notes")]
struct Note {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

/// 수동 스펙 — 스냅샷 파일명은 `note_db.1.json`
struct Db;

impl DatabaseSpec for Db {
    const VERSION: u32 = 1;
    const DB_NAME: &'static str = "note_db";

    /// 엔티티 메타 수집 (수동 impl)
    fn schema() -> SchemaDef {
        SchemaDef {
            version: Self::VERSION,
            ddl: <Note as Entity>::DDL.to_vec(),
            tables: vec![TableMeta {
                name: <Note as Entity>::TABLE,
                columns: <Note as Entity>::COLUMNS_META,
                ddl: <Note as Entity>::DDL,
                triggers: <Note as Entity>::TRIGGERS,
                strict: false,
                without_rowid: false,
            }],
        }
    }

    /// core Database 래핑 (이 테스트에서는 build하지 않음)
    fn from_database(_db: roomrs::Database) -> Self {
        Db
    }
}

/// 스냅샷 write → check → 스테일 → 명시 export(생성/파손/디렉토리 env) 전 시나리오
#[test]
fn snapshot_write_check_and_explicit_export() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().to_str().unwrap();
    let expected = dir.path().join("migrations").join("schema").join("note_db.1.json");

    // write — 표준 경로 {manifest}/migrations/schema/{db}.{version}.json (결정 21)
    let written = roomrs::write_schema_snapshot::<Db>(manifest).unwrap();
    assert_eq!(written, expected);
    assert!(written.exists());

    // 일치 = 통과
    roomrs::check_schema_snapshot::<Db>(manifest).unwrap();

    // 변조 = 스테일
    let mut snap = roomrs::SchemaSnapshot::read_from(&written).unwrap();
    snap.tables[0].columns[0].name = "renamed".into();
    snap.write_to(&written).unwrap();
    match roomrs::check_schema_snapshot::<Db>(manifest) {
        Err(roomrs::Error::SnapshotStale(_)) => {}
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }

    // 명시 export — 스테일 파일: 덮어쓰기 금지 + version 증가 안내 (결정 39)
    let stale_before = std::fs::read_to_string(&written).unwrap();
    match roomrs::export_schema_snapshot::<Db>(manifest) {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("version") || msg.contains("덮어쓰"), "version 증가 안내: {msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }
    let stale_after = std::fs::read_to_string(&written).unwrap();
    assert_eq!(stale_before, stale_after, "스테일 export는 파일을 덮어쓰지 않는다");

    // 올바른 스냅샷으로 복구 후 통과
    roomrs::SchemaDef {
        version: 1,
        ddl: <Note as Entity>::DDL.to_vec(),
        tables: vec![TableMeta {
            name: <Note as Entity>::TABLE,
            columns: <Note as Entity>::COLUMNS_META,
            ddl: <Note as Entity>::DDL,
            triggers: <Note as Entity>::TRIGGERS,
            strict: false,
            without_rowid: false,
        }],
    }
    .to_snapshot()
    .write_to(&written)
    .unwrap();
    roomrs::check_schema_snapshot::<Db>(manifest).unwrap();

    // 파손 파일 = 덮어쓰기 금지 (결정 39)
    let good = std::fs::read_to_string(&written).unwrap();
    std::fs::write(&written, "{ 파손된 JSON").unwrap();
    match roomrs::export_schema_snapshot::<Db>(manifest) {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("파손"), "파손 구분 메시지: {msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "{ 파손된 JSON", "파손 파일 불변");
    // 복구
    std::fs::write(&written, &good).unwrap();
    // 부재 — check는 에러, 명시 export가 생성한다.
    std::fs::remove_file(&written).unwrap();
    assert!(roomrs::check_schema_snapshot::<Db>(manifest).is_err(), "check는 부재 = 에러");
    roomrs::export_schema_snapshot::<Db>(manifest).unwrap();
    assert!(written.exists(), "명시 export 최초 실행 = 파일 생성");
    roomrs::check_schema_snapshot::<Db>(manifest).unwrap();

    // ROOMRS_SCHEMA_DIR — 디렉토리 재지정 env가 manifest 인자보다 우선 (명세 §7.2)
    let dir3 = tempfile::tempdir().unwrap();
    // SAFETY: 이 파일의 유일한 #[test] 안 순차 실행 — env 동시 접근 없음.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_DIR", dir3.path());
    }
    let w3 = roomrs::write_schema_snapshot::<Db>("무시되는-manifest").unwrap();
    assert_eq!(w3, dir3.path().join("note_db.1.json"));
    roomrs::check_schema_snapshot::<Db>("무시되는-manifest").unwrap();
    // SAFETY: 이 파일의 유일한 #[test] 안 순차 실행 — env 동시 접근 없음.
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var("ROOMRS_SCHEMA_DIR");
    }
}
