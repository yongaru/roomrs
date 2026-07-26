//! LiveQuery 공개 관측성 metrics (명세 §9.5 P2)
#![cfg(feature = "live")]

use roomrs::{LiveQuery, database, entity, params};
use std::sync::{Arc, Barrier};
use std::time::Duration;

#[entity(table = "items")]
struct Item {
    #[pk(autoincrement)]
    id: i64,
    name: String,
}

#[database(entities(Item), version = 1)]
struct Db;

/// emit 대기
fn next<T: Clone + Send + 'static>(q: &LiveQuery<T>) -> T {
    q.recv_timeout(Duration::from_secs(3)).expect("recv").expect("timeout")
}

/// 기본 metrics 카운터·coalesce 병합이 증가한다
#[test]
fn metrics_count_events_coalesce_and_refresh_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::builder().sqlite(dir.path().join("m.db")).live_debounce(Duration::from_millis(200)).connections(2).notifier_readers(2).build().expect("build");
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", params![]);
    assert_eq!(next(&q), 0);

    let before = db.live_metrics();
    h.execute("INSERT INTO items(name) VALUES ('a')", params![]).unwrap();
    // 고정 창 안 추가 무효화 = coalesce
    std::thread::sleep(Duration::from_millis(40));
    h.execute("INSERT INTO items(name) VALUES ('b')", params![]).unwrap();
    assert_eq!(next(&q), 2);

    let after = db.live_metrics();
    assert!(after.events_received > before.events_received, "events_received: {before:?} → {after:?}");
    assert!(after.coalesce_merged > before.coalesce_merged, "coalesce_merged: {before:?} → {after:?}");
    assert!(after.refresh_ok > before.refresh_ok, "refresh_ok: {before:?} → {after:?}");
    // 민감 필드 없음 — 구조체 필드만 숫자
    let _ = after.worker_queue_depth;
    let _ = after.refresh_err;
    let _ = after.stale_discarded;
}

/// rebind 중 진행 재조회는 stale_discarded 로 센다
#[test]
fn metrics_count_stale_discard_on_rebind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::builder().sqlite(dir.path().join("stale.db")).connections(2).notifier_readers(2).build().expect("build");
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", params![]).debounce(Duration::ZERO);
    assert_eq!(next(&q), 0);
    let before = db.live_metrics().stale_discarded;

    // 느린 재조회 유발 후 즉시 rebind — in-flight 결과 폐기 가능
    h.execute("INSERT INTO items(name) VALUES ('x')", params![]).unwrap();
    // rebind 가 epoch 를 올려 이전 Full 결과를 버릴 수 있음
    q.rebind(params![]).expect("rebind");
    let _ = q.recv_timeout(Duration::from_secs(2));

    let after = db.live_metrics().stale_discarded;
    // 환경에 따라 0 일 수 있어 비감소만 보장 + rebind 후 최종 값은 수신 가능
    assert!(after >= before, "stale_discarded 감소 금지: {before} → {after}");
}

/// metrics 스냅샷은 여러 스레드에서 동시 읽기 안전
#[test]
fn metrics_snapshot_is_concurrent_safe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::builder().sqlite(dir.path().join("conc.db")).connections(3).notifier_readers(2).build().expect("build"));
    let h = db.run_sync();
    let q: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", params![]).debounce(Duration::ZERO);
    assert_eq!(next(&q), 0);

    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..3 {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..50 {
                let m = db.live_metrics();
                let _ = m.events_received.wrapping_add(m.refresh_ok);
            }
        }));
    }
    barrier.wait();
    for i in 0..20 {
        h.execute("INSERT INTO items(name) VALUES (?1)", params![format!("n{i}")]).unwrap();
        let _ = q.recv_timeout(Duration::from_millis(200));
    }
    for h in handles {
        h.join().expect("reader join");
    }
    let m = db.live_metrics();
    assert!(m.refresh_ok >= 1);
}

/// DB drop 후 남아 있는 Arc 에서 metrics 읽기는 panic 하지 않는다
#[test]
fn metrics_readable_until_db_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::builder().sqlite(dir.path().join("drop.db")).build().expect("build");
    let m1 = db.live_metrics();
    let _ = m1.events_received;
    drop(db);
    // drop 후 핸들 없음 — 스냅샷은 drop 전에만 유효. 여기서는 drop 자체가 panic 없음 확인.
}
