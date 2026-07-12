//! 라이브 쿼리 예제 — write마다 자동 재조회 emit (명세 §5.6)
//! roomrs 내부 log 레코드를 tracing으로 수집하는 브리지 시연 포함.
//! 필요 feature: live (기본 on)

use roomrs::{LiveQuery, dao, database, entity};
use std::time::Duration;

/// log → tracing 브리지 초기화 —
/// roomrs는 log 파사드로만 방출한다(구독자 초기화는 소비자 몫).
/// LogTracer가 log 레코드를 tracing 이벤트로 변환하고,
/// fmt 구독자가 debug 레벨 필터로 출력한다.
fn init_tracing() {
    // 1) log → tracing 변환기 설치 (전역 log 로거)
    tracing_log::LogTracer::init().expect("LogTracer 초기화 실패");
    // 2) fmt 구독자를 debug 필터로 설치 —
    //    LogTracer를 직접 설치했으므로 set_global_default 사용
    //    (fmt().init()은 tracing-log 기능으로 LogTracer를 중복 설치하려다 실패한다)
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("tracing 구독자 설치 실패");
}

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
    // 브리지 초기화 — roomrs 내부 debug 로그(오픈·트랜잭션·무효화)가 콘솔에 보인다
    init_tracing();

    let db = Db::builder().in_memory().build()?;
    let h = db.run_sync();

    let live = h.todo_dao().watch_count();
    // 구독 콜백 — 노티파이어 스레드에서 호출
    let _guard = live.subscribe(|n| println!("현재 todo 개수: {n}"));

    for i in 0..3 {
        h.todo_dao().add(&Todo {
            id: 0,
            title: format!("작업 {i}"),
            done: false,
        })?;
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}
