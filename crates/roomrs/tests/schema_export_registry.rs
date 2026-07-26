//! schema export preflight / registry (결정 47/48)

use roomrs::{DatabaseSpec, PlannedExportAction, database, entity, plan_export_auto, plan_export_snapshot};

#[entity(table = "notes_export")]
struct Note {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[database(entities(Note), version = 1)]
struct ExportDb;

/// 부재 시 생성 · 동일 hash no-op · drift 는 덮어쓰지 않음
#[test]
fn plan_export_creates_missing_and_noops_same_hash() {
    let dir = tempfile::tempdir().unwrap();
    let schema_dir = dir.path().join("migrations/schema");
    // SAFETY: 테스트 격리용 ROOMRS_SCHEMA_DIR
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_DIR", &schema_dir);
    }
    let manifest = dir.path().to_str().expect("utf8 path");

    let plan = plan_export_snapshot(ExportDb::DB_NAME, ExportDb::VERSION, &ExportDb::schema(), manifest).expect("plan");
    assert!(!plan.is_noop(), "missing file should plan write");
    let path = plan.write().expect("write");
    assert!(path.exists());

    let plan2 = plan_export_snapshot(ExportDb::DB_NAME, ExportDb::VERSION, &ExportDb::schema(), manifest).expect("plan2");
    assert!(plan2.is_noop(), "same hash = no-op");
    let _ = plan2.write().expect("noop write");

    // drift: 파일 내용 변조 후 plan = SnapshotStale
    std::fs::write(&path, r#"{"version":1,"tables":[]}"#).unwrap();
    let err = plan_export_snapshot(ExportDb::DB_NAME, ExportDb::VERSION, &ExportDb::schema(), manifest).expect_err("drift");
    let msg = err.to_string();
    assert!(msg.contains("스테일") || msg.contains("stale") || msg.contains("덮어쓰지"), "{msg}");

    unsafe {
        std::env::remove_var("ROOMRS_SCHEMA_DIR");
    }
}

/// inventory 등록 — ExportDb 가 링크되면 엔트리 존재
#[test]
fn inventory_registers_export_db() {
    let _ = ExportDb::builder;
    let found = roomrs::__private::inventory::iter::<roomrs::SchemaExportEntry>.into_iter().any(|e| e.db_name == "export_db" && !e.auto);
    assert!(found, "#[database] should submit SchemaExportEntry");
}

/// version=auto — 변경 없음 no-op, 안전 ADD 는 next snapshot + SQL 초안
#[test]
fn auto_export_noop_and_safe_forward() {
    let dir = tempfile::tempdir().unwrap();
    let schema_dir = dir.path().join("migrations/schema");
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_DIR", &schema_dir);
    }
    let manifest = dir.path().to_str().expect("utf8");

    // 최초: v1 생성, SQL 없음
    let actions = plan_export_auto("export_db", &ExportDb::schema(), manifest).expect("auto v1");
    assert_eq!(actions.len(), 1);
    for a in actions {
        let _ = a.write().expect("write v1");
    }

    // 동일 엔티티 = no-op
    let actions = plan_export_auto("export_db", &ExportDb::schema(), manifest).expect("noop");
    assert!(matches!(actions.as_slice(), [PlannedExportAction::Snapshot(p)] if p.is_noop()));

    unsafe {
        std::env::remove_var("ROOMRS_SCHEMA_DIR");
    }
}
