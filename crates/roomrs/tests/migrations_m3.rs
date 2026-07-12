// M3 검증 통합 테스트 (명세 §15 M3) —
// 버전 체인 실행 · 중복/갭 에러 · destructive fallback · migrations_dir! · diff 초안

// 함수 안 #[database]가 생성하는 export 테스트(명세 §7.4)는 중첩 항목이라
// 수집 불가 — rustc unnameable_test_items 경고를 파일 단위로 허용한다.
#![allow(unnameable_test_items)]

use roomrs::{Migration, MigrationPolicy, dao, database, entity, migrations_dir, params};

// ───── v1 스키마 ─────
#[entity(table = "docs")]
struct DocV1 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
}

#[dao]
trait DocV1Dao {
    #[insert]
    fn add(&self, d: &DocV1) -> roomrs::Result<i64>;
}

#[database(entities(DocV1), daos(DocV1Dao), version = 1)]
struct Db1;

// ───── v3 스키마 (note, done 추가) ─────
#[entity(table = "docs")]
#[derive(Debug)]
struct DocV3 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    note: String,
    done: bool,
}

#[dao]
trait DocV3Dao {
    #[query("SELECT * FROM docs ORDER BY id")]
    fn all(&self) -> roomrs::Result<Vec<DocV3>>;
}

#[database(entities(DocV3), daos(DocV3Dao), version = 3)]
struct Db3;

/// v1 DB 생성 + 데이터 1건
fn make_v1(path: &std::path::Path) {
    let db = Db1::builder().sqlite(path).build().unwrap();
    db.run_sync()
        .doc_v1_dao()
        .add(&DocV1 {
            id: 0,
            title: "기존".into(),
        })
        .unwrap();
}

/// 체인 실행 (1→2→3) — 데이터 보존 + user_version 갱신
#[test]
fn chain_upgrades_and_preserves_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.db");
    make_v1(&path);

    let db = Db3::builder()
        .sqlite(&path)
        .migrate(MigrationPolicy::Auto)
        .migration(Migration::sql(
            1,
            2,
            r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#,
        ))
        .migration(Migration::sql(
            2,
            3,
            r#"ALTER TABLE "docs" ADD COLUMN "done" INTEGER NOT NULL DEFAULT 0"#,
        ))
        .build()
        .unwrap();

    let h = db.run_sync();
    let rows = h.doc_v3_dao().all().unwrap();
    assert_eq!(rows.len(), 1, "기존 데이터 보존");
    assert_eq!(rows[0].title, "기존");
    assert_eq!(rows[0].note, "");
    assert!(!rows[0].done);

    let v: i64 = h.query_scalar("PRAGMA user_version", params![]).unwrap();
    assert_eq!(v, 3, "user_version 갱신");
}

/// migrations_dir! 임베드 — 같은 체인을 SQL 파일로
#[test]
fn migrations_dir_embeds_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.db");
    make_v1(&path);

    let db = Db3::builder()
        .sqlite(&path)
        .migrations(migrations_dir!("tests/migrations_sql"))
        .build()
        .unwrap();
    assert_eq!(db.run_sync().doc_v3_dao().all().unwrap().len(), 1);
}

/// 중복 구간 = 에러
#[test]
fn duplicate_segment_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup.db");
    make_v1(&path);

    let r = Db3::builder()
        .sqlite(&path)
        .migration(Migration::sql(1, 2, "SELECT 1"))
        .migration(Migration::sql(1, 3, "SELECT 1"))
        .build();
    assert!(
        matches!(r, Err(roomrs::Error::Migration(_))),
        "중복 from = 에러"
    );
}

/// 체인 갭 = 에러, destructive fallback 옵트인 시 = drop+재생성
#[test]
fn gap_errors_and_destructive_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.db");
    make_v1(&path);

    // 갭 (스텝 없음) = 에러
    let r = Db3::builder().sqlite(&path).build();
    assert!(matches!(r, Err(roomrs::Error::Migration(_))), "갭 = 에러");

    // destructive 폴백 = 성공, 데이터 소실 + 새 스키마
    let db = Db3::builder()
        .sqlite(&path)
        .fallback_to_destructive_migration(true)
        .build()
        .unwrap();
    let h = db.run_sync();
    assert_eq!(
        h.doc_v3_dao().all().unwrap().len(),
        0,
        "파괴적 재생성 = 데이터 소실"
    );
    let v: i64 = h.query_scalar("PRAGMA user_version", params![]).unwrap();
    assert_eq!(v, 3);
    // 새 스키마로 정상 동작
    h.execute(
        "INSERT INTO docs (title, note, done) VALUES (?1, ?2, ?3)",
        params!["신규", "n", false],
    )
    .unwrap();
}

/// 코드 스텝 + MigrationStep trait 래핑
#[test]
fn code_step_and_trait_step() {
    struct AddNote;
    impl roomrs::MigrationStep for AddNote {
        fn from_version(&self) -> u32 {
            1
        }
        fn to_version(&self) -> u32 {
            2
        }
        /// note 컬럼 추가 (코드 스텝)
        fn up(&self, tx: &roomrs::Tx<'_>) -> roomrs::Result<()> {
            tx.execute_batch(r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#)
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    make_v1(&path);

    let db = Db3::builder()
        .sqlite(&path)
        .migration(Migration::from_step(AddNote))
        .migration(Migration::code(2, 3, |tx| {
            tx.execute_batch(r#"ALTER TABLE "docs" ADD COLUMN "done" INTEGER NOT NULL DEFAULT 0"#)
        }))
        .build()
        .unwrap();
    assert_eq!(db.run_sync().doc_v3_dao().all().unwrap().len(), 1);
}

/// diff 초안 — 컬럼 추가 + renamed_from 힌트 반영 (명세 §8.1/§8.3)
#[test]
fn diff_draft_generation() {
    use roomrs::DatabaseSpec;

    #[entity(table = "docs")]
    struct DocRenamed {
        #[pk(autoincrement)]
        id: i64,
        #[column(renamed_from = "title")]
        subject: String,
        note: String,
    }
    #[database(entities(DocRenamed), version = 2)]
    struct DbRenamed;

    let old = <Db1 as DatabaseSpec>::schema().to_snapshot();
    let new = <DbRenamed as DatabaseSpec>::schema().to_snapshot();
    let draft = roomrs::diff_sql(&old, &new);

    assert!(
        draft.contains(r#"RENAME COLUMN "title" TO "subject""#),
        "renamed_from 힌트 = RENAME 제안: {draft}"
    );
    assert!(
        draft.contains(r#"ADD COLUMN "note""#),
        "신규 컬럼 = ADD: {draft}"
    );
    assert!(draft.contains("PRAGMA user_version = 2"), "{draft}");
    assert!(
        !draft.contains(r#"DROP COLUMN "title""#),
        "rename된 컬럼은 DROP 제안 없음"
    );
}
