//! 라이브 쿼리 페이지네이션 사용 케이스 (명세 §5.6b)
//! - 화면 = LiveQuery 1개 보관, 페이지 이동 = rebind(같은 SQL, 바인딩 교체)
//! - 총건수 = watch_scalar(COUNT)
//! - write가 들어오면 현재 페이지가 자동 갱신
//!
//! 실행: cargo run --example pagination

use roomrs::{LiveQuery, dao, database, entity, params};
use std::time::Duration;

#[entity(table = "logs")]
#[derive(Debug, Clone)]
struct Log {
    #[pk(autoincrement)]
    id: i64,
    msg: String,
}

#[dao]
trait LogDao {
    #[insert]
    fn add(&self, l: &Log) -> roomrs::Result<i64>;
}

#[database(entities(Log), daos(LogDao), version = 1)]
struct Db;

const PAGE: i64 = 5;

/// emit 1건 수신 (데모용 블로킹)
fn recv<T: Clone + Send + 'static>(q: &LiveQuery<T>) -> T {
    q.recv_timeout(Duration::from_secs(2))
        .expect("수신")
        .expect("emit")
}

fn main() -> roomrs::Result<()> {
    let db = Db::builder().in_memory().build()?;
    let h = db.run_sync();
    for i in 1..=12 {
        h.log_dao().add(&Log {
            id: 0,
            msg: format!("이벤트 {i:02}"),
        })?;
    }

    // 화면 상태: 페이지 라이브 쿼리 + 총건수 라이브 쿼리
    let page: LiveQuery<Vec<Log>> = h.watch_all(
        "SELECT * FROM logs ORDER BY id LIMIT ?1 OFFSET ?2",
        params![PAGE, 0i64],
    );
    let total: LiveQuery<i64> = h.watch_scalar("SELECT COUNT(*) FROM logs", params![]);

    println!("총 {}건", recv(&total));
    println!(
        "1페이지: {:?}",
        recv(&page).iter().map(|l| &l.msg).collect::<Vec<_>>()
    );

    // 페이지 이동 — 재구독 없이 rebind (명세 §5.6b)
    page.rebind(params![PAGE, PAGE])?;
    println!(
        "2페이지: {:?}",
        recv(&page).iter().map(|l| &l.msg).collect::<Vec<_>>()
    );

    page.rebind(params![PAGE, 2 * PAGE])?;
    println!(
        "3페이지: {:?}",
        recv(&page).iter().map(|l| &l.msg).collect::<Vec<_>>()
    );

    // write 유입 — 보고 있는 페이지·총건수가 자동 emit
    h.log_dao().add(&Log {
        id: 0,
        msg: "이벤트 13 (신규)".into(),
    })?;
    println!(
        "write 후 3페이지: {:?}",
        recv(&page).iter().map(|l| &l.msg).collect::<Vec<_>>()
    );
    println!("write 후 총 {}건", recv(&total));
    Ok(())
}
