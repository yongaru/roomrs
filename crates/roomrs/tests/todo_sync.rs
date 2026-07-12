// M1 검증 통합 테스트 (명세 §15 M1) —
// todo CRUD · PK 생략 삽입+rowid 반환 · write 직렬성 · 통합 풀 · 트랜잭션

// 함수 안 #[database]가 생성하는 export 테스트(명세 §7.4)는 중첩 항목이라
// 수집 불가 — rustc unnameable_test_items 경고를 파일 단위로 허용한다.
#![allow(unnameable_test_items)]

use roomrs::{MigrationPolicy, dao, database, entity, params};

#[entity(table = "todos")]
#[derive(Debug, Clone, PartialEq)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, t: &Todo) -> roomrs::Result<i64>;

    #[insert(on_conflict = "replace")]
    fn upsert(&self, t: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Option<Todo>>;

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;

    #[update("UPDATE todos SET done = :done WHERE id = :id")]
    fn set_done(&self, id: i64, done: bool) -> roomrs::Result<u64>;

    #[delete("DELETE FROM todos WHERE id = :id")]
    fn remove(&self, id: i64) -> roomrs::Result<u64>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

/// 테스트 DB — 임시 파일 (WAL 경로 포함 검증), 리포 밖 tempdir
fn open_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("tempdir 생성 실패");
    let db = Db::builder()
        .sqlite(dir.path().join("test.db"))
        .migrate(MigrationPolicy::Auto)
        .build()
        .expect("DB 오픈 실패");
    (dir, db)
}

/// CRUD 왕복 — insert/select/update/delete
#[test]
fn crud_roundtrip() {
    let (_dir, db) = open_db();
    let h = db.run_sync();
    let dao = h.todo_dao();

    // C — PK 생략 삽입, 새 rowid 반환 (id:0은 센티널이 아님 — 무시됨)
    let id1 = dao
        .add(&Todo {
            id: 0,
            title: "첫번째".into(),
            done: false,
        })
        .unwrap();
    let id2 = dao
        .add(&Todo {
            id: 0,
            title: "두번째".into(),
            done: false,
        })
        .unwrap();
    assert!(id1 > 0, "rowid는 1 이상");
    assert_eq!(id2, id1 + 1, "autoincrement 연속");

    // R
    let t = dao.find(id1).unwrap().expect("존재해야 함");
    assert_eq!(t.title, "첫번째");
    assert!(!t.done);
    assert_eq!(dao.by_done(false).unwrap().len(), 2);

    // U
    assert_eq!(dao.set_done(id1, true).unwrap(), 1);
    assert!(dao.find(id1).unwrap().unwrap().done);
    assert_eq!(dao.by_done(false).unwrap().len(), 1);

    // D
    assert_eq!(dao.remove(id1).unwrap(), 1);
    assert!(dao.find(id1).unwrap().is_none());
}

/// 직접 쿼리 API (명세 §5.7)
#[test]
fn direct_query_api() {
    let (_dir, db) = open_db();
    let h = db.run_sync();

    let n = h
        .execute(
            "INSERT INTO todos (title, done) VALUES (?1, ?2)",
            params!["직접", false],
        )
        .unwrap();
    assert_eq!(n, 1);

    let cnt: i64 = h
        .query_scalar("SELECT COUNT(*) FROM todos", params![])
        .unwrap();
    assert_eq!(cnt, 1);

    let row: (i64, String) = h
        .query_one("SELECT id, title FROM todos LIMIT 1", params![])
        .unwrap();
    assert_eq!(row.1, "직접");

    let missing: Option<(i64, String)> = h
        .query_optional("SELECT id, title FROM todos WHERE id = ?1", params![9999])
        .unwrap();
    assert!(missing.is_none());

    // query_one 0건 = NotFound (명세 §5.2)
    let err = h
        .query_one::<(i64, String), _>("SELECT id, title FROM todos WHERE id = ?1", params![9999])
        .unwrap_err();
    assert!(matches!(err, roomrs::Error::NotFound));
}

/// 다중 스레드 동시 insert가 손실 없이 전량 성공한다.
#[test]
fn concurrent_inserts_complete_without_loss() {
    let (_dir, db) = open_db();
    let db = std::sync::Arc::new(db);

    const THREADS: usize = 8;
    const PER_THREAD: usize = 50;

    std::thread::scope(|s| {
        for t in 0..THREADS {
            let db = db.clone();
            s.spawn(move || {
                let h = db.run_sync();
                let dao = h.todo_dao();
                for i in 0..PER_THREAD {
                    dao.add(&Todo {
                        id: 0,
                        title: format!("t{t}-{i}"),
                        done: false,
                    })
                    .expect("동시 insert 실패");
                }
            });
        }
    });

    let cnt: i64 = db
        .run_sync()
        .query_scalar("SELECT COUNT(*) FROM todos", params![])
        .unwrap();
    assert_eq!(cnt as usize, THREADS * PER_THREAD, "insert 손실 없음");
}

/// 풀에서 checkout한 모든 커넥션은 CUD를 실행할 수 있다.
#[test]
fn every_checked_out_connection_accepts_cud() {
    let (_dir, db) = open_db();
    let h = db.run_sync();

    h.with_connection(|conn| {
        conn.execute("INSERT INTO todos (title, done) VALUES ('통합 풀', 0)", [])?;
        conn.execute("UPDATE todos SET done = 1 WHERE title = '통합 풀'", [])?;
        Ok(())
    })
    .expect("통합 풀 커넥션 CUD");

    let count: i64 = h
        .query_scalar("SELECT count(*) FROM todos WHERE done = 1", params![])
        .expect("CUD 결과 조회");
    assert_eq!(count, 1);
}

/// query API는 쓰기 SQL의 RETURNING 행을 반환한다.
#[test]
fn query_accepts_insert_returning() {
    let (_dir, db) = open_db();
    let row: (i64, String) = db
        .run_sync()
        .query_one(
            "INSERT INTO todos (title, done) VALUES (?1, 0) RETURNING id, title",
            params!["returning"],
        )
        .expect("INSERT RETURNING");
    assert!(row.0 > 0);
    assert_eq!(row.1, "returning");
}

/// SQL 주석과 CTE가 앞에 있어도 라우팅 판별 없이 실행한다.
#[test]
fn query_accepts_complex_write_returning_without_routing() {
    let (_dir, db) = open_db();
    let title: String = db
        .run_sync()
        .query_scalar(
            "-- 첫 키워드로 판별할 수 없음\nWITH input(title) AS (VALUES (?1)) INSERT INTO todos(title, done) SELECT title, 0 FROM input RETURNING title",
            params!["complex"],
        )
        .expect("복잡 SQL RETURNING");
    assert_eq!(title, "complex");
}

/// 트랜잭션 — 클로저 커밋/롤백 + RAII drop 롤백 (명세 §5.5)
#[test]
fn transactions() {
    let (_dir, db) = open_db();
    let h = db.run_sync();

    // 커밋 — tx-바운드 DAO 사용 (명세 §5.9 한 메커니즘)
    use DbTxDaos as _;
    h.transaction(|tx| {
        tx.todo_dao().add(&Todo {
            id: 0,
            title: "tx1".into(),
            done: false,
        })?;
        tx.todo_dao().add(&Todo {
            id: 0,
            title: "tx2".into(),
            done: false,
        })?;
        Ok(())
    })
    .unwrap();
    let cnt: i64 = h
        .query_scalar("SELECT COUNT(*) FROM todos", params![])
        .unwrap();
    assert_eq!(cnt, 2);

    // 에러 = 롤백
    let r: roomrs::Result<()> = h.transaction(|tx| {
        tx.todo_dao().add(&Todo {
            id: 0,
            title: "롤백될 행".into(),
            done: false,
        })?;
        Err(roomrs::Error::Config("의도적 실패".into()))
    });
    assert!(r.is_err());
    let cnt: i64 = h
        .query_scalar("SELECT COUNT(*) FROM todos", params![])
        .unwrap();
    assert_eq!(cnt, 2, "롤백으로 개수 불변");

    // RAII — 미커밋 drop = 롤백
    {
        let tx = h.begin().unwrap();
        tx.execute(
            "INSERT INTO todos (title, done) VALUES ('drop', 0)",
            params![],
        )
        .unwrap();
        // commit 없이 drop
    }
    let cnt: i64 = h
        .query_scalar("SELECT COUNT(*) FROM todos", params![])
        .unwrap();
    assert_eq!(cnt, 2, "RAII drop = 롤백");

    // RAII — 커밋
    {
        let tx = h.begin().unwrap();
        tx.execute(
            "INSERT INTO todos (title, done) VALUES ('commit', 0)",
            params![],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let cnt: i64 = h
        .query_scalar("SELECT COUNT(*) FROM todos", params![])
        .unwrap();
    assert_eq!(cnt, 3);
}

/// upsert — INSERT OR REPLACE (keep_pk 아님 주의: autoincrement PK 생략이라 항상 신규)
#[test]
fn upsert_replace() {
    let (_dir, db) = open_db();
    let h = db.run_sync();
    let dao = h.todo_dao();
    let id = dao
        .upsert(&Todo {
            id: 0,
            title: "u1".into(),
            done: false,
        })
        .unwrap();
    assert!(id > 0);
}

/// 버전 불일치 = 에러 (M1 잠정 마이그레이션)
#[test]
fn version_mismatch_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.db");
    drop(Db::builder().sqlite(&path).build().unwrap());

    // 같은 파일을 다른 버전 스키마로 열기
    #[database(entities(Todo), daos(TodoDao), version = 2)]
    struct Db2;
    match Db2::builder().sqlite(&path).build() {
        Err(roomrs::Error::Migration(_)) => {}
        Err(other) => panic!("Migration 에러를 기대했으나: {other}"),
        Ok(_) => panic!("버전 불일치가 통과되면 안 된다"),
    }
}
