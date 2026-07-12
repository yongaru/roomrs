// M4b 검증 통합 테스트 (명세 §15 M4b) —
// 인스턴스 A write → B emit · 로컬 이중 통지 없음 · 미옵트인 미알림 · Validate 트리거 소실 · 버전 태깅
#![cfg(feature = "multi-instance")]

use roomrs::{LiveQuery, MigrationPolicy, dao, database, entity, params};
use std::time::Duration;

#[entity(table = "orders", multi_instance)]
#[derive(Debug, Clone)]
struct Order {
    #[pk(autoincrement)]
    id: i64,
    item: String,
}

#[entity(table = "drafts")] // 미옵트인
#[derive(Debug, Clone)]
struct Draft {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[dao]
trait OrderDao {
    #[insert]
    fn add(&self, o: &Order) -> roomrs::Result<i64>;

    #[query("SELECT COUNT(*) FROM orders")]
    fn watch_count(&self) -> LiveQuery<i64>;
}

#[database(entities(Order, Draft), daos(OrderDao), version = 1)]
struct Db;

/// MI 활성 DB 오픈 (짧은 폴링 주기)
fn open_mi(path: &std::path::Path) -> Db {
    Db::builder()
        .sqlite(path)
        .enable_multi_instance_invalidation(true)
        .mi_poll_interval(Duration::from_millis(50))
        .build()
        .unwrap()
}

/// emit 대기 헬퍼
fn next<T: Clone + Send + 'static>(q: &LiveQuery<T>) -> T {
    q.recv_timeout(Duration::from_secs(3))
        .expect("수신 에러")
        .expect("emit 타임아웃")
}

/// 인스턴스 A write → 인스턴스 B emit (옵트인 테이블, 폴링 경로)
#[test]
fn cross_instance_write_emits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mi.db");
    let a = open_mi(&path);
    let b = open_mi(&path);

    let hb = b.run_sync();
    let live = hb.order_dao().watch_count();
    assert_eq!(next(&live), 0, "초기 emit");

    // A에서 write — B는 폴러 경유로만 알 수 있다
    a.run_sync()
        .order_dao()
        .add(&Order {
            id: 0,
            item: "교차".into(),
        })
        .unwrap();
    assert_eq!(next(&live), 1, "교차 인스턴스 emit");
}

/// 로컬 write 이중 통지 없음 — 문장 기반 emit 1회 + 폴러 재방출 0 [B-2]
#[test]
fn local_write_no_double_emit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dd.db");
    let a = open_mi(&path);
    let h = a.run_sync();

    let live = h.order_dao().watch_count();
    assert_eq!(next(&live), 0);

    h.order_dao()
        .add(&Order {
            id: 0,
            item: "로컬".into(),
        })
        .unwrap();
    assert_eq!(next(&live), 1, "문장 기반 emit 1회");

    // 폴링 주기 여러 번 지나도 재방출 없음
    assert!(
        live.recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_none(),
        "폴러 이중 통지 없음"
    );
}

/// 미옵트인 테이블 = 교차 알림 없음 (인-프로세스는 M4 경로로 동작)
#[test]
fn non_opted_table_no_cross_notification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no.db");
    let a = open_mi(&path);
    let b = open_mi(&path);

    // 미옵트인 drafts 구독 (B) — 경고 1회 출력됨(수동 확인)
    let live: LiveQuery<i64> = b
        .run_sync()
        .watch_scalar("SELECT COUNT(*) FROM drafts", &[]);
    assert_eq!(next(&live), 0);

    // A에서 drafts write — 트리거 없음 = B에 교차 알림 없음
    a.run_sync()
        .execute("INSERT INTO drafts (body) VALUES ('x')", params![])
        .unwrap();
    assert!(
        live.recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_none(),
        "미옵트인 = 교차 알림 없음"
    );
}

/// 로컬 write가 원격 무효화를 삼키지 않는다 (H-2 —
/// 구 MAX(seq) 선점 방식은 소비 전 원격 로그 행을 영구 건너뜀)
#[test]
fn local_write_does_not_swallow_remote_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sw.db");
    let a = open_mi(&path);
    let b = open_mi(&path);

    let hb = b.run_sync();
    let live = hb.order_dao().watch_count();
    assert_eq!(next(&live), 0);

    // A가 orders에 write(원격) → 직후 B가 다른 테이블에 로컬 write.
    // 구버전은 B의 로컬 write가 last_seen을 MAX(seq)로 선점해 A의 행을 삼켰다.
    a.run_sync()
        .order_dao()
        .add(&Order {
            id: 0,
            item: "원격".into(),
        })
        .unwrap();
    hb.execute("INSERT INTO drafts (body) VALUES ('로컬')", params![])
        .unwrap();

    // B의 폴러가 A의 write를 무효화로 방출해야 한다
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut got = false;
    while std::time::Instant::now() < deadline {
        match live.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(1)) => {
                got = true;
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("수신 에러: {e}"),
        }
    }
    assert!(got, "원격 write 무효화 수신 (로컬 write에 삼켜지지 않음)");
}

/// drop = 폴러/노티파이어 join — 긴 폴링 주기에도 즉시 종료 (M-5)
#[test]
fn drop_joins_threads_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("j.db"))
        .enable_multi_instance_invalidation(true)
        .mi_poll_interval(Duration::from_secs(30))
        .build()
        .unwrap();
    // 폴러가 긴 대기에 들어간 뒤 drop — Condvar 신호로 즉시 깨어나야 한다
    std::thread::sleep(Duration::from_millis(100));
    let start = std::time::Instant::now();
    drop(db);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "폴링 주기(30초)를 기다리지 않고 즉시 join"
    );
}

/// Validate 정책 — 트리거 소실 감지 (명세 §9.5)
#[test]
fn validate_detects_missing_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.db");
    // 정상 설치
    drop(open_mi(&path));

    // 외부 도구 시뮬레이션 — 트리거 하나 삭제
    let conn = roomrs::rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER \"__roomrs_inv_v1_orders_i\"")
        .unwrap();
    drop(conn);

    // Validate = 에러
    let r = Db::builder()
        .sqlite(&path)
        .enable_multi_instance_invalidation(true)
        .migrate(MigrationPolicy::Validate)
        .build();
    assert!(r.is_err(), "트리거 소실 = Validate 에러");

    // Auto = 재설치 성공
    let db = open_mi(&path);
    let n: i64 = db
        .run_sync()
        .query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE '__roomrs_inv_v1_orders_%'",
            params![],
        )
        .unwrap();
    assert_eq!(n, 3, "Auto = 트리거 재설치");
}

/// 트리거 버전 태깅 확인 (명세 §9.5)
#[test]
fn trigger_names_are_version_tagged() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_mi(&dir.path().join("t.db"));
    let names: Vec<String> = db
        .run_sync()
        .query_all(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE '__roomrs_inv_%' ORDER BY name",
            params![],
        )
        .unwrap()
        .into_iter()
        .map(|(n,): (String,)| n)
        .collect();
    assert_eq!(
        names,
        vec![
            "__roomrs_inv_v1_orders_d",
            "__roomrs_inv_v1_orders_i",
            "__roomrs_inv_v1_orders_u"
        ],
        "버전 태깅 트리거 이름"
    );
}
