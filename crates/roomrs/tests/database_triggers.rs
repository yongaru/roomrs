// DB-level trigger 선언의 신규 DB 생성과 snapshot migration diff 회귀 테스트.

use roomrs::{Database, DatabaseBuilder, DatabaseSpec, DatabaseTriggerMeta, DatabaseTriggerSnapshot, Entity, SchemaDef, SchemaSnapshot, TableMeta, diff_plan, entity};

#[entity(table = "trigger_notes")]
struct TriggerNote {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[entity(table = "trigger_audit")]
struct TriggerAudit {
    #[pk(autoincrement)]
    id: i64,
    note_id: i64,
}

const AUDIT_TRIGGER_SQL: &str = r#"
CREATE TRIGGER trg_note_audit
AFTER INSERT ON trigger_notes
BEGIN
    INSERT INTO trigger_audit(note_id) VALUES (NEW.id);
END
"#;

struct TriggerDb {
    inner: Database,
}

impl DatabaseSpec for TriggerDb {
    const VERSION: u32 = 1;
    const DB_NAME: &'static str = "trigger_db_test";
    const SNAPSHOT_FILE_SEEN: bool = true;

    /// 테스트 DB schema를 구성한다.
    fn schema() -> SchemaDef {
        let mut ddl = TriggerNote::DDL.to_vec();
        ddl.extend_from_slice(TriggerAudit::DDL);
        SchemaDef {
            version: Self::VERSION,
            ddl,
            tables: vec![
                TableMeta {
                    name: TriggerNote::TABLE,
                    columns: TriggerNote::COLUMNS_META,
                    ddl: TriggerNote::DDL,
                    strict: TriggerNote::STRICT,
                    without_rowid: TriggerNote::WITHOUT_ROWID,
                },
                TableMeta {
                    name: TriggerAudit::TABLE,
                    columns: TriggerAudit::COLUMNS_META,
                    ddl: TriggerAudit::DDL,
                    strict: TriggerAudit::STRICT,
                    without_rowid: TriggerAudit::WITHOUT_ROWID,
                },
            ],
            triggers: vec![DatabaseTriggerMeta { name: "trg_note_audit", sql: AUDIT_TRIGGER_SQL, file: None }],
        }
    }

    /// core Database를 테스트 wrapper로 감싼다.
    fn from_database(db: Database) -> Self {
        Self { inner: db }
    }
}

/// 신규 DB 생성 시 table 뒤 DB-level trigger가 생성되고 실행된다.
#[test]
fn database_trigger_is_created_and_runs() {
    let db = DatabaseBuilder::<TriggerDb>::default().in_memory().build().unwrap();
    let handle = db.inner.run_sync();

    handle.execute("INSERT INTO trigger_notes(body) VALUES ('hello')", roomrs::params![]).unwrap();
    let count: i64 = handle.query_scalar("SELECT COUNT(*) FROM trigger_audit", roomrs::params![]).unwrap();
    assert_eq!(count, 1);
}

/// trigger 추가·변경·삭제는 실행 가능한 안전 migration SQL이 된다.
#[test]
fn database_trigger_diff_is_executable_migration() {
    let old = SchemaSnapshot {
        version: 1,
        tables: vec![],
        triggers: vec![DatabaseTriggerSnapshot {
            name: "trg_old".into(),
            sql: "CREATE TRIGGER trg_old AFTER INSERT ON t BEGIN SELECT 1; END".into(),
            file: None,
        }],
    };
    let changed = SchemaSnapshot {
        version: 2,
        tables: vec![],
        triggers: vec![
            DatabaseTriggerSnapshot {
                name: "trg_old".into(),
                sql: "CREATE TRIGGER trg_old AFTER INSERT ON t BEGIN SELECT 2; END".into(),
                file: Some("migrations/triggers/trg_old.sql".into()),
            },
            DatabaseTriggerSnapshot {
                name: "trg_new".into(),
                sql: "CREATE TRIGGER trg_new AFTER INSERT ON t BEGIN SELECT 1; END".into(),
                file: None,
            },
        ],
    };

    let changed_plan = diff_plan(&old, &changed);
    assert!(changed_plan.destructive.is_empty(), "{changed_plan:?}");
    assert!(changed_plan.safe.iter().any(|sql| sql == "DROP TRIGGER \"trg_old\""), "{changed_plan:?}");
    assert!(changed_plan.safe.iter().any(|sql| sql.contains("CREATE TRIGGER trg_old")), "{changed_plan:?}");
    assert!(changed_plan.safe.iter().any(|sql| sql.contains("CREATE TRIGGER trg_new")), "{changed_plan:?}");

    let connection = roomrs::rusqlite::Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE t(id INTEGER); CREATE TRIGGER trg_old AFTER INSERT ON t BEGIN SELECT 1; END").unwrap();
    for sql in &changed_plan.safe {
        connection.execute_batch(sql).unwrap();
    }
    let trigger_count: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'", [], |row| row.get(0)).unwrap();
    assert_eq!(trigger_count, 2);

    let removed = SchemaSnapshot { version: 3, tables: vec![], triggers: vec![] };
    let removed_plan = diff_plan(&changed, &removed);
    assert_eq!(removed_plan.safe, vec!["DROP TRIGGER \"trg_old\"".to_string(), "DROP TRIGGER \"trg_new\"".to_string()]);
    for sql in &removed_plan.safe {
        connection.execute_batch(sql).unwrap();
    }
    let trigger_count: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'", [], |row| row.get(0)).unwrap();
    assert_eq!(trigger_count, 0);
}

/// source 경로만 바뀌면 schema hash는 바뀌지만 SQL migration은 생기지 않는다.
#[test]
fn database_trigger_source_changes_hash_without_sql() {
    let inline = SchemaSnapshot {
        version: 1,
        tables: vec![],
        triggers: vec![DatabaseTriggerSnapshot {
            name: "trg_t".into(),
            sql: "CREATE TRIGGER trg_t AFTER INSERT ON t BEGIN SELECT 1; END".into(),
            file: None,
        }],
    };
    let from_file = SchemaSnapshot {
        triggers: vec![DatabaseTriggerSnapshot {
            file: Some("migrations/triggers/trg_t.sql".into()),
            ..inline.triggers[0].clone()
        }],
        ..inline.clone()
    };

    assert_ne!(inline.hash(), from_file.hash());
    let plan = diff_plan(&inline, &from_file);
    assert!(plan.safe.is_empty(), "{plan:?}");
    assert!(plan.destructive.is_empty(), "{plan:?}");
}
