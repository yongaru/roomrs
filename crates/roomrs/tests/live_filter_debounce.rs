// LiveQuery filter API 대칭 · observer debounce (250ms coalesce)
#![cfg(feature = "live")]

use roomrs::{DEFAULT_DEBOUNCE, InvalidationFilter, LiveQuery, dao, database, entity, params};
use std::time::{Duration, Instant};

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
    #[insert]
    fn add(&self, item: &Item) -> roomrs::Result<i64>;

    #[query("SELECT * FROM items WHERE kind = :kind ORDER BY id")]
    fn watch_by_kind(&self, kind: String, filter: InvalidationFilter) -> LiveQuery<Vec<Item>>;

    #[query("SELECT COUNT(*) FROM items WHERE kind = :kind")]
    fn watch_count(&self, kind: String, filter: InvalidationFilter) -> LiveQuery<i64>;
}

#[database(entities(Item), daos(ItemDao), version = 1)]
struct Db;

/// emit 대기 — 기본 debounce(250ms)보다 여유 있게
fn next<T: Clone + Send + 'static>(query: &LiveQuery<T>) -> T {
    query.recv_timeout(Duration::from_secs(3)).expect("수신 에러").expect("emit 타임아웃")
}

fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder().sqlite(dir.path().join("live-filter.db")).build().unwrap();
    (dir, db)
}

fn kind_filter(kind: &str) -> InvalidationFilter {
    InvalidationFilter::table("items").where_group(|g| g.eq("kind", kind)).build().unwrap()
}

/// DB 전역 debounce 미설정 시 기본 250ms 상속 (`.debounce` 호출 없음)
#[test]
fn sync_filtered_debounces_default_250ms() {
    let (_dir, db) = open();
    let h = db.run_sync();
    // observer override 없이 DB 전역 기본값(DEFAULT_DEBOUNCE) 사용
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], kind_filter("a"));
    assert_eq!(next(&q), 0);

    let t0 = Instant::now();
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 1)", params![]).unwrap();
    assert!(q.recv_timeout(Duration::from_millis(80)).unwrap().is_none(), "debounce 전 emit 금지");
    assert_eq!(next(&q), 1);
    assert!(t0.elapsed() >= Duration::from_millis(200), "대략 기본 debounce 이후 emit");
    assert_eq!(DEFAULT_DEBOUNCE, Duration::from_millis(250));
}

/// Builder `live_debounce` 전역값이 새 LiveQuery 에 상속된다
#[test]
fn live_debounce_builder_default_is_inherited() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder().sqlite(dir.path().join("live-db-debounce.db")).live_debounce(Duration::from_millis(400)).build().unwrap();
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], kind_filter("a"));
    assert_eq!(next(&q), 0);

    let t0 = Instant::now();
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 1)", params![]).unwrap();
    assert!(q.recv_timeout(Duration::from_millis(100)).unwrap().is_none(), "전역 400ms 전 emit 금지");
    assert_eq!(next(&q), 1);
    let elapsed = t0.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "전역 debounce 미달: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(700), "전역 debounce 과다: {elapsed:?}");
}

/// observer `.debounce` 가 DB 전역값을 override 한다
#[test]
fn observer_debounce_overrides_db_live_debounce() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder().sqlite(dir.path().join("live-override.db")).live_debounce(Duration::from_millis(500)).build().unwrap();
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], kind_filter("a")).debounce(Duration::from_millis(100));
    assert_eq!(next(&q), 0);

    let t0 = Instant::now();
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 1)", params![]).unwrap();
    assert_eq!(next(&q), 1);
    let elapsed = t0.elapsed();
    assert!(elapsed < Duration::from_millis(350), "observer override 가 전역 500ms 를 이김: {elapsed:?}");
}

/// observer별 debounce(ZERO) = 즉시 재조회
#[test]
fn observer_debounce_zero_refreshes_promptly() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], kind_filter("a")).debounce(Duration::ZERO);
    assert_eq!(next(&q), 0);
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 1)", params![]).unwrap();
    assert_eq!(next(&q), 1);
}

/// DAO filtered LiveQuery — InvalidationFilter 인자는 SQL bind 제외
#[test]
fn dao_filtered_watch_ignores_unrelated_rows() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let dao = h.item_dao();
    let q = dao.watch_count("a".into(), kind_filter("a")).debounce(Duration::ZERO);
    assert_eq!(next(&q), 0);

    dao.add(&Item { id: 0, kind: "b".into(), value: 1 }).unwrap();
    assert!(q.recv_timeout(Duration::from_millis(200)).unwrap().is_none(), "다른 kind = emit 없음");

    dao.add(&Item { id: 0, kind: "a".into(), value: 2 }).unwrap();
    assert_eq!(next(&q), 1);
    let rows = next(&dao.watch_by_kind("a".into(), kind_filter("a")).debounce(Duration::ZERO));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "a");
}

/// async filtered API 대칭
#[test]
fn async_filtered_watch_works() {
    let (_dir, db) = open();
    let ha = db.run_async();
    let q: LiveQuery<i64> = ha.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], kind_filter("a")).debounce(Duration::ZERO);
    assert_eq!(next(&q), 0);
    db.run_sync().execute("INSERT INTO items(kind, value) VALUES ('a', 9)", params![]).unwrap();
    assert_eq!(next(&q), 1);
}

/// 고정 coalesce: 첫 무효화부터 창을 열고, 창 안 추가 무효화는 만료를 연장하지 않는다
#[test]
fn debounce_window_is_fixed_not_sliding() {
    let (_dir, db) = open();
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar_filtered("SELECT COUNT(*) FROM items WHERE kind = 'a'", params![], kind_filter("a")).debounce(Duration::from_millis(1000));
    assert_eq!(next(&q), 0);

    let t0 = Instant::now();
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 1)", params![]).unwrap();
    // 창 안 추가 무효화 — 만료 시각 연장 없이 병합
    std::thread::sleep(Duration::from_millis(300));
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 2)", params![]).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    h.execute("INSERT INTO items(kind, value) VALUES ('a', 3)", params![]).unwrap();

    assert_eq!(next(&q), 3);
    let elapsed = t0.elapsed();
    // 고정 창 ≈1초. sliding 이면 마지막 insert부터 1초를 다시 기다려 ≈1.6초 이상이다.
    assert!(elapsed >= Duration::from_millis(850), "너무 빠름: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(1400), "sliding 연장 의심: {elapsed:?}");
}
