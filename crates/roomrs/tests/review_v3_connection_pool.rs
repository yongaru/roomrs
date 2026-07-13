//! 3차 리뷰 연결 초기화·풀 복구 회귀 테스트.

use roomrs::{database, entity};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[entity(table = "items")]
struct Item {
    #[pk]
    id: i64,
}

#[database(entities(Item), version = 1)]
struct Db;

/// 지정한 통합 커넥션 수로 임시 DB를 연다.
fn open_db(connections: usize) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("임시 디렉터리 생성");
    let db = Db::builder()
        .sqlite(dir.path().join("pool.db"))
        .connections(connections)
        .build()
        .expect("DB 열기");
    (dir, db)
}

/// on_open은 모든 일반 커넥션과 notifier 연결에 각각 적용된다.
#[test]
fn on_open_runs_for_every_internal_connection() {
    let dir = tempfile::tempdir().expect("임시 디렉터리 생성");
    let opens = Arc::new(AtomicUsize::new(0));
    let callback_opens = Arc::clone(&opens);

    let _db = Db::builder()
        .sqlite(dir.path().join("hooks.db"))
        .connections(2)
        .on_open(move |conn| {
            callback_opens.fetch_add(1, Ordering::SeqCst);
            let table_exists: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='items'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(table_exists, 1, "on_open은 migration 뒤 실행되어야 함");
            conn.pragma_update(None, "cache_size", -321)?;
            Ok(())
        })
        .build()
        .expect("DB 열기");

    let expected = if cfg!(feature = "live") { 3 } else { 2 };
    assert_eq!(opens.load(Ordering::SeqCst), expected);
}

/// notifier 전용 연결에서도 callback의 연결 로컬 설정을 실제 조회에 사용한다.
#[cfg(feature = "live")]
#[test]
fn notifier_uses_on_open_configuration() {
    let dir = tempfile::tempdir().expect("임시 디렉터리 생성");
    let db = Db::builder()
        .sqlite(dir.path().join("notifier-hook.db"))
        .on_open(|conn| {
            conn.pragma_update(None, "cache_size", -654)?;
            Ok(())
        })
        .build()
        .expect("DB 열기");

    let live = db.run_sync().watch_scalar::<i64>("PRAGMA cache_size", &[]);
    let value = live
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("초기 조회")
        .expect("초기 값");
    assert_eq!(value, -654);
}

/// on_open이 연 트랜잭션은 풀 투입 전에 롤백하고 쓰기 가능 상태로 복구한다.
#[test]
fn on_open_connection_state_is_sanitized() {
    let dir = tempfile::tempdir().expect("임시 디렉터리 생성");
    let db = Db::builder()
        .sqlite(dir.path().join("dirty-hook.db"))
        .connections(2)
        .on_open(|conn| {
            conn.pragma_update(None, "query_only", "OFF")?;
            conn.execute_batch("BEGIN; INSERT INTO items(id) VALUES (10)")?;
            Ok(())
        })
        .build()
        .expect("오염 callback 뒤 DB 열기");

    db.run_sync()
        .with_connection(|conn| {
            assert!(conn.is_autocommit());
            let query_only: i64 = conn.query_row("PRAGMA query_only", [], |row| row.get(0))?;
            assert_eq!(query_only, 0);
            Ok(())
        })
        .expect("통합 풀 불변식 확인");
    let count = db
        .run_sync()
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT count(*) FROM items", [], |row| row.get::<_, i64>(0))?)
        })
        .expect("행 수 확인");
    assert_eq!(count, 0);
}

/// on_open Err와 panic은 build 에러로 격리된다.
#[test]
fn on_open_failure_is_returned_without_panic() {
    let dir = tempfile::tempdir().expect("임시 디렉터리 생성");
    let error = Db::builder()
        .sqlite(dir.path().join("error-hook.db"))
        .on_open(|_| Err(roomrs::Error::Config("의도적 callback 실패".into())))
        .build();
    assert!(error.is_err());

    let panic = std::panic::catch_unwind(|| {
        Db::builder()
            .sqlite(dir.path().join("panic-hook.db"))
            .on_open(|_| panic!("의도적 callback panic"))
            .build()
    });
    assert!(panic.is_ok(), "callback panic은 build 경계를 넘으면 안 됨");
    assert!(panic.expect("직전 검사").is_err());
}

/// checkout 탈출구가 트랜잭션을 남겨도 반납 때 롤백하고 쓰기를 허용한다.
#[test]
fn checked_out_connection_is_writable_after_restore() {
    let (_dir, db) = open_db(1);
    let handle = db.run_sync();

    handle
        .with_connection(|conn| {
            conn.pragma_update(None, "query_only", "OFF")?;
            conn.execute_batch("BEGIN; INSERT INTO items(id) VALUES (1)")?;
            Ok(())
        })
        .expect("오염 동작 자체는 성공");

    handle
        .with_connection(|conn| {
            assert!(conn.is_autocommit(), "열린 트랜잭션은 롤백되어야 함");
            let query_only: i64 = conn.query_row("PRAGMA query_only", [], |row| row.get(0))?;
            assert_eq!(query_only, 0, "반납 커넥션은 쓰기 가능해야 함");
            conn.execute("INSERT INTO items(id) VALUES (2)", [])?;
            Ok(())
        })
        .expect("복구 상태 확인");

    let count = handle
        .with_connection(|conn| {
            Ok(conn.query_row("SELECT count(*) FROM items", [], |row| row.get::<_, i64>(0))?)
        })
        .expect("행 수 확인");
    assert_eq!(count, 1, "미완료 트랜잭션만 롤백되어야 함");
}

/// checkout Err/panic 경로도 트랜잭션을 롤백하고 쓰기 가능 상태를 복구한다.
#[test]
fn checked_out_connection_is_writable_after_error_and_panic() {
    let (_dir, db) = open_db(1);
    let handle = db.run_sync();
    let error: roomrs::Result<()> = handle.with_connection(|conn| {
        conn.pragma_update(None, "query_only", "OFF")?;
        conn.execute_batch("BEGIN")?;
        Err(roomrs::Error::Config("의도적 실패".into()))
    });
    assert!(error.is_err());

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: roomrs::Result<()> = handle.with_connection(|conn| {
            conn.pragma_update(None, "query_only", "OFF")?;
            conn.execute_batch("BEGIN")?;
            panic!("의도적 panic");
        });
    }));
    assert!(panic.is_err());

    handle
        .with_connection(|conn| {
            assert!(conn.is_autocommit());
            let query_only: i64 = conn.query_row("PRAGMA query_only", [], |row| row.get(0))?;
            assert_eq!(query_only, 0);
            conn.execute("INSERT INTO items(id) VALUES (3)", [])?;
            Ok(())
        })
        .expect("Err/panic 뒤 통합 풀 복구");
}

/// builder queue_timeout은 통합 풀 checkout 고갈에 적용된다.
#[test]
fn pool_checkout_is_exclusive_and_uses_builder_queue_timeout() {
    let dir = tempfile::tempdir().expect("임시 디렉터리 생성");
    let db = Db::builder()
        .sqlite(dir.path().join("reader-timeout.db"))
        .connections(1)
        .queue_timeout(std::time::Duration::from_millis(20))
        .build()
        .expect("DB 열기");
    let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let db_ref = &db;
        scope.spawn(move || {
            db_ref
                .run_sync()
                .with_connection(|_| {
                    acquired_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        acquired_rx.recv().unwrap();
        let result = db.run_sync().with_connection(|_| Ok(()));
        assert!(matches!(result, Err(roomrs::Error::QueueTimeout(_))));
        release_tx.send(()).unwrap();
    });
}

/// 커넥션 탈출구 Err 뒤 열린 트랜잭션을 반납 전에 롤백한다.
#[test]
fn connection_transaction_is_rolled_back_after_error() {
    let (_dir, db) = open_db(1);
    let handle = db.run_sync();

    let result: roomrs::Result<()> = handle.with_connection(|conn| {
        conn.execute_batch("BEGIN; INSERT INTO items(id) VALUES (1)")?;
        Err(roomrs::Error::Config("의도적 실패".into()))
    });
    assert!(result.is_err());

    handle
        .with_connection(|conn| {
            assert!(conn.is_autocommit());
            conn.execute_batch("BEGIN; INSERT INTO items(id) VALUES (2); COMMIT")?;
            Ok(())
        })
        .expect("다음 커넥션 작업은 정상이어야 함");
}

/// 커넥션 탈출구 panic 뒤에도 가드가 열린 트랜잭션을 롤백한다.
#[test]
fn connection_transaction_is_rolled_back_after_panic() {
    let (_dir, db) = open_db(1);
    let handle = db.run_sync();

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: roomrs::Result<()> = handle.with_connection(|conn| {
            conn.execute_batch("BEGIN; INSERT INTO items(id) VALUES (1)")?;
            panic!("의도적 panic");
        });
    }));
    assert!(panic.is_err());

    handle
        .with_connection(|conn| {
            assert!(conn.is_autocommit());
            conn.execute_batch("BEGIN; INSERT INTO items(id) VALUES (2); COMMIT")?;
            Ok(())
        })
        .expect("panic 뒤 커넥션 작업은 정상이어야 함");
}
