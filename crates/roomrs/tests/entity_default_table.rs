// table 인자 생략 시 struct 이름이 테이블명 (Room @Entity 기본 동작 대응)

use roomrs::{Entity, dao, database, entity, params};

#[entity] // table 생략 — "Memo" 테이블
#[derive(Debug, Clone)]
struct Memo {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[dao]
trait MemoDao {
    #[insert]
    fn add(&self, m: &Memo) -> roomrs::Result<i64>;

    // 생략된 기본 테이블명으로 SQL 작성
    #[query("SELECT * FROM Memo ORDER BY id")]
    fn all(&self) -> roomrs::Result<Vec<Memo>>;
}

#[database(entities(Memo), daos(MemoDao), version = 1)]
struct Db;

/// 기본 테이블명 = struct 이름 그대로 + CRUD 왕복
#[test]
fn default_table_is_struct_name() {
    assert_eq!(<Memo as Entity>::TABLE, "Memo");

    let db = Db::builder().in_memory().build().unwrap();
    let h = db.run_sync();
    h.memo_dao()
        .add(&Memo {
            id: 0,
            body: "기본 테이블".into(),
        })
        .unwrap();
    assert_eq!(h.memo_dao().all().unwrap().len(), 1);

    // sqlite_master에도 struct 이름 그대로
    let n: i64 = h
        .query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Memo'",
            params![],
        )
        .unwrap();
    assert_eq!(n, 1);
}
