//! 멀티 프로세스 무효화 사용 케이스 (명세 §9.5, feature multi-instance)
//!
//! 같은 실행 파일을 자식 프로세스(writer 역할)로 띄우고,
//! 부모(watcher)는 자식의 write를 폴링 경로로 감지해 emit을 받는다.
//!
//! 실행: cargo run --example multi_process --features multi-instance

use roomrs::{LiveQuery, dao, database, entity};
use std::time::Duration;

#[entity(table = "events", multi_instance)] // 교차 프로세스 추적 옵트인
#[derive(Debug, Clone)]
struct Event {
    #[pk(autoincrement)]
    id: i64,
    tag: String,
}

#[dao]
trait EventDao {
    #[insert]
    fn add(&self, e: &Event) -> roomrs::Result<i64>;

    #[query("SELECT COUNT(*) FROM events")]
    fn watch_count(&self) -> LiveQuery<i64>;
}

#[database(entities(Event), daos(EventDao), version = 1)]
struct Db;

/// MI 활성 오픈 (데모용 짧은 폴링)
fn open(path: &str) -> roomrs::Result<Db> {
    Db::builder()
        .sqlite(path)
        .enable_multi_instance_invalidation(true)
        .mi_poll_interval(Duration::from_millis(100))
        .build()
}

/// 자식 프로세스 — writer 역할: 3건 write 후 종료
fn run_writer(path: &str) -> roomrs::Result<()> {
    let db = open(path)?;
    for i in 1..=3 {
        db.run_sync().event_dao().add(&Event {
            id: 0,
            tag: format!("자식 write {i}"),
        })?;
        println!("  [writer pid={}] {i}건째 기록", std::process::id());
        std::thread::sleep(Duration::from_millis(400));
    }
    Ok(())
}

/// 부모 프로세스 — watcher 역할: 자식 spawn + 교차 프로세스 emit 수신.
/// 반환 = 프로세스 exit code (writer 자식 실패를 그대로 전파, L-21)
fn run_watcher(path: &str) -> roomrs::Result<u8> {
    let db = open(path)?;
    let live = db.run_sync().event_dao().watch_count();
    println!(
        "[watcher pid={}] 초기 개수: {:?}",
        std::process::id(),
        live.recv()?
    );

    // 같은 exe를 writer 모드로 spawn
    let exe = std::env::current_exe().expect("현재 실행 파일 경로");
    let mut child = std::process::Command::new(exe)
        .env("ROOMRS_ROLE", "writer")
        .env("ROOMRS_DB", path)
        .spawn()
        .expect("자식 프로세스 spawn");

    // 자식 write마다 폴러 경유 emit — 3에 도달할 때까지 수신.
    // 어떤 경로로 나가든 자식은 반드시 wait()로 회수한다(좀비 방지).
    loop {
        match live.recv_timeout(Duration::from_secs(5)) {
            Ok(Some(n)) => {
                println!("[watcher] 교차 프로세스 emit 수신: 개수 = {n}");
                if n >= 3 {
                    break;
                }
            }
            Ok(None) => {
                println!("[watcher] 타임아웃 — 수신 중단");
                break;
            }
            Err(e) => {
                println!("[watcher] 수신 에러: {e}");
                break;
            }
        }
    }
    // 자식 exit status 검증 — writer 실패를 성공으로 위장하지 않는다 (L-21).
    // std::process::exit 는 tempdir Drop을 건너뛰므로 코드만 반환하고
    // 종료 처리는 가드가 전부 drop된 뒤 main이 담당한다
    let status = child.wait().expect("자식 wait");
    if !status.success() {
        eprintln!("[watcher] writer 자식 실패: {status}");
        return Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1));
    }
    println!("[watcher] 완료 — 다른 프로세스의 write를 트리거+폴링으로 감지했다");
    Ok(0)
}

/// 역할 분기 실행 — exit code 반환 (가드 drop 이후 종료는 main 몫)
fn run() -> roomrs::Result<u8> {
    match std::env::var("ROOMRS_ROLE").as_deref() {
        Ok("writer") => {
            let path = std::env::var("ROOMRS_DB").expect("ROOMRS_DB 필요");
            run_writer(&path).map(|()| 0)
        }
        _ => {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("mp.db");
            // 데모 한정: 비 UTF-8 임시 경로는 expect 실패로 종료해도 무방 (L-9)
            run_watcher(path.to_str().expect("utf-8 경로"))
            // dir 가드는 여기서 drop — 임시 DB 정리 후 main이 코드 반환
        }
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(0) => std::process::ExitCode::SUCCESS,
        Ok(code) => std::process::ExitCode::from(code),
        Err(e) => {
            eprintln!("에러: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
