// 2차 리뷰 수정 검증 통합 테스트 —
// H-1 RETURNING write 무효화 · H-2 서브쿼리 의존 추출 · H-3 콜백 내 DB drop 무교착 ·
// M-2 execute_batch 무효화 · M-5 동시 마이그레이션 감지 · L-2 SELECT 무방출 · L-4 iter fuse

#[cfg(feature = "live")]
use roomrs::params;
use roomrs::{database, entity};
use std::time::Duration;

#[cfg(feature = "live")]
use roomrs::LiveQuery;

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

#[database(entities(Item, Audit), version = 1)]
struct Db;

/// 테스트 DB 오픈 (파일 기반)
#[cfg(feature = "live")]
fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("r2.db"))
        .build()
        .unwrap();
    (dir, db)
}

/// emit 대기 헬퍼 — 최대 2초
#[cfg(feature = "live")]
fn next<T: Clone + Send + 'static>(q: &LiveQuery<T>) -> T {
    q.recv_timeout(Duration::from_secs(2))
        .expect("수신 에러")
        .expect("emit 타임아웃")
}

/// 기대값 수렴 대기 — 과잉 emit(§9.4 최종 일관성)을 흡수하며 기대값 도달 확인
#[cfg(feature = "live")]
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

/// H-1 — INSERT…RETURNING을 쿼리 경로(query_one)로 실행해도 무효화가 방출된다
#[cfg(feature = "live")]
#[test]
fn returning_write_via_query_wakes_watcher() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);

    // 자동 커밋 경로 — SyncHandle 쿼리 함수의 writer 분기 (H-1)
    let row: Item = h
        .query_one(
            "INSERT INTO items (name, done) VALUES ('r', 0) RETURNING *",
            params![],
        )
        .unwrap();
    assert_eq!(row.name, "r");
    wait_for(&live, 1);

    // 트랜잭션 경로 — Tx의 writer-형 쿼리도 커밋 후 무효화 (H-1)
    h.transaction(|tx| {
        let _: Item = tx.query_one(
            "INSERT INTO items (name, done) VALUES ('t', 0) RETURNING *",
            params![],
        )?;
        Ok(())
    })
    .unwrap();
    wait_for(&live, 2);
}

/// R3-1 — OR FAIL 부분 적용: 문장 에러 후에도 선행 행이 영속된다 — 무효화 방출 필수
#[cfg(feature = "live")]
#[test]
fn or_fail_partial_insert_still_invalidates() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);

    // id 중복 — OR FAIL은 에러 시 선행 행 변경을 되돌리지 않는다
    let r = h.execute(
        "INSERT OR FAIL INTO items (id, name, done) VALUES (1, 'a', 0), (1, 'b', 0)",
        params![],
    );
    assert!(r.is_err(), "중복 pk = 에러");
    wait_for(&live, 1); // 선행 1행 영속 + 무효화 방출 (R3-1)
}

/// R2-2 — SQL은 성공했으나 결과 매핑이 실패한 RETURNING write도 무효화된다
/// (RETURNING은 첫 step에서 DML 완결 — write는 영속)
#[cfg(feature = "live")]
#[test]
fn failed_mapping_returning_still_invalidates() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);

    // name(TEXT)을 i64로 매핑 → 타입 불일치 실패. INSERT 자체는 영속된다
    let r: roomrs::Result<i64> = h.query_scalar(
        "INSERT INTO items (name, done) VALUES ('불일치', 0) RETURNING name",
        params![],
    );
    assert!(r.is_err(), "매핑 실패 기대");
    wait_for(&live, 1); // write 영속 + 무효화 방출 (R2-2)

    // 트랜잭션 경로 — Tx의 매핑 실패 write도 커밋 시 무효화
    h.transaction(|tx| {
        let r: roomrs::Result<i64> = tx.query_scalar(
            "INSERT INTO items (name, done) VALUES ('둘', 0) RETURNING name",
            params![],
        );
        assert!(r.is_err(), "매핑 실패 기대");
        Ok(())
    })
    .unwrap();
    wait_for(&live, 2);
}

/// H-2 — WHERE IN (서브쿼리)의 내부 테이블 write도 재조회를 깨운다
#[cfg(feature = "live")]
#[test]
fn subquery_dependency_refreshes() {
    let (_d, db) = open();
    let h = db.run_sync();
    h.execute(
        "INSERT INTO items (id, name, done) VALUES (1, 'a', 0)",
        params![],
    )
    .unwrap();

    let live: LiveQuery<i64> = h.watch_scalar(
        "SELECT COUNT(*) FROM items WHERE id IN (SELECT id FROM audit)",
        &[],
    );
    // 의존 추출이 서브쿼리를 방문해야 첫 수신이 에러가 아니다 (H-2)
    assert_eq!(next(&live), 0, "초기 emit — audit 비어 있음");

    // 내부(서브쿼리) 테이블 write → 재조회
    h.execute("INSERT INTO audit (id, note) VALUES (1, 'x')", params![])
        .unwrap();
    wait_for(&live, 1);
}

/// H-3 — 구독 콜백 안에서 마지막 Database가 drop돼도 교착하지 않는다
#[cfg(feature = "live")]
#[test]
fn drop_db_inside_callback_no_deadlock() {
    let (finished_tx, finished_rx) = std::sync::mpsc::channel::<()>();
    // 본문을 별도 스레드에서 실행 — 교착 시 감시 타임아웃으로 실패
    std::thread::spawn(move || {
        let db = Db::builder().in_memory().build().unwrap();
        let live: LiveQuery<i64> = db
            .run_sync()
            .watch_scalar("SELECT COUNT(*) FROM items", &[]);
        assert_eq!(next(&live), 0);

        let (cb_tx, cb_rx) = std::sync::mpsc::channel::<()>();
        let mut slot = Some(db);
        let guard = live.subscribe(move |_| {
            // 첫 호출에서 마지막 Database drop — DatabaseInner::drop이
            // 노티파이어 스레드 위에서 실행된다 (H-3 self-join 경로)
            if let Some(db) = slot.take() {
                drop(db);
            }
            let _ = cb_tx.send(());
        });
        cb_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("콜백 내 DB drop이 완료되지 않음 (H-3 교착 의심)");
        drop(guard);
        drop(live);
        let _ = finished_tx.send(());
    });
    finished_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("본문 타임아웃 — 노티파이어 self-join 교착 의심 (H-3)");
}

/// M-2 — Tx::execute_batch write도 커밋 후 무효화를 방출한다
#[cfg(feature = "live")]
#[test]
fn tx_execute_batch_invalidates() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);

    h.transaction(|tx| {
        tx.execute_batch(
            "INSERT INTO items (name, done) VALUES ('b1', 0); \
             INSERT INTO items (name, done) VALUES ('b2', 1)",
        )
    })
    .unwrap();
    wait_for(&live, 2);
}

/// L-2 — execute("SELECT …") 성공은 무효화를 방출하지 않는다
#[cfg(feature = "live")]
#[test]
fn execute_select_does_not_invalidate() {
    let (_d, db) = open();
    let h = db.run_sync();
    let live: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);

    // 행을 반환하지 않는 SELECT — execute 성공, 문장 기반 방출 없어야 함 (L-2)
    h.execute("SELECT id FROM items WHERE 1 = 0", params![])
        .unwrap();
    assert!(
        live.recv_timeout(Duration::from_millis(400))
            .unwrap()
            .is_none(),
        "SELECT = 무효화 없음"
    );

    // write는 여전히 방출 — 채널 정상 확인
    h.execute("INSERT INTO items (name, done) VALUES ('w', 0)", params![])
        .unwrap();
    wait_for(&live, 1);
}

/// L-4 — DB 종료 후 iter는 Closed 1회 방출 뒤 None (fuse)
#[cfg(feature = "live")]
#[test]
fn iter_fuses_after_closed() {
    let (_d, db) = open();
    let live: LiveQuery<i64> = db
        .run_sync()
        .watch_scalar("SELECT COUNT(*) FROM items", &[]);
    assert_eq!(next(&live), 0);
    drop(db);

    let mut it = live.iter();
    let mut closed_count = 0u32;
    // 잔여 값 소진 → Closed 1회 → 종료 (무한 반복이면 여기서 영원히 돈다)
    for r in it.by_ref() {
        match r {
            Err(roomrs::Error::Closed) => closed_count += 1,
            Ok(_) => {}
            Err(e) => panic!("예상 밖 에러: {e}"),
        }
    }
    assert_eq!(closed_count, 1, "Closed는 정확히 1회");
    assert!(it.next().is_none(), "fuse — 이후 None");
}

// ───── M-5 — 체인 구성이 다른 두 인스턴스의 동시 마이그레이션 감지 ─────

#[entity(table = "docs")]
struct DocV1 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
}

#[database(entities(DocV1), version = 1)]
struct MigDb1;

#[entity(table = "docs")]
struct DocV2 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    note: String,
}

#[database(entities(DocV2), version = 2)]
struct MigDb2;

#[entity(table = "docs")]
struct DocV3 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    note: String,
    extra: String,
}

#[database(entities(DocV3), version = 3)]
struct MigDb3;

/// M-5 — 인스턴스 A(체인 1→2)가 끼어들면 인스턴스 B(체인 1→3)는
/// 스텝 시작 버전 불일치를 감지하고 잘못된 SQL 적용을 거부한다
#[test]
fn concurrent_migration_chain_mismatch_detected() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering as AO};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m5.db");
    // v1 DB 생성
    drop(MigDb1::builder().sqlite(&path).build().unwrap());

    let entered = Arc::new(AtomicBool::new(false));
    let go = Arc::new(AtomicBool::new(false));

    // A 역할 — 1→2 스텝 트랜잭션 안(IMMEDIATE 락 보유)에서 신호를 기다린다
    let (e2, g2) = (entered.clone(), go.clone());
    let p_a = path.clone();
    let a = std::thread::spawn(move || {
        MigDb2::builder()
            .sqlite(&p_a)
            .migration(roomrs::Migration::code(1, 2, move |tx| {
                tx.execute_batch(
                    r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#,
                )?;
                e2.store(true, AO::SeqCst);
                // B가 current=1을 읽고 스텝 락에서 대기할 때까지 유지
                while !g2.load(AO::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(())
            }))
            .build()
            .map(|_| ())
    });

    // A가 스텝 트랜잭션 안에 들어갈 때까지 대기
    while !entered.load(AO::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }

    // B 역할 — 다른 체인(1→3). A의 락 보유 중이라 current=1을 읽은 뒤
    // BEGIN IMMEDIATE에서 블록되고, A가 v2를 커밋한 후 진입한다
    let p_b = path.clone();
    let b = std::thread::spawn(move || {
        MigDb3::builder()
            .sqlite(&p_b)
            .busy_timeout(Duration::from_secs(20))
            .migration(roomrs::Migration::sql(
                1,
                3,
                r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT '';
                   ALTER TABLE "docs" ADD COLUMN "extra" TEXT NOT NULL DEFAULT ''"#,
            ))
            .build()
            .map(|_| ())
    });

    // B가 current 읽기 + 스텝 락 대기에 도달할 여유
    std::thread::sleep(Duration::from_millis(1000));
    go.store(true, AO::SeqCst);

    a.join().unwrap().expect("A(1→2) 마이그레이션 성공");
    match b.join().unwrap() {
        Err(roomrs::Error::Migration(msg)) => {
            assert!(msg.contains("동시 마이그레이션 감지"), "{msg}");
        }
        other => panic!("동시 마이그레이션 감지 에러 기대, 결과: {other:?}"),
    }
}
