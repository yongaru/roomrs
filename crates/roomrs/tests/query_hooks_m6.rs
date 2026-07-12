// M6/M6b 검증 통합 테스트 (명세 §15) —
// 쿼리빌더 핸들 대칭 실행 · 스키마 검증 · 운영 훅(on_create/on_open/query_logger)

use roomrs::{Order, Query, col, dao, database, entity};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AO};

#[entity(table = "books")]
#[derive(Debug, Clone, PartialEq)]
struct Book {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    year: i64,
}

#[dao]
trait BookDao {
    #[insert]
    fn add(&self, b: &Book) -> roomrs::Result<i64>;
}

#[database(entities(Book), daos(BookDao), version = 1)]
struct Db;

fn seed() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("q.db"))
        .build()
        .unwrap();
    let h = db.run_sync();
    for (t, y) in [("가", 2001), ("나", 2005), ("다", 2010)] {
        h.book_dao()
            .add(&Book {
                id: 0,
                title: t.into(),
                year: y,
            })
            .unwrap();
    }
    (dir, db)
}

/// 빌더 실행 — 조건·정렬·페이지 (동기 핸들)
#[test]
fn builder_sync_execution() {
    let (_d, db) = seed();

    let rows: Vec<Book> = Query::select::<Book>()
        .and_where(col("year").ge(2005i64))
        .order_by("year", Order::Desc)
        .fetch_all(db.run_sync())
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].year, 2010);

    let one: Book = Query::select::<Book>()
        .and_where(col("title").eq("나"))
        .fetch_one(db.run_sync())
        .unwrap();
    assert_eq!(one.year, 2005);

    let none: Option<Book> = Query::select::<Book>()
        .and_where(col("year").gt(9999i64))
        .fetch_optional(db.run_sync())
        .unwrap();
    assert!(none.is_none());

    let cnt: i64 = Query::count::<Book>()
        .and_where(col("year").in_list([2001i64, 2010]))
        .fetch_scalar(db.run_sync())
        .unwrap();
    assert_eq!(cnt, 2);
}

/// 핸들 대칭 [C-6] — 같은 쿼리를 clone해 동기/비동기 동일 메서드명 실행
#[cfg(all(feature = "async", not(feature = "tokio")))]
#[test]
fn builder_handle_symmetry() {
    let (_d, db) = seed();
    let q = Query::select::<Book>()
        .and_where(col("year").ge(2005i64))
        .order_by("id", Order::Asc);

    let sync_rows: Vec<Book> = q.clone().fetch_all(db.run_sync()).unwrap();
    let async_rows: Vec<Book> =
        smol::block_on(async { q.fetch_all(db.run_async()).await }).unwrap();
    assert_eq!(sync_rows, async_rows, "동기/비동기 동일 결과");
}

/// 스키마 인지 — 미지 컬럼 = Config 에러 (SQLite 도달 전)
#[test]
fn builder_schema_validation() {
    let (_d, db) = seed();
    let r: roomrs::Result<Vec<Book>> = Query::select::<Book>()
        .and_where(col("ghost").eq(1i64))
        .fetch_all(db.run_sync());
    assert!(matches!(r, Err(roomrs::Error::Config(_))));
}

/// LIKE ESCAPE는 `%`, `_`, escape 문자 자체를 리터럴로 검색한다.
#[test]
fn escaped_like_matches_literal_wildcards() {
    let (_d, db) = seed();
    let h = db.run_sync();
    for title in ["100%", "under_score", "bang!mark", "한%"] {
        h.book_dao()
            .add(&Book {
                id: 0,
                title: title.into(),
                year: 2020,
            })
            .unwrap();
    }

    for (pattern, escape, expected) in [
        ("100!%", '!', "100%"),
        ("under!_score", '!', "under_score"),
        ("bang!!mark", '!', "bang!mark"),
        ("한界%", '界', "한%"),
    ] {
        let rows: Vec<Book> = Query::select::<Book>()
            .and_where(col("title").like_escaped(pattern, escape))
            .fetch_all(*h)
            .unwrap();
        assert_eq!(rows.len(), 1, "pattern={pattern}");
        assert_eq!(rows[0].title, expected);
    }
}

/// M6b — on_create 1회성 · on_open 매회 · query_logger 출력 (명세 §15 M6b)
#[test]
fn operational_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("h.db");
    let creates = Arc::new(AtomicU64::new(0));
    let opens = Arc::new(AtomicU64::new(0));
    let logs = Arc::new(AtomicU64::new(0));

    let open = |c: Arc<AtomicU64>, o: Arc<AtomicU64>, l: Arc<AtomicU64>| {
        Db::builder()
            .sqlite(&path)
            .on_create(move |_| {
                c.fetch_add(1, AO::SeqCst);
                Ok(())
            })
            .on_open(move |_| {
                o.fetch_add(1, AO::SeqCst);
                Ok(())
            })
            .query_logger(move |_sql, _d| {
                l.fetch_add(1, AO::SeqCst);
            })
            .build()
            .unwrap()
    };

    // 1차 오픈 — 신규 생성
    let db = open(creates.clone(), opens.clone(), logs.clone());
    assert_eq!(creates.load(AO::SeqCst), 1, "on_create 1회");
    let connections_per_open = if cfg!(feature = "live") { 6 } else { 5 };
    assert_eq!(
        opens.load(AO::SeqCst),
        connections_per_open,
        "on_open은 모든 내부 연결마다 호출"
    );

    let before = logs.load(AO::SeqCst);
    db.run_sync()
        .book_dao()
        .add(&Book {
            id: 0,
            title: "로그".into(),
            year: 1,
        })
        .unwrap();
    assert!(logs.load(AO::SeqCst) > before, "query_logger 호출");
    drop(db);

    // 2차 오픈 — on_create 미발화, on_open 재발화
    let _db2 = open(creates.clone(), opens.clone(), logs.clone());
    assert_eq!(creates.load(AO::SeqCst), 1, "on_create는 최초 1회만");
    assert_eq!(
        opens.load(AO::SeqCst),
        connections_per_open * 2,
        "on_open은 매 오픈의 모든 내부 연결마다 호출"
    );
}
