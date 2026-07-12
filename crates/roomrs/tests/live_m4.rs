// M4 검증 통합 테스트 (명세 §15 M4) —
// 즉시 emit · write 재조회 · truncate DELETE · 트리거 간접 write · 롤백 미발동 ·
// drop 후 emit 0 · rebind 스테일 폐기 · subscribe/into_stream
#![cfg(feature = "live")]

use roomrs::{LiveQuery, dao, database, entity, params};
use std::time::Duration;

#[entity(table = "items")]
#[derive(Debug, Clone, PartialEq)]
struct Item {
    #[pk(autoincrement)]
    id: i64,
    name: String,
    done: bool,
}

#[entity(table = "audit")]
#[derive(Debug, Clone)]
struct Audit {
    #[pk(autoincrement)]
    id: i64,
    note: String,
}

#[dao]
trait ItemDao {
    #[insert]
    fn add(&self, i: &Item) -> roomrs::Result<i64>;

    #[query("SELECT * FROM items WHERE done = :done ORDER BY id")]
    fn watch_by_done(&self, done: bool) -> LiveQuery<Vec<Item>>;

    #[query("SELECT COUNT(*) FROM items")]
    fn watch_count(&self) -> LiveQuery<i64>;
}

#[database(entities(Item, Audit), daos(ItemDao), version = 1)]
struct Db;

/// emit 대기 헬퍼 — 최대 2초
fn next<T: Clone + Send + 'static>(q: &LiveQuery<T>) -> T {
    q.recv_timeout(Duration::from_secs(2))
        .expect("수신 에러")
        .expect("emit 타임아웃")
}

/// 기대값 수렴 대기 — 과잉 emit(§9.4 최종 일관성)을 흡수하며 기대값 도달 확인
fn wait_for<T: Clone + Send + 'static + PartialEq + std::fmt::Debug>(
    q: &LiveQuery<T>,
    expected: T,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut last: Option<T> = None;
    while std::time::Instant::now() < deadline {
        match q.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(v)) => {
                if v == expected {
                    return;
                }
                last = Some(v);
            }
            Ok(None) => {}
            Err(e) => panic!("수신 에러: {e}"),
        }
    }
    panic!("기대값 {expected:?} 미도달 (마지막: {last:?})");
}

fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("l.db"))
        .build()
        .unwrap();
    (dir, db)
}

/// 구독 즉시 1회 emit + write 후 재조회 emit (명세 §9.1)
#[test]
fn initial_and_write_emit() {
    let (_d, db) = open();
    let h = db.run_sync();
    let dao = h.item_dao();

    let live = dao.watch_by_done(false);
    assert_eq!(next(&live).len(), 0, "구독 즉시 1회 emit (빈 결과)");

    dao.add(&Item {
        id: 0,
        name: "a".into(),
        done: false,
    })
    .unwrap();
    assert_eq!(next(&live).len(), 1, "write 후 재조회 emit");

    // 무관 테이블 write는 emit 없음
    h.execute("INSERT INTO audit (note) VALUES ('x')", params![])
        .unwrap();
    assert!(
        live.recv_timeout(Duration::from_millis(300))
            .unwrap()
            .is_none(),
        "무관 테이블 = emit 없음"
    );
}

/// notifier 전용 커넥션은 write LiveQuery 실행을 거부한다.
#[test]
fn notifier_rejects_write_watch() {
    let (_d, db) = open();
    let h = db.run_sync();

    let assert_rejected = |sql: &'static str| {
        let live: LiveQuery<i64> = h.watch_scalar(sql, &[]).watching(&["audit"]);
        let error = live
            .recv_timeout(Duration::from_secs(2))
            .expect_err("write watch는 실패해야 함");
        assert!(
            error.to_string().contains("readonly database"),
            "예상하지 못한 오류: {error}"
        );
    };

    assert_rejected("INSERT INTO audit (note) VALUES ('notifier-write') RETURNING id");
    h.execute("INSERT INTO audit (note) VALUES ('seed')", params![])
        .unwrap();
    assert_rejected("UPDATE audit SET note = 'notifier-write' WHERE note = 'seed' RETURNING id");
    assert_rejected("DELETE FROM audit WHERE note = 'seed' RETURNING id");
    assert_rejected("CREATE TABLE notifier_write (id INTEGER)");

    let count: i64 = h
        .with_connection(|conn| {
            conn.query_row("SELECT COUNT(*) FROM audit", [], |row| row.get(0))
                .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(count, 1, "notifier에서 DML이 실행되면 안 됨");
    let table_count: i64 = h
        .with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'notifier_write'",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(table_count, 0, "notifier에서 DDL이 실행되면 안 됨");
}

/// notifier on_open은 write를 허용한 뒤 read-only 상태를 복원한다.
#[test]
fn notifier_on_open_can_write_then_restores_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("l.db"))
        .on_open(|conn| {
            conn.execute_batch(
                "CREATE TEMP TABLE notifier_on_open (id INTEGER); \
                 INSERT INTO notifier_on_open DEFAULT VALUES",
            )?;
            Ok(())
        })
        .build()
        .unwrap();
    let h = db.run_sync();

    let read: LiveQuery<i64> = h
        .watch_scalar("SELECT COUNT(*) FROM notifier_on_open NOT INDEXED", &[])
        .watching(&["notifier_on_open"]);
    assert_eq!(next(&read), 1, "notifier on_open write 결과");

    let write: LiveQuery<i64> = h.watch_scalar(
        "INSERT INTO audit (note) VALUES ('after-on-open') RETURNING id",
        &[],
    );
    let error = write
        .recv_timeout(Duration::from_secs(2))
        .expect_err("on_open 뒤 write watch는 실패해야 함");
    assert!(
        error.to_string().contains("readonly database"),
        "예상하지 못한 오류: {error}"
    );
}

/// WHERE 없는 DELETE(truncate 최적화) — 문장 기반 주 경로가 잡는다 (명세 §9.2)
#[test]
fn truncate_delete_invalidates() {
    let (_d, db) = open();
    let h = db.run_sync();
    let dao = h.item_dao();
    dao.add(&Item {
        id: 0,
        name: "x".into(),
        done: false,
    })
    .unwrap();

    let live = dao.watch_count();
    wait_for(&live, 1);

    // truncate 최적화 경로 — update_hook은 발화하지 않지만 문장 기반은 정확
    h.execute("DELETE FROM items", params![]).unwrap();
    wait_for(&live, 0); // truncate DELETE 무효화
}

/// 트리거 간접 write — update_hook 보조 경로 (명세 §9.2)
#[test]
fn trigger_indirect_write_detected() {
    let (_d, db) = open();
    let h = db.run_sync();

    // items INSERT 시 audit에 기록하는 사용자 트리거
    h.with_connection(|c| {
        c.execute_batch(
            "CREATE TRIGGER trg_audit AFTER INSERT ON items \
             BEGIN INSERT INTO audit (note) VALUES ('삽입됨'); END",
        )?;
        Ok(())
    })
    .unwrap();

    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM audit", &[]);
    assert_eq!(next(&live), 0);

    // items에만 쓰지만 트리거가 audit을 수정 — hook이 잡아야 함
    h.item_dao()
        .add(&Item {
            id: 0,
            name: "t".into(),
            done: false,
        })
        .unwrap();
    assert_eq!(next(&live), 1, "트리거 간접 write 감지");
}

/// 롤백 = 미발동 (명세 §9.2 — commit 후 방출)
#[test]
fn rollback_does_not_emit() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live = h.item_dao().watch_count();
    assert_eq!(next(&live), 0);

    let r: roomrs::Result<()> = h.transaction(|tx| {
        tx.execute(
            "INSERT INTO items (name, done) VALUES ('롤백', 0)",
            params![],
        )?;
        Err(roomrs::Error::Config("의도적".into()))
    });
    assert!(r.is_err());
    assert!(
        live.recv_timeout(Duration::from_millis(300))
            .unwrap()
            .is_none(),
        "롤백 = emit 없음"
    );

    // 커밋되는 트랜잭션은 emit
    h.transaction(|tx| {
        tx.execute(
            "INSERT INTO items (name, done) VALUES ('커밋', 0)",
            params![],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(next(&live), 1, "커밋 = emit");
}

/// LiveQuery/가드 drop 후 emit 0 (명세 §5.6 수명 계약)
#[test]
fn drop_stops_emissions() {
    let (_d, db) = open();
    let h = db.run_sync();

    // subscribe 가드 drop
    let live = h.item_dao().watch_count();
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let c2 = counter.clone();
    let guard = live.subscribe(move |_| {
        c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    // 초기 emit 폴링 수렴 — 고정 sleep은 느린 러너에서 플레이키 (L-19)
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while counter.load(std::sync::atomic::Ordering::SeqCst) == 0
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let before = counter.load(std::sync::atomic::Ordering::SeqCst);
    assert!(before >= 1, "구독 즉시 emit");

    drop(guard);
    // 프로브 구독의 수렴으로 노티파이어가 write를 처리했음을 보장 — 고정 sleep 대체 (L-19)
    let probe = h.item_dao().watch_count();
    h.execute(
        "INSERT INTO items (name, done) VALUES ('после', 0)",
        params![],
    )
    .unwrap();
    wait_for(&probe, 1); // 노티파이어 write 처리 완료 시점
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        before,
        "가드 drop 후 콜백 emit 0"
    );

    // LiveQuery 자체 drop — 구독 해제 후 write가 노티파이어를 방해하지 않음
    drop(live);
    h.execute(
        "INSERT INTO items (name, done) VALUES ('추가', 0)",
        params![],
    )
    .unwrap();
}

/// rebind — 새 바인딩 재조회, 스테일 폐기 (명세 §5.6/C-8)
#[test]
fn rebind_requeries() {
    let (_d, db) = open();
    let h = db.run_sync();
    let dao = h.item_dao();
    dao.add(&Item {
        id: 0,
        name: "미완".into(),
        done: false,
    })
    .unwrap();
    dao.add(&Item {
        id: 0,
        name: "완료".into(),
        done: true,
    })
    .unwrap();

    let live = dao.watch_by_done(false);
    let first = next(&live);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].name, "미완");

    // done=true로 rebind
    live.rebind(params![true]).unwrap();
    // rebind 직후 emit — 이전 세대 값일 수 없음 (epoch 폐기)
    let mut got = next(&live);
    // 드물게 rebind 전 큐에 남은 값이 있으면 한 번 더
    if got.len() == 1 && got[0].name == "미완" {
        got = next(&live);
    }
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "완료", "rebind 후 새 바인딩 결과");
}

/// 직접 쿼리 watch + watching 해소 (명세 §5.7)
#[test]
fn direct_watch_and_watching() {
    let (_d, db) = open();
    let h = db.run_sync();

    let live: LiveQuery<i64> =
        h.watch_scalar("SELECT COUNT(*) FROM items WHERE done = ?1", params![false]);
    assert_eq!(next(&live), 0);
    h.execute("INSERT INTO items (name, done) VALUES ('d', 0)", params![])
        .unwrap();
    assert_eq!(next(&live), 1);
}

/// 콜백 panic — 노티파이어 생존, 다른 라이브 쿼리 정상 동작 (H-4)
#[test]
fn panicking_callback_does_not_kill_notifier() {
    let (_d, db) = open();
    let h = db.run_sync();
    let bad = h.item_dao().watch_count();
    let good = h.item_dao().watch_count();

    // panic 하는 콜백 등록 — 노티파이어를 죽이면 안 된다
    let guard = bad.subscribe(|_| panic!("의도적 콜백 panic"));
    assert_eq!(next(&good), 0, "정상 구독 초기 emit");

    h.execute("INSERT INTO items (name, done) VALUES ('p', 0)", params![])
        .unwrap();
    wait_for(&good, 1); // 노티파이어 생존 — 다른 구독 emit 계속

    // panic 이후에도 가드 drop/추가 write 정상 (락 poison 복구)
    drop(guard);
    drop(bad);
    h.execute("INSERT INTO items (name, done) VALUES ('q', 0)", params![])
        .unwrap();
    wait_for(&good, 2);
}

/// 콜백 내 재진입 — 콜백(노티파이어 스레드)에서 watch 생성/drop 해도 교착 없음 (H-1/M-1)
#[test]
fn callback_reentrancy_no_deadlock() {
    let (_d, db) = open();
    let db = std::sync::Arc::new(db);
    let h = db.run_sync();
    let live = h.item_dao().watch_count();

    let (tx, rx) = std::sync::mpsc::channel::<i64>();
    let db2 = std::sync::Arc::clone(&db);
    let once = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let once2 = once.clone();
    let guard = live.subscribe(move |v| {
        if once2.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // 재진입: 콜백 안에서 새 구독 등록(레지스트리 락) 후 즉시 해제
        let inner: LiveQuery<i64> = db2
            .run_sync()
            .watch_scalar("SELECT COUNT(*) FROM items", &[]);
        drop(inner);
        let _ = tx.send(v);
    });

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).expect("교착 없음"),
        0,
        "콜백 내 재진입 완료"
    );
    // 노티파이어 생존 확인
    h.execute("INSERT INTO items (name, done) VALUES ('r', 0)", params![])
        .unwrap();
    wait_for(&live, 1);
    drop(guard);
}

/// watching 체이닝 — 스테일 UnknownDependencies를 첫 recv에 남기지 않는다 (M-2)
#[test]
fn watching_clears_pending_unknown_deps() {
    let (_d, db) = open();
    let h = db.run_sync();

    // CTE = 의존 추출 실패 경로 → watching으로 즉시 해소
    let live: LiveQuery<i64> = h
        .watch_scalar(
            "WITH t AS (SELECT COUNT(*) AS c FROM items) SELECT c FROM t",
            &[],
        )
        .watching(&["items"]);
    assert_eq!(next(&live), 0, "watching 후 첫 수신 = 값 (에러 아님)");

    h.execute("INSERT INTO items (name, done) VALUES ('w', 0)", params![])
        .unwrap();
    wait_for(&live, 1);
}

/// watching 미호출 — 첫 recv가 UnknownDependencies (M-2 지연 통지)
#[test]
fn unknown_deps_error_on_first_recv() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar(
        "WITH t AS (SELECT COUNT(*) AS c FROM items) SELECT c FROM t",
        &[],
    );
    let r = live.recv_timeout(Duration::from_secs(1));
    assert!(
        matches!(r, Err(roomrs::Error::UnknownDependencies(_))),
        "의존 미상 = UnknownDependencies"
    );
}

/// DDL(ALTER TABLE) = 보수적 전체 무효화 — 워처 깨움 (M-3)
#[test]
fn ddl_triggers_full_invalidation() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live = h.item_dao().watch_count();
    assert_eq!(next(&live), 0);

    h.execute("ALTER TABLE items ADD COLUMN extra TEXT", params![])
        .unwrap();
    assert_eq!(
        live.recv_timeout(Duration::from_secs(2)).unwrap(),
        Some(0),
        "DDL 후 재조회 emit"
    );
}

/// DB drop 후 recv = Closed 에러 — 영구 블로킹 없음 (M-7)
#[test]
fn recv_errors_after_db_drop() {
    let (_d, db) = open();
    let live = db.run_sync().item_dao().watch_count();
    assert_eq!(next(&live), 0);

    drop(db);
    let start = std::time::Instant::now();
    let r = live.recv();
    assert!(
        matches!(r, Err(roomrs::Error::Closed)),
        "DB 종료 = Closed 에러"
    );
    assert!(start.elapsed() < Duration::from_secs(2), "즉시 반환");
    // 이후 호출도 계속 에러
    assert!(matches!(live.recv(), Err(roomrs::Error::Closed)));
}

/// 통합 풀 checkout을 통한 write도 무효화를 방출한다 (M-11).
#[test]
fn checked_out_connection_emits_invalidation() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live = h.item_dao().watch_count();
    assert_eq!(next(&live), 0);

    h.with_connection(|c| {
        c.execute("INSERT INTO items (name, done) VALUES ('esc', 0)", [])?;
        Ok(())
    })
    .unwrap();
    wait_for(&live, 1);
}

/// on_open이 update_hook을 교체해도 roomrs 훅을 마지막에 복구한다.
#[test]
fn on_open_update_hook_does_not_disable_live_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("on-open-hook.db"))
        .on_open(|conn| {
            let _previous =
                conn.update_hook(Some(|_action, _db: &str, _table: &str, _rowid: i64| {}));
            Ok(())
        })
        .build()
        .unwrap();
    let h = db.run_sync();
    let live = h.item_dao().watch_count();
    assert_eq!(next(&live), 0);

    h.with_connection(|conn| {
        conn.execute("INSERT INTO items (name, done) VALUES ('hook', 0)", [])?;
        Ok(())
    })
    .unwrap();
    wait_for(&live, 1);
}

/// subscribe는 기존 구독자(recv 채널)에 재-emit하지 않는다 (L-7)
#[test]
fn subscribe_no_duplicate_emit() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live = h.item_dao().watch_count();
    assert_eq!(next(&live), 0, "초기 emit");

    let (tx, rx) = std::sync::mpsc::channel::<i64>();
    let guard = live.subscribe(move |v| {
        let _ = tx.send(v);
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        0,
        "새 콜백은 현재 값(캐시) 수신"
    );
    assert!(
        live.recv_timeout(Duration::from_millis(400))
            .unwrap()
            .is_none(),
        "기존 recv 채널 재-emit 없음"
    );
    drop(guard);
}

/// savepoint 롤백된 write = 무효화 없음 (L-8)
#[test]
fn savepoint_rollback_no_spurious_invalidation() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live = h.item_dao().watch_count();
    assert_eq!(next(&live), 0);

    h.transaction(|tx| {
        let r: roomrs::Result<()> = tx.savepoint(|sp| {
            sp.execute("INSERT INTO items (name, done) VALUES ('sp', 0)", params![])?;
            Err(roomrs::Error::Config("의도적".into()))
        });
        assert!(r.is_err());
        Ok(())
    })
    .unwrap();
    assert!(
        live.recv_timeout(Duration::from_millis(400))
            .unwrap()
            .is_none(),
        "롤백된 savepoint write = emit 없음"
    );

    // 정상 write는 emit — 채널 정상 동작 확인
    h.execute("INSERT INTO items (name, done) VALUES ('ok', 0)", params![])
        .unwrap();
    wait_for(&live, 1);
}

/// into_stream — 비동기 Stream 소비 (명세 §5.6)
#[cfg(all(feature = "async", not(feature = "tokio")))]
#[test]
fn into_stream_consumption() {
    use futures::StreamExt;

    let (_d, db) = open();
    smol::block_on(async {
        let h = db.run_async();
        let live = h.item_dao().watch_count();
        let mut stream = live.into_stream();

        let first = stream
            .next()
            .await
            .expect("스트림 열림")
            .expect("쿼리 성공");
        assert_eq!(first, 0, "초기 emit");

        h.execute("INSERT INTO items (name, done) VALUES ('s', 0)", ())
            .await
            .unwrap();
        let second = stream
            .next()
            .await
            .expect("스트림 열림")
            .expect("쿼리 성공");
        assert_eq!(second, 1, "write 후 스트림 emit");
    });
}

/// recv 소비는 미소비 중간값을 버리고 최신값 하나만 유지한다 (결정 30).
#[test]
fn recv_keeps_only_latest_value() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let _guard = live.subscribe(move |value| {
        let _ = tx.send(value);
    });
    let _ = rx.recv_timeout(Duration::from_secs(1));

    for i in 0..8 {
        h.execute(
            "INSERT INTO items(name, done) VALUES (?1, 0)",
            params![format!("latest-{i}")],
        )
        .unwrap();
        while rx.recv_timeout(Duration::from_secs(1)).unwrap() != i + 1 {}
    }

    assert_eq!(live.try_recv().unwrap(), Some(8));
    assert_eq!(live.try_recv().unwrap(), None, "단일 슬롯이어야 함");
}

/// view 의존성은 기저 테이블을 알 수 없으므로 명시 전 UnknownDependencies다.
#[test]
fn view_watch_requires_explicit_dependencies() {
    let (_d, db) = open();
    let h = db.run_sync();
    h.execute("CREATE VIEW item_view AS SELECT * FROM items", params![])
        .unwrap();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM item_view", &[]);
    assert!(matches!(
        live.recv_timeout(Duration::from_secs(1)),
        Err(roomrs::Error::UnknownDependencies(_))
    ));

    let live = live.watching(&["items"]);
    assert_eq!(next(&live), 0);
}

/// CTE-DML execute는 보수적 전체 무효화로 실제 라이브 재조회를 일으킨다.
#[test]
fn cte_dml_execute_invalidates_live_query() {
    let (_d, db) = open();
    let h = db.run_sync();
    h.execute("INSERT INTO items(name, done) VALUES ('cte', 0)", params![])
        .unwrap();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 1);

    h.execute(
        "WITH doomed AS (SELECT id FROM items WHERE name='cte') \
         DELETE FROM items WHERE id IN (SELECT id FROM doomed)",
        params![],
    )
    .unwrap();
    assert_eq!(next(&live), 0);
}

/// shared-cache in-memory checkout은 read_uncommitted가 활성화된다.
#[test]
fn in_memory_connection_enables_read_uncommitted() {
    for _ in 0..20 {
        let db = Db::builder().in_memory().build().unwrap();
        db.run_sync()
            .with_connection(|conn| {
                let enabled: i64 =
                    conn.query_row("PRAGMA read_uncommitted", [], |row| row.get(0))?;
                assert_eq!(enabled, 1);
                Ok(())
            })
            .unwrap();
        let live: LiveQuery<i64> = db
            .run_sync()
            .watch_scalar("SELECT COUNT(*) FROM items", &[]);
        assert_eq!(next(&live), 0);
        db.run_sync()
            .execute("INSERT INTO items(name, done) VALUES ('x', 0)", params![])
            .unwrap();
        assert_eq!(next(&live), 1);
    }
}
