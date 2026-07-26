//! 라이브 쿼리 예제 — write마다 자동 재조회 emit (명세 §5.6)
//! roomrs 내부 log 레코드를 tracing으로 수집하는 브리지 시연 포함.
//! 필요 feature: live (기본 on)

use roomrs::{LiveQuery, dao, database, entity};
use std::time::Duration;

mod support;

#[entity(table = "todos")]
#[derive(Debug, Clone)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, t: &Todo) -> roomrs::Result<i64>;

    #[query("SELECT COUNT(*) FROM todos")]
    fn watch_count(&self) -> LiveQuery<i64>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

/// 실행: cargo run --example live_query
fn main() -> roomrs::Result<()> {
    support::init_tracing();

    let db = Db::builder().in_memory().build()?;
    let h = db.run_sync();

    let live = h.todo_dao().watch_count();
    // 구독 콜백 — 노티파이어 스레드에서 호출
    let _guard = live.subscribe(|n| println!("현재 todo 개수: {n}"));

    for i in 0..3 {
        h.todo_dao().add(&Todo { id: 0, title: format!("작업 {i}"), done: false })?;
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}
