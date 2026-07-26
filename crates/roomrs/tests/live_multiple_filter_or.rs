//! LiveQuery 다중 InvalidationFilter OR 매칭 (결정 52)

use roomrs::{InvalidationFilter, LiveQuery, dao, database, entity, params};
use std::time::{Duration, Instant};

#[entity(table = "items")]
#[derive(Debug, Clone)]
struct Item {
    #[pk(autoincrement)]
    id: i64,
    kind: String,
    status: String,
}

#[entity(table = "notes")]
#[derive(Debug, Clone)]
struct Note {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[dao]
trait ItemDao {
    #[query("SELECT * FROM items WHERE kind = :kind OR status = :status")]
    fn watch_or(&self, kind: String, status: String, f_kind: InvalidationFilter, f_status: InvalidationFilter) -> LiveQuery<Vec<Item>>;
}

#[database(entities(Item, Note), daos(ItemDao), version = 1)]
struct AppDb;

/// 테스트 DB 오픈
fn open() -> AppDb {
    AppDb::builder().in_memory().build().expect("open")
}

/// filter 빌드 헬퍼
fn eq_filter(table: &str, col: &str, val: &str) -> InvalidationFilter {
    InvalidationFilter::table(table).where_group(|g| g.eq(col, val)).build().expect("filter")
}

/// 다음 값 수신 (timeout)
fn next_count(q: &LiveQuery<i64>) -> i64 {
    q.recv_timeout(Duration::from_secs(3)).expect("recv").expect("value")
}

/// 단일 filter 기존 호출 호환
#[test]
fn single_filter_still_works() {
    let db = open();
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], eq_filter("items", "kind", "a")).debounce(Duration::ZERO);
    assert_eq!(next_count(&q), 0);

    h.execute("INSERT INTO items (kind, status) VALUES ('a', 'open')", params![]).unwrap();
    assert_eq!(next_count(&q), 1);

    h.execute("INSERT INTO items (kind, status) VALUES ('b', 'open')", params![]).unwrap();
    // kind=b 는 filter 미매칭 — 재조회 없음
    assert!(q.recv_timeout(Duration::from_millis(150)).unwrap().is_none());
}

/// 서로 다른 table 포함 복수 filter 중 하나 매칭 = 재조회
#[test]
fn multi_filter_or_matches_either() {
    let db = open();
    let h = db.run_sync();
    let filters = vec![eq_filter("items", "kind", "a"), eq_filter("notes", "body", "x")];
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT (SELECT COUNT(*) FROM items WHERE kind = 'a') + (SELECT COUNT(*) FROM notes WHERE body = 'x')", params![], filters).debounce(Duration::ZERO);
    assert_eq!(next_count(&q), 0);

    // notes 쪽 filter 만 매칭
    h.execute("INSERT INTO notes (body) VALUES ('x')", params![]).unwrap();
    assert_eq!(next_count(&q), 1);

    // items 쪽 filter 만 매칭
    h.execute("INSERT INTO items (kind, status) VALUES ('a', 'open')", params![]).unwrap();
    assert_eq!(next_count(&q), 2);

    // 어느 filter 도 매칭 안 함
    h.execute("INSERT INTO items (kind, status) VALUES ('z', 'closed')", params![]).unwrap();
    h.execute("INSERT INTO notes (body) VALUES ('other')", params![]).unwrap();
    assert!(q.recv_timeout(Duration::from_millis(150)).unwrap().is_none());
}

/// 어떤 filter 도 매칭하지 않으면 재조회 안 함
#[test]
fn multi_filter_no_match_skips_refresh() {
    let db = open();
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a' OR status = 'vip'", params![], [eq_filter("items", "kind", "a"), eq_filter("items", "status", "vip")]).debounce(Duration::ZERO);
    assert_eq!(next_count(&q), 0);

    h.execute("INSERT INTO items (kind, status) VALUES ('b', 'open')", params![]).unwrap();
    assert!(q.recv_timeout(Duration::from_millis(150)).unwrap().is_none());
}

/// DAO 복수 InvalidationFilter 인자 = OR
#[test]
fn dao_multi_filter_or() {
    let db = open();
    let h = db.run_sync();
    let f_kind = eq_filter("items", "kind", "a");
    let f_status = eq_filter("items", "status", "vip");
    let q = h.item_dao().watch_or("a".into(), "vip".into(), f_kind, f_status).debounce(Duration::ZERO);
    let first = q.recv_timeout(Duration::from_secs(3)).expect("recv").expect("value");
    assert!(first.is_empty());

    h.execute("INSERT INTO items (kind, status) VALUES ('a', 'open')", params![]).unwrap();
    let rows = q.recv_timeout(Duration::from_secs(2)).expect("recv").expect("value");
    assert_eq!(rows.len(), 1);

    h.execute("INSERT INTO items (kind, status) VALUES ('b', 'vip')", params![]).unwrap();
    let rows = q.recv_timeout(Duration::from_secs(2)).expect("recv").expect("value");
    assert_eq!(rows.len(), 2);

    // 무관 행
    h.execute("INSERT INTO items (kind, status) VALUES ('c', 'open')", params![]).unwrap();
    assert!(q.recv_timeout(Duration::from_millis(150)).unwrap().is_none());
}

/// async filtered 복수 filter 대칭
#[test]
fn async_multi_filter_or() {
    let db = open();
    let ha = db.run_async();
    let q: LiveQuery<i64> = ha.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a' OR kind = 'b'", params![], vec![eq_filter("items", "kind", "a"), eq_filter("items", "kind", "b")]).debounce(Duration::ZERO);
    assert_eq!(next_count(&q), 0);

    db.run_sync().execute("INSERT INTO items (kind, status) VALUES ('b', 'open')", params![]).unwrap();
    assert_eq!(next_count(&q), 1);
}

/// 타임아웃 헬퍼 — CI 느린 환경 여유
#[allow(dead_code)]
fn wait_until(deadline: Instant) -> bool {
    Instant::now() < deadline
}
