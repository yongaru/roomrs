// ROADMAP 우선순위 1 — 스키마 DSL 확장 (결정 40–46)
// 유효 입력 → DDL/스냅샷 필드, 런타임 CREATE, diff 분류.

use roomrs::{DiffPlan, Entity, SchemaDef, TableMeta, diff_plan, entity};

/// 복합 PK + table UNIQUE + index + FK + CHECK + sql_type
#[entity(
    table = "t_payment",
    unique(store_id, external_payment_id),
    index(name = "idx_payment_store_created", columns(store_id, created_at desc)),
    index(name = "idx_payment_active", columns(store_id), where = "deleted_at IS NULL"),
    foreign_key(
        columns(store_id, customer_id),
        references = "customers(store_id, customer_id)",
        on_delete = "CASCADE",
        on_update = "NO ACTION"
    ),
    check = "amount >= 0"
)]
struct Payment {
    #[pk]
    store_id: String,
    #[pk]
    payment_id: String,
    customer_id: String,
    external_payment_id: String,
    #[column(sql_type = "DECIMAL(12,2)")]
    amount: i64,
    created_at: String,
    deleted_at: Option<String>,
}

#[entity(table = "customers")]
struct Customer {
    #[pk]
    store_id: String,
    #[pk]
    customer_id: String,
}

/// 복합 PK DDL — table-level PRIMARY KEY, 컬럼-level PRIMARY KEY 없음
#[test]
fn composite_pk_ddl() {
    let create = Payment::DDL[0];
    assert!(create.contains("PRIMARY KEY (\"store_id\", \"payment_id\")"), "{create}");
    assert!(!create.contains("\"store_id\" TEXT PRIMARY KEY"), "{create}");
    assert!(create.contains("UNIQUE (\"store_id\", \"external_payment_id\")"), "{create}");
    assert!(create.contains("CHECK (amount >= 0)"), "{create}");
    assert!(create.contains("FOREIGN KEY (\"store_id\", \"customer_id\") REFERENCES customers(store_id, customer_id) ON DELETE CASCADE ON UPDATE NO ACTION"), "{create}");
    assert!(create.contains("\"amount\" DECIMAL(12,2)"), "{create}");
    assert!(Payment::DDL.iter().any(|d| d.contains("idx_payment_store_created") && d.contains("DESC")), "{:?}", Payment::DDL);
    assert!(Payment::DDL.iter().any(|d| d.contains("idx_payment_active") && d.contains("WHERE deleted_at IS NULL")), "{:?}", Payment::DDL);
    assert_eq!(Payment::COLUMNS_META.iter().filter(|c| c.pk).count(), 2);
    assert_eq!(Payment::COLUMNS_META.iter().find(|c| c.name == "amount").map(|c| c.sql_type), Some("DECIMAL(12,2)"));
}

/// 신규 DB — 복합 제약 DDL 실행 성공
#[test]
fn composite_schema_creates_db() {
    let conn = roomrs::rusqlite::Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    for ddl in Customer::DDL {
        conn.execute_batch(ddl).unwrap_or_else(|e| panic!("customer ddl {ddl}: {e}"));
    }
    for ddl in Payment::DDL {
        conn.execute_batch(ddl).unwrap_or_else(|e| panic!("payment ddl {ddl}: {e}"));
    }
    conn.execute(r#"INSERT INTO customers(store_id, customer_id) VALUES ('s1', 'c1')"#, []).unwrap();
    conn.execute(
        r#"INSERT INTO t_payment(store_id, payment_id, customer_id, external_payment_id, amount, created_at, deleted_at)
           VALUES ('s1', 'p1', 'c1', 'ext1', 100, '2026-01-01', NULL)"#,
        [],
    )
    .unwrap();
    // CHECK 위반
    let bad = conn.execute(
        r#"INSERT INTO t_payment(store_id, payment_id, customer_id, external_payment_id, amount, created_at, deleted_at)
           VALUES ('s1', 'p2', 'c1', 'ext2', -1, '2026-01-01', NULL)"#,
        [],
    );
    assert!(bad.is_err(), "amount < 0 은 CHECK 실패");
}

/// collate · generated · strict · without_rowid · index COLLATE (결정 54)
#[entity(
    table = "t_advanced",
    strict,
    without_rowid,
    index(name = "idx_adv_name", columns(name collate nocase, rank desc)),
)]
struct AdvancedEntity {
    #[pk]
    id: String,
    #[column(collate = "NOCASE")]
    name: String,
    price: i64,
    qty: i64,
    #[column(generated = "price * qty", stored)]
    total: i64,
    rank: i64,
}

/// 고급 schema DSL — DDL·메타·snapshot hash·diff 수동 분류
#[test]
fn advanced_schema_dsl_pipeline() {
    let create = AdvancedEntity::DDL[0];
    assert!(create.contains("COLLATE NOCASE"), "{create}");
    assert!(create.contains("GENERATED ALWAYS AS (price * qty) STORED"), "{create}");
    assert!(create.contains("STRICT"), "{create}");
    assert!(create.contains("WITHOUT ROWID"), "{create}");
    assert!(AdvancedEntity::DDL.iter().any(|d| d.contains("idx_adv_name") && d.contains("COLLATE NOCASE") && d.contains("DESC")), "{:?}", AdvancedEntity::DDL);
    const { assert!(AdvancedEntity::STRICT) };
    const { assert!(AdvancedEntity::WITHOUT_ROWID) };
    let name = AdvancedEntity::COLUMNS_META.iter().find(|c| c.name == "name").expect("name");
    assert_eq!(name.collate, Some("NOCASE"));
    let total = AdvancedEntity::COLUMNS_META.iter().find(|c| c.name == "total").expect("total");
    assert_eq!(total.generated.map(|g| (g.expr, g.stored)), Some(("price * qty", true)));

    // 런타임 CREATE 성공
    let conn = roomrs::rusqlite::Connection::open_in_memory().unwrap();
    for ddl in AdvancedEntity::DDL {
        conn.execute_batch(ddl).unwrap_or_else(|e| panic!("ddl {ddl}: {e}"));
    }
    conn.execute(r#"INSERT INTO t_advanced(id, name, price, qty, rank) VALUES ('1', 'Ab', 2, 3, 1)"#, []).unwrap();
    let total: i64 = conn.query_row("SELECT total FROM t_advanced WHERE id = '1'", [], |r| r.get(0)).unwrap();
    assert_eq!(total, 6);

    let snap = SchemaDef {
        version: 1,
        ddl: AdvancedEntity::DDL.to_vec(),
        tables: vec![TableMeta {
            name: AdvancedEntity::TABLE,
            columns: AdvancedEntity::COLUMNS_META,
            ddl: AdvancedEntity::DDL,
            strict: AdvancedEntity::STRICT,
            without_rowid: AdvancedEntity::WITHOUT_ROWID,
        }],
        triggers: vec![],
    }
    .to_snapshot();
    assert!(snap.tables[0].strict);
    assert!(snap.tables[0].without_rowid);
    assert_eq!(snap.tables[0].columns.iter().find(|c| c.name == "name").unwrap().collate.as_deref(), Some("NOCASE"));

    // collate 변경 = hash 변화 + 파괴적
    let mut other = snap.clone();
    other.tables[0].columns.iter_mut().find(|c| c.name == "name").unwrap().collate = Some("RTRIM".into());
    assert_ne!(snap.hash(), other.hash());
    let plan = diff_plan(&snap, &other);
    assert!(plan.destructive.iter().any(|d| d.contains("collate")), "{plan:?}");

    // STRICT 플래그 변경 = 파괴적
    let mut no_strict = snap.clone();
    no_strict.tables[0].strict = false;
    let plan2 = diff_plan(&snap, &no_strict);
    assert!(plan2.destructive.iter().any(|d| d.contains("STRICT") || d.contains("WITHOUT ROWID")), "{plan2:?}");

    // 기존 테이블에 generated 컬럼 추가 = 수동
    let base = roomrs::SchemaSnapshot {
        version: 1,
        tables: vec![roomrs::TableSnapshot {
            name: "t_advanced".into(),
            columns: vec![roomrs::ColumnSnapshot {
                name: "id".into(),
                sql_type: "TEXT".into(),
                not_null: true,
                pk: true,
                renamed_from: None,
                default_sql: None,
                collate: None,
                generated: None,
            }],
            ddl: vec![r#"CREATE TABLE "t_advanced" ("id" TEXT PRIMARY KEY) STRICT WITHOUT ROWID"#.into()],
            strict: true,
            without_rowid: true,
        }],
        triggers: vec![],
    };
    let plan3 = diff_plan(&base, &snap);
    assert!(plan3.destructive.iter().any(|d| d.contains("generated")), "{plan3:?}");
}

/// DEFAULT 컬럼 — DDL·메타·스냅샷·hash·diff 전 구간 정합 (결정 53)
#[entity(table = "t_with_default")]
struct WithDefault {
    #[pk(autoincrement)]
    id: i64,
    #[column(default = "active")]
    status: String,
    #[column(default = "0")]
    flags: i64,
}

/// DEFAULT 가 ColumnMeta·DDL·snapshot hash·NOT NULL ADD 안전 분류에 반영된다
#[test]
fn default_sql_pipeline_consistency() {
    let status = WithDefault::COLUMNS_META.iter().find(|c| c.name == "status").expect("status");
    assert_eq!(status.default_sql, Some("'active'"));
    assert!(WithDefault::DDL[0].contains("DEFAULT 'active'"), "{}", WithDefault::DDL[0]);
    assert!(WithDefault::DDL[0].contains("DEFAULT 0"), "{}", WithDefault::DDL[0]);

    let snap = SchemaDef {
        version: 1,
        ddl: WithDefault::DDL.to_vec(),
        tables: vec![TableMeta {
            name: WithDefault::TABLE,
            columns: WithDefault::COLUMNS_META,
            ddl: WithDefault::DDL,
            strict: false,
            without_rowid: false,
        }],
        triggers: vec![],
    }
    .to_snapshot();
    let status_col = snap.tables[0].columns.iter().find(|c| c.name == "status").expect("status col");
    assert_eq!(status_col.default_sql.as_deref(), Some("'active'"));

    // DEFAULT 변경 = hash 변화
    let mut other = snap.clone();
    other.tables[0].columns.iter_mut().find(|c| c.name == "status").unwrap().default_sql = Some("'gone'".into());
    assert_ne!(snap.hash(), other.hash());

    // NOT NULL DEFAULT 신규 컬럼 = 안전 ADD
    let old = roomrs::SchemaSnapshot {
        version: 1,
        tables: vec![roomrs::TableSnapshot {
            name: "t_with_default".into(),
            columns: vec![roomrs::ColumnSnapshot {
                name: "id".into(),
                sql_type: "INTEGER".into(),
                not_null: true,
                pk: true,
                renamed_from: None,
                default_sql: None,
                collate: None,
                generated: None,
            }],
            ddl: vec![r#"CREATE TABLE "t_with_default" ("id" INTEGER PRIMARY KEY AUTOINCREMENT)"#.into()],
            strict: false,
            without_rowid: false,
        }],
        triggers: vec![],
    };
    let plan = diff_plan(&old, &snap);
    assert!(plan.safe.iter().any(|s| s.contains(r#"ADD COLUMN "status""#) && s.contains("NOT NULL DEFAULT 'active'")), "{plan:?}");
    assert!(plan.safe.iter().any(|s| s.contains(r#"ADD COLUMN "flags""#) && s.contains("NOT NULL DEFAULT 0")), "{plan:?}");

    // DEFAULT 변경 = 수동
    let plan_change = diff_plan(&snap, &other);
    assert!(plan_change.destructive.iter().any(|d| d.contains("default")), "{plan_change:?}");
}

/// 스냅샷 round-trip + index 변경 시 hash 변화
#[test]
fn snapshot_hash_tracks_dsl() {
    let snap = SchemaDef {
        version: 1,
        ddl: Payment::DDL.to_vec(),
        tables: vec![TableMeta {
            name: Payment::TABLE,
            columns: Payment::COLUMNS_META,
            ddl: Payment::DDL,
            strict: false,
            without_rowid: false,
        }],
        triggers: vec![],
    }
    .to_snapshot();
    let json = snap.to_json().unwrap();
    let back = roomrs::SchemaSnapshot::from_slice(json.as_bytes()).unwrap();
    assert_eq!(snap.hash(), back.hash());

    let mut other = snap.clone();
    other.tables[0].ddl.push("CREATE INDEX IF NOT EXISTS \"x\" ON \"t_payment\"(\"payment_id\")".into());
    assert_ne!(snap.hash(), other.hash());
}

/// UNIQUE INDEX 추가 = 수동, 일반 INDEX 추가 = 안전
#[test]
fn diff_classifies_unique_index_manual() {
    let base_ddl = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY, "a" TEXT)"#;
    let normal = r#"CREATE INDEX IF NOT EXISTS "idx_a" ON "t"("a")"#;
    let unique = r#"CREATE UNIQUE INDEX IF NOT EXISTS "uidx_a" ON "t"("a")"#;
    let col = roomrs::ColumnSnapshot {
        name: "id".into(),
        sql_type: "INTEGER".into(),
        not_null: true,
        pk: true,
        renamed_from: None,
        default_sql: None,
        collate: None,
        generated: None,
    };
    let old = roomrs::SchemaSnapshot {
        version: 1,
        tables: vec![roomrs::TableSnapshot {
            name: "t".into(),
            columns: vec![col.clone()],
            ddl: vec![base_ddl.into()],
            strict: false,
            without_rowid: false,
        }],
        triggers: vec![],
    };
    let new_normal = roomrs::SchemaSnapshot {
        version: 2,
        tables: vec![roomrs::TableSnapshot {
            name: "t".into(),
            columns: vec![col.clone()],
            ddl: vec![base_ddl.into(), normal.into()],
            strict: false,
            without_rowid: false,
        }],
        triggers: vec![],
    };
    let new_unique = roomrs::SchemaSnapshot {
        version: 2,
        tables: vec![roomrs::TableSnapshot {
            name: "t".into(),
            columns: vec![col],
            ddl: vec![base_ddl.into(), unique.into()],
            strict: false,
            without_rowid: false,
        }],
        triggers: vec![],
    };
    let plan_n: DiffPlan = diff_plan(&old, &new_normal);
    assert_eq!(plan_n.safe, vec![normal.to_string()], "{plan_n:?}");
    let plan_u = diff_plan(&old, &new_unique);
    assert!(plan_u.safe.is_empty(), "{plan_u:?}");
    assert!(plan_u.destructive.iter().any(|d| d.contains("UNIQUE INDEX")), "{plan_u:?}");
}

/// SNAPSHOT_FILE_SEEN=false 이고 허용 env 없으면 build 실패 (결정 39)
#[test]
fn build_fails_when_snapshot_missing_without_allow() {
    use roomrs::{DatabaseSpec, Entity, SchemaDef, TableMeta};

    #[derive(Debug)]
    struct MissingSnapDb;
    impl DatabaseSpec for MissingSnapDb {
        const VERSION: u32 = 1;
        const DB_NAME: &'static str = "missing_snap_db";
        const SNAPSHOT_FILE_SEEN: bool = false;

        fn schema() -> SchemaDef {
            SchemaDef {
                version: 1,
                ddl: Customer::DDL.to_vec(),
                tables: vec![TableMeta {
                    name: Customer::TABLE,
                    columns: Customer::COLUMNS_META,
                    ddl: Customer::DDL,
                    strict: false,
                    without_rowid: false,
                }],
                triggers: vec![],
            }
        }

        fn from_database(_db: roomrs::Database) -> Self {
            MissingSnapDb
        }
    }

    // 프로세스 env 격리 — 허용 플래그 제거
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var("ROOMRS_ALLOW_MISSING_SNAPSHOT");
    }
    let err = roomrs::DatabaseBuilder::<MissingSnapDb>::default().in_memory().build();
    // 복구 — 다른 테스트와 공유 프로세스
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ROOMRS_ALLOW_MISSING_SNAPSHOT", "1");
    }
    match err {
        Err(roomrs::Error::SnapshotStale(msg)) => {
            assert!(msg.contains("스냅샷") || msg.contains("snapshot"), "{msg}");
        }
        other => panic!("SnapshotStale 기대, 결과: {other:?}"),
    }
}
