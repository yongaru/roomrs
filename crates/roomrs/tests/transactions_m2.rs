// M2 검증 통합 테스트 (명세 §15 M2) —
// #[transaction] 같은 커넥션 · 원자성 · savepoint 중첩 · Future 취소 종결

use roomrs::{dao, database, entity, params};

#[entity(table = "accounts")]
#[derive(Debug, Clone)]
struct Account {
    #[pk(autoincrement)]
    id: i64,
    name: String,
    balance: i64,
}

#[dao]
trait AccountDao {
    #[insert]
    fn add(&self, a: &Account) -> roomrs::Result<i64>;

    #[query("SELECT balance FROM accounts WHERE id = :id")]
    fn balance(&self, id: i64) -> roomrs::Result<i64>;

    #[update("UPDATE accounts SET balance = balance + :delta WHERE id = :id")]
    fn adjust(&self, id: i64, delta: i64) -> roomrs::Result<u64>;

    /// 이체 — 본문이 하나의 트랜잭션에서 원자 실행 (명세 §5.9).
    /// 내부 self 호출은 매크로 재작성으로 같은 트랜잭션 커넥션을 쓴다 —
    /// 통합 풀에서는 재획득이 데드락 대신 다른 커넥션으로 진행될 수 있으므로
    /// 이 테스트만으로 같은-커넥션이 증명되지는 않는다(원자성 검증이 목적).
    #[transaction]
    fn transfer(&self, from: i64, to: i64, amount: i64) -> roomrs::Result<()> {
        let src = self.balance(from)?;
        if src < amount {
            return Err(roomrs::Error::Config("잔액 부족".into()));
        }
        self.adjust(from, -amount)?;
        self.adjust(to, amount)?;
        Ok(())
    }
}

#[database(entities(Account), daos(AccountDao), version = 1)]
struct Db;

/// 테스트 DB + 계좌 2개 (a=100, b=0)
fn setup() -> (tempfile::TempDir, Db, i64, i64) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("t.db"))
        .build()
        .unwrap();
    let h = db.run_sync();
    let a = h
        .account_dao()
        .add(&Account {
            id: 0,
            name: "a".into(),
            balance: 100,
        })
        .unwrap();
    let b = h
        .account_dao()
        .add(&Account {
            id: 0,
            name: "b".into(),
            balance: 0,
        })
        .unwrap();
    (dir, db, a, b)
}

/// #[transaction] 성공 경로 — 같은 커넥션(데드락 0) + 원자 반영
#[test]
fn transaction_method_commits() {
    let (_d, db, a, b) = setup();
    let h = db.run_sync();
    h.account_dao().transfer(a, b, 30).unwrap();
    assert_eq!(h.account_dao().balance(a).unwrap(), 70);
    assert_eq!(h.account_dao().balance(b).unwrap(), 30);
}

/// #[transaction] 실패 경로 — 중간 에러 = 전체 롤백 (원자성)
#[test]
fn transaction_method_rolls_back_atomically() {
    let (_d, db, a, b) = setup();
    let h = db.run_sync();
    // 잔액 부족 = 에러 (본문 초입 실패 경로)
    assert!(
        h.account_dao().transfer(a, b, 1000).is_err(),
        "잔액 부족 = 에러"
    );
    assert_eq!(h.account_dao().balance(a).unwrap(), 100);

    // 중간 실패: 첫 adjust 후 에러 — savepoint/rollback으로 첫 adjust도 취소돼야 함
    use DbTxDaos as _;
    let r: roomrs::Result<()> = h.transaction(|tx| {
        tx.account_dao().adjust(a, -40)?;
        Err(roomrs::Error::Config("중간 실패".into()))
    });
    assert!(r.is_err());
    assert_eq!(
        h.account_dao().balance(a).unwrap(),
        100,
        "롤백으로 원상복구"
    );
}

/// 중첩 #[transaction] = savepoint — 내부 실패는 내부만 롤백
#[test]
fn nested_savepoint() {
    let (_d, db, a, b) = setup();
    let h = db.run_sync();

    use DbTxDaos as _;
    h.transaction(|tx| {
        tx.account_dao().adjust(a, -10)?; // 외부 변경

        // 내부 트랜잭션(= savepoint) 실패 — 내부 변경만 취소
        let inner: roomrs::Result<()> = roomrs::SqlContext::ctx_transaction(&&*tx, |sp| {
            sp.account_dao().adjust(b, 999)?;
            Err(roomrs::Error::Config("내부 실패".into()))
        });
        assert!(inner.is_err());

        tx.account_dao().adjust(b, 10)?; // 외부 계속
        Ok(())
    })
    .unwrap();

    assert_eq!(h.account_dao().balance(a).unwrap(), 90);
    assert_eq!(
        h.account_dao().balance(b).unwrap(),
        10,
        "내부 999는 롤백, 외부 10만 반영"
    );
}

/// panic 시 롤백 — RAII drop 경로
#[test]
fn panic_rolls_back() {
    let (_d, db, a, _b) = setup();
    let h = db.run_sync();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = h.transaction(|tx| -> roomrs::Result<()> {
            tx.execute("UPDATE accounts SET balance = 0", params![])?;
            panic!("의도적 패닉");
        });
    }));
    assert!(result.is_err());
    assert_eq!(h.account_dao().balance(a).unwrap(), 100, "패닉 = 롤백");
}

/// BEGIN IMMEDIATE — 트랜잭션 시작 즉시 write 락 확보 (H-3:
/// WAL read→write 승격의 SQLITE_BUSY_SNAPSHOT 우회 차단)
#[test]
fn begin_immediate_blocks_second_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("imm.db");
    let db1 = Db::builder().sqlite(&path).build().unwrap();
    let db2 = Db::builder()
        .sqlite(&path)
        .busy_timeout(std::time::Duration::from_millis(100))
        .build()
        .unwrap();

    let h1 = db1.run_sync();
    let tx = h1.begin().unwrap(); // IMMEDIATE = 문장 실행 전에도 write 락 보유
    let r = db2.run_sync().execute(
        "INSERT INTO accounts (name, balance) VALUES ('x', 1)",
        params![],
    );
    assert!(r.is_err(), "IMMEDIATE 락으로 두 번째 프로세스 writer 차단");

    tx.rollback().unwrap();
    db2.run_sync()
        .execute(
            "INSERT INTO accounts (name, balance) VALUES ('x', 1)",
            params![],
        )
        .unwrap();
}

/// 통합 풀 checkout 커넥션은 writable PRAGMA와 write를 허용한다.
#[test]
fn checked_out_connection_accepts_writable_pragma_and_write() {
    let (_d, db, _a, _b) = setup();
    db.run_sync()
        .with_connection(|c| {
            c.pragma_update(None, "cache_size", -777)?;
            c.execute("INSERT INTO accounts (name, balance) VALUES ('r', 0)", [])?;
            Ok(())
        })
        .expect("쓰기 가능 checkout");
}

/// 동시 오픈 — 마이그레이션(신규 생성) 경합 없이 전부 성공 (M-4)
#[test]
fn concurrent_open_migration_race() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let mut handles = Vec::new();
    for _ in 0..4 {
        let p = path.clone();
        handles.push(std::thread::spawn(move || {
            Db::builder().sqlite(&p).build().map(|_| ())
        }));
    }
    for h in handles {
        h.join().expect("스레드 join").expect("동시 오픈 성공");
    }
}

/// 비동기 #[transaction] + Future 취소 종결 (명세 §5.5 취소 의미론)
#[cfg(all(feature = "async", not(feature = "tokio")))]
#[test]
fn async_transaction_and_cancellation() {
    let (_d, db, a, b) = setup();

    smol::block_on(async {
        let h = db.run_async();

        // 정상 비동기 #[transaction] — 소유 인자(i64)라 'static 충족
        h.account_dao().transfer(a, b, 20).await.unwrap();

        // 취소: 시작 전 drop = 미실행
        drop(h.account_dao().transfer(a, b, 5));

        // 취소: 시작 후 drop = 트랜잭션은 종결(커밋)되고 결과만 폐기.
        // 1회 poll로 워커 제출을 보장한 뒤 drop.
        {
            let dao = h.account_dao();
            let fut = dao.transfer(a, b, 10);
            futures::pin_mut!(fut);
            let _ = futures::poll!(&mut fut); // 제출
        } // fut drop

        // 워커 완료 대기 — writer가 잠기지 않았고 결과가 종결됐음을 후속 쿼리로 확인
        let mut done = false;
        for _ in 0..100 {
            let a_bal = h.account_dao().balance(a).await.unwrap();
            if a_bal == 70 {
                done = true; // 100 - 20 - 10 = 70 (취소된 tx도 커밋 완료)
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(done, "취소된 트랜잭션이 커밋으로 종결되어야 함");
        assert_eq!(h.account_dao().balance(b).await.unwrap(), 30);
    });
}
