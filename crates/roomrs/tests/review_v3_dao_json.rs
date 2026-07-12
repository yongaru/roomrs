#![cfg(feature = "json")]

use roomrs::{MigrationPolicy, SqlContext, dao, database, entity, params};

#[entity(table = "documents")]
#[derive(Debug, PartialEq)]
struct Document {
    #[pk(autoincrement)]
    id: i64,
    #[json]
    tags: Option<Vec<String>>,
}

#[dao]
trait DocumentDao {
    #[insert]
    fn add(&self, document: &Document) -> roomrs::Result<i64>;

    #[query("SELECT * FROM documents WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Document>;
}

#[database(entities(Document), daos(DocumentDao), version = 1)]
struct TestDb;

/// H-1: 0행 INSERT는 이전 rowid를 반환하지 않는다.
#[test]
fn zero_row_insert_does_not_return_stale_rowid() {
    let db = TestDb::builder()
        .sqlite(":memory:")
        .migrate(MigrationPolicy::Auto)
        .build()
        .expect("DB 생성");
    let handle = db.run_sync();
    handle
        .ctx_execute("CREATE TABLE items(id INTEGER PRIMARY KEY)", params![])
        .expect("테이블 생성");
    assert_eq!(
        handle
            .ctx_insert("INSERT INTO items(id) VALUES (1)", params![])
            .expect("첫 삽입"),
        1
    );
    let error = handle
        .ctx_insert("INSERT OR IGNORE INTO items(id) VALUES (1)", params![])
        .expect_err("0행 insert는 실패해야 함");
    assert!(matches!(error, roomrs::Error::NotFound));
}

/// H-5: JSON Option은 None/Some과 기존 SQL NULL을 왕복한다.
#[test]
fn optional_json_uses_sql_null() {
    let dir = tempfile::tempdir().expect("임시 디렉터리");
    let db = TestDb::builder()
        .sqlite(dir.path().join("json.db"))
        .migrate(MigrationPolicy::Auto)
        .build()
        .expect("DB 생성");
    let handle = db.run_sync();
    let dao = handle.document_dao();

    let none_id = dao.add(&Document { id: 0, tags: None }).expect("None 삽입");
    let some_id = dao
        .add(&Document {
            id: 0,
            tags: Some(vec!["rust".into()]),
        })
        .expect("Some 삽입");
    let null_count: i64 = handle
        .ctx_query_one(
            "SELECT COUNT(*) FROM documents WHERE tags IS NULL",
            params![],
        )
        .expect("NULL 조회");
    assert_eq!(null_count, 1);
    assert_eq!(dao.find(none_id).expect("None 로드").tags, None);
    assert_eq!(
        dao.find(some_id).expect("Some 로드").tags,
        Some(vec!["rust".into()])
    );

    handle
        .ctx_execute("INSERT INTO documents(tags) VALUES (NULL)", params![])
        .expect("기존 NULL 행 삽입");
    let existing_id: i64 = handle
        .ctx_query_one("SELECT MAX(id) FROM documents", params![])
        .expect("기존 행 ID");
    assert_eq!(dao.find(existing_id).expect("기존 NULL 로드").tags, None);

    handle
        .ctx_execute("INSERT INTO documents(tags) VALUES ('null')", params![])
        .expect("레거시 JSON null 행 삽입");
    let legacy_id: i64 = handle
        .ctx_query_one("SELECT MAX(id) FROM documents", params![])
        .expect("레거시 행 ID");
    assert_eq!(
        dao.find(legacy_id).expect("레거시 JSON null 로드").tags,
        None
    );
}
