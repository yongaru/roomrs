//! 트랜잭션 사용 케이스 (명세 §5.5/§5.9)
//! - #[transaction] 메서드: 본문 전체가 하나의 트랜잭션 (계좌 이체)
//! - 클로저 트랜잭션 + 중첩 savepoint
//! - RAII begin(): drop = 롤백
//!
//! 실행: cargo run --example transactions

use roomrs::{dao, database, entity};

#[entity]
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

    #[query("SELECT balance FROM Account WHERE id = :id")]
    fn balance(&self, id: i64) -> roomrs::Result<i64>;

    #[update("UPDATE Account SET balance = balance + :delta WHERE id = :id")]
    fn adjust(&self, id: i64, delta: i64) -> roomrs::Result<u64>;

    /// 이체 — 내부 self 호출들이 전부 같은 트랜잭션 커넥션을 쓴다.
    /// 중간 실패 시 전체 롤백.
    #[transaction]
    fn transfer(&self, from: i64, to: i64, amount: i64) -> roomrs::Result<()> {
        if self.balance(from)? < amount {
            return Err(roomrs::Error::Config("잔액 부족".into()));
        }
        self.adjust(from, -amount)?;
        self.adjust(to, amount)?;
        Ok(())
    }
}

#[database(entities(Account), daos(AccountDao), version = 1)]
struct Db;

fn main() -> roomrs::Result<()> {
    let db = Db::builder().in_memory().build()?;
    let h = db.run_sync();
    let a = h.account_dao().add(&Account {
        id: 0,
        name: "a".into(),
        balance: 100,
    })?;
    let b = h.account_dao().add(&Account {
        id: 0,
        name: "b".into(),
        balance: 0,
    })?;

    // 1) #[transaction] 이체
    h.account_dao().transfer(a, b, 30)?;
    println!(
        "이체 후: a={} b={}",
        h.account_dao().balance(a)?,
        h.account_dao().balance(b)?
    );

    // 2) 잔액 부족 = 에러 + 원상 유지
    let err = h.account_dao().transfer(a, b, 9999).unwrap_err();
    println!(
        "실패 이체: {err} (a={} 그대로)",
        h.account_dao().balance(a)?
    );

    // 3) 클로저 트랜잭션 + 중첩 savepoint — 내부 실패는 내부만 롤백
    use DbTxDaos as _;
    h.transaction(|tx| {
        tx.account_dao().adjust(a, -10)?; // 외부 변경 유지됨
        let inner: roomrs::Result<()> = roomrs::SqlContext::ctx_transaction(&&*tx, |sp| {
            sp.account_dao().adjust(b, 999)?; // savepoint 롤백으로 취소됨
            Err(roomrs::Error::Config("내부 취소".into()))
        });
        println!("savepoint 결과: {inner:?}");
        Ok(())
    })?;
    println!(
        "중첩 후: a={} b={} (999 미반영)",
        h.account_dao().balance(a)?,
        h.account_dao().balance(b)?
    );

    // 4) RAII — commit 없이 drop = 롤백
    {
        let tx = h.begin()?;
        tx.execute("UPDATE Account SET balance = 0", roomrs::params![])?;
        // drop → 롤백
    }
    println!(
        "RAII drop 후: a={} (롤백 확인)",
        h.account_dao().balance(a)?
    );
    Ok(())
}
