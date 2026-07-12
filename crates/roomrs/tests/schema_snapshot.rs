// [명세 §7.4] write_schema_snapshot / check_schema_snapshot / export_schema_for_test
// 런타임 검증. 별도 통합 테스트 파일 = 별도 프로세스 — 다른 테스트의 env와 격리.
//
// DatabaseSpec을 수동 구현한다 — #[database] 매크로를 쓰면 export 테스트가 함께
// 생성되는데, 이 파일은 ROOMRS_SCHEMA_EXPORT env를 조작하므로 생성 테스트와의
// 경합을 원천 차단해야 한다. env는 프로세스 전역이라 전 시나리오를 단일
// #[test]에 순차로 몰아넣는다.

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
                multi_instance: <Note as Entity>::MULTI_INSTANCE,
            }],
        }
    }

    /// core Database 래핑 (이 테스트에서는 build하지 않음)
    fn from_database(_db: roomrs::Database) -> Self {
        Db
    }
}

/// D-3b 시나리오용 스펙 — 매크로 전개 시점에 현재 버전 파일이 없던 상태
/// (`SNAPSHOT_FILE_SEEN = false`)를 흉내낸다. 파일 존재+해시 일치여도
/// 재빌드(재전개) 전까지 export가 실패해야 한다 (결정 28)
struct DbUnseen;

impl DatabaseSpec for DbUnseen {
    const VERSION: u32 = 1;
    const DB_NAME: &'static str = "note_db_unseen";
    const SNAPSHOT_FILE_SEEN: bool = false;

    /// 엔티티 메타 수집 (수동 impl — Db와 동일 스키마)
    fn schema() -> SchemaDef {
        Db::schema()
    }

    /// core Database 래핑 (이 테스트에서는 build하지 않음)
    fn from_database(_db: roomrs::Database) -> Self {
        DbUnseen
    }
}

/// 스냅샷 write → check → 스테일 → export(생성/스테일/파손/옵트아웃/디렉토리 env) 전 시나리오
#[test]
fn snapshot_write_check_export() {
    // 리포 설정(.cargo/config.toml)이 export를 끄므로(=0) 명시적으로 켠다.
    // SAFETY: env는 프로세스 전역 — 이 파일의 유일한 #[test]라 동시 접근(경합) 없음.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_EXPORT", "1");
        std::env::remove_var("ROOMRS_SCHEMA_DIR");
    }

    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().to_str().unwrap();
    let expected = dir
        .path()
        .join("migrations")
        .join("schema")
        .join("note_db.1.json");

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

    // export — 스테일 파일 재생성 + 실패 반환 (CI 차단, 로컬 재생성)
    match roomrs::export_schema_for_test::<Db>(manifest) {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("커밋"), "커밋 유도 메시지: {msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }
    // 재생성됐으니 재호출 = 통과
    roomrs::export_schema_for_test::<Db>(manifest).unwrap();
    roomrs::check_schema_snapshot::<Db>(manifest).unwrap();

    // 파손 파일 = 재생성 + 실패 반환 (부재와 구분, M-19)
    std::fs::write(&written, "{ 파손된 JSON").unwrap();
    match roomrs::export_schema_for_test::<Db>(manifest) {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("파손"), "파손 구분 메시지: {msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }
    roomrs::export_schema_for_test::<Db>(manifest).unwrap();

    // 부재 — check는 에러, export는 생성 후 **실패**(커밋+재빌드 유도).
    // 신규 파일은 include_bytes 의존성 미등록이라 성공 처리하면 내장 체인이
    // 스테일한 채 남는다 (결정 28, H-6)
    std::fs::remove_file(&written).unwrap();
    assert!(
        roomrs::check_schema_snapshot::<Db>(manifest).is_err(),
        "check는 부재 = 에러"
    );
    match roomrs::export_schema_for_test::<Db>(manifest) {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("생성"), "생성 안내 메시지: {msg}");
            assert!(msg.contains("재빌드"), "재빌드 유도 메시지: {msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }
    assert!(written.exists(), "export 최초 실행 = 파일 생성");
    // 파일이 생겼으니 재호출 = 통과 (Db는 SNAPSHOT_FILE_SEEN 기본값 true)
    roomrs::export_schema_for_test::<Db>(manifest).unwrap();

    // SNAPSHOT_FILE_SEEN = false — 파일 존재+해시 일치여도 재전개 전까지 실패
    // (fail-open 창 차단, 결정 28/D-3b)
    roomrs::write_schema_snapshot::<DbUnseen>(manifest).unwrap();
    match roomrs::export_schema_for_test::<DbUnseen>(manifest) {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("반영"), "미반영 안내 메시지: {msg}");
            assert!(msg.contains("재빌드"), "재빌드 유도 메시지: {msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }

    // 옵트아웃 — ROOMRS_SCHEMA_EXPORT=0 이면 아무것도 만들지 않는다
    let dir2 = tempfile::tempdir().unwrap();
    // SAFETY: 이 파일의 유일한 #[test] 안 순차 실행 — env 동시 접근 없음.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_EXPORT", "0");
    }
    roomrs::export_schema_for_test::<Db>(dir2.path().to_str().unwrap()).unwrap();
    assert!(
        !dir2
            .path()
            .join("migrations")
            .join("schema")
            .join("note_db.1.json")
            .exists(),
        "옵트아웃 = 파일 미생성"
    );
    // SAFETY: 이 파일의 유일한 #[test] 안 순차 실행 — env 동시 접근 없음.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_EXPORT", "1");
    }

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
