//! InvalidationFilter 스키마 검증 (명세 §9.5 P1)
#![cfg(feature = "live")]

use roomrs::{ErrorAdvice, ErrorPath, InvalidationFilter, LiveQuery, dao, database, entity, params};
use std::time::Duration;

#[entity(table = "items")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    #[pk(autoincrement)]
    id: i64,
    kind: String,
    value: i64,
}

#[dao]
trait ItemDao {
    #[query("SELECT COUNT(*) FROM items WHERE kind = :kind")]
    fn watch_count(&self, kind: String, filter: InvalidationFilter) -> LiveQuery<i64>;
}

#[database(entities(Item), daos(ItemDao), version = 1)]
struct Db;

/// 임시 DB 오픈
fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::builder().sqlite(dir.path().join("filter-schema.db")).build().expect("build");
    (dir, db)
}

/// 유효 filter 는 구독·재조회 성공
#[test]
fn valid_filter_registers_and_emits() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let filter = InvalidationFilter::table("items").where_group(|g| g.eq("kind", "a")).build().expect("filter");
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], filter).debounce(Duration::ZERO);
    let v = q.recv_timeout(Duration::from_secs(2)).expect("recv").expect("value");
    assert_eq!(v, 0);
}

/// 없는 테이블 = InvalidationFilter 구조화 에러
#[test]
fn missing_table_returns_structured_error() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let filter = InvalidationFilter::table("no_such_table").where_group(|g| g.eq("kind", "a")).build().expect("filter");
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT 1", params![], filter);
    let err = q.recv_timeout(Duration::from_secs(2)).expect_err("스키마 검증 실패여야 함");
    assert_eq!(err.path(), ErrorPath::LiveQuery);
    assert_eq!(err.advice(), ErrorAdvice::CheckConfiguration);
    assert!(err.to_string().contains("no_such_table"), "{err}");
}

/// 없는 컬럼 = InvalidationFilter 구조화 에러 (watch_all_filtered 경로)
#[test]
fn missing_column_returns_structured_error() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let filter = InvalidationFilter::table("items").where_group(|g| g.eq("no_such_col", 1)).build().expect("filter");
    let q: LiveQuery<Vec<Item>> = h.watch_all_filtered("SELECT * FROM items", params![], filter);
    let err = q.recv_timeout(Duration::from_secs(2)).expect_err("컬럼 검증 실패여야 함");
    assert_eq!(err.path(), ErrorPath::LiveQuery);
    assert_eq!(err.advice(), ErrorAdvice::CheckConfiguration);
    assert!(err.to_string().contains("no_such_col"), "{err}");
}

/// DAO filtered watch 동일 검증
#[test]
fn dao_filtered_watch_validates_schema() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let dao = h.item_dao();
    let bad = InvalidationFilter::table("items").where_group(|g| g.eq("typo_col", "x")).build().expect("filter");
    let q = dao.watch_count("a".into(), bad);
    let err = q.recv_timeout(Duration::from_secs(2)).expect_err("DAO 경로 검증 실패여야 함");
    assert_eq!(err.path(), ErrorPath::LiveQuery);
    assert!(err.to_string().contains("typo_col"), "{err}");

    let good = InvalidationFilter::table("items").where_group(|g| g.eq("kind", "a")).build().expect("filter");
    let q = dao.watch_count("a".into(), good).debounce(Duration::ZERO);
    let v = q.recv_timeout(Duration::from_secs(2)).expect("recv").expect("value");
    assert_eq!(v, 0);
}
