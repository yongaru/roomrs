//! 멀티 인스턴스(교차 프로세스) 무효화 (명세 §9.5) — feature `multi-instance`
//!
//! 옵트인 테이블에 버전 태깅 트리거를 설치해 write를 로그 테이블에 기록하고,
//! 폴러 스레드가 로그를 감시해 다른 인스턴스/프로세스의 write를 무효화로 방출한다.
//! 각 로그 행에는 write를 수행한 인스턴스 id(`src`)가 기록된다 — 트리거가
//! 커넥션별 스칼라 함수 `roomrs_src()`를 호출해 채운다. 폴러는 `src != 자기 id`
//! 행만 방출하므로 로컬 write의 이중 통지가 없고, 소비하지 못한 원격 행을
//! 건너뛰는 일도 없다 (H-2 — 구 MAX(seq) 선점 방식의 원격 무효화 소실 제거).
//!
//! 혼합 배포 주의 (M-1): v2 로그 형식(src 컬럼 + roomrs_src() 트리거)은 2.0
//! 이전 인스턴스와 호환되지 않는다 — v2가 트리거를 재생성하면 구버전 인스턴스의
//! 옵트인 테이블 write가 "no such function: roomrs_src"로 실패한다. 같은 DB
//! 파일을 공유하는 프로세스는 전부 함께 업그레이드해야 한다
//! (`enable_multi_instance_invalidation` rustdoc 참조).

use crate::error::Result;
use crate::live::Tracker;
use rusqlite::Connection;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// 로그 테이블 DDL — `src` = 기록 인스턴스 id (H-2)
const LOG_DDL: &str = "CREATE TABLE IF NOT EXISTS \"__roomrs_inv_log\" \
                       (\"seq\" INTEGER PRIMARY KEY AUTOINCREMENT, \
                        \"tbl\" TEXT NOT NULL, \"src\" TEXT NOT NULL)";

/// 로그 정리 여유분 — 다른 인스턴스가 아직 소비하지 않았을 수 있는 최근 행을
/// 남겨두는 개수. 크면 로그가 커지고, 작으면 폴링이 느린 인스턴스가 행을
/// 잃을 수 있다 (L-4 트레이드오프)
const PRUNE_MARGIN: i64 = 1000;

/// 폴러 연속 실패 한도 — 초과 시 폴러를 정지시켜 조용한 무한 실패를 막는다 (L-3)
const MAX_CONSEC_FAILURES: u32 = 20;

/// poison 복구 락 — 폴러/종료 경로는 panic 이후에도 동작해야 한다.
/// poison은 panic 직후에만 발생하므로 warn 로그가 스팸이 되지 않는다
fn plock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| {
        log::warn!("mutex poisoned in multi-instance state — recovering");
        e.into_inner()
    })
}

/// 인스턴스 식별자 생성 — 프로세스 id + 오픈 시각(ns) + 프로세스 내 카운터.
/// 외부 crate 없이 교차 프로세스/동일 프로세스 다중 오픈을 구분하기에 충분하다 (H-2)
pub(crate) fn generate_instance_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{:x}-{}",
        std::process::id(),
        nanos,
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// MI 상태 — DatabaseInner 소유
pub(crate) struct MiState {
    /// 폴러 종료 플래그 — Condvar로 즉시 깨운다 (M-5)
    stop: Mutex<bool>,
    cv: Condvar,
    /// 폴러 스레드 join 핸들 — DB drop 시 join으로 커넥션 잔류 방지 (M-5)
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 옵트인 테이블 목록 — 미옵트인 구독 경고용
    pub(crate) tables: HashSet<String>,
}

impl MiState {
    /// 종료 신호 — 폴링 대기 중이어도 즉시 깨어난다 (M-5)
    pub(crate) fn shutdown(&self) {
        *plock(&self.stop) = true;
        self.cv.notify_all();
    }

    /// 폴러 스레드 join — 종료 신호 후 호출 (M-5).
    /// 폴러 스레드 자신에서 호출되면 self-join 교착이므로 분리한다 (H-3)
    pub(crate) fn join_poller(&self) {
        let handle = plock(&self.join).take();
        if let Some(h) = handle {
            if h.thread().id() == std::thread::current().id() {
                // 폴러 스레드 위에서의 종료 경로 — join 생략, 분리 (H-3)
                log::warn!("MI poller shutdown on its own thread — detaching instead of joining");
            } else {
                let _ = h.join();
            }
        }
    }
}

/// 트리거 이름 — 버전 태깅 (명세 §9.5)
fn trigger_names(version: u32, table: &str) -> [(String, &'static str); 3] {
    [
        (format!("__roomrs_inv_v{version}_{table}_i"), "INSERT"),
        (format!("__roomrs_inv_v{version}_{table}_u"), "UPDATE"),
        (format!("__roomrs_inv_v{version}_{table}_d"), "DELETE"),
    ]
}

/// 구형(src 없는) 로그 스키마 감지 → 테이블·트리거 전부 제거해 재설치 유도 (H-2 스키마 범프)
fn migrate_log_shape(conn: &Connection) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='__roomrs_inv_log'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(());
    }
    if log_has_src(conn)? {
        return Ok(());
    }
    // 구형 — 트리거 본문도 구형(INSERT (tbl))이므로 전부 제거 후 재생성
    log::warn!("old multi-instance log schema detected — dropping and recreating");
    let old: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE '__roomrs_inv_%'",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for name in old {
        conn.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {}",
            crate::database::escape_ident(&name)
        ))?;
    }
    // IF EXISTS — 같은 트랜잭션 밖 경쟁자가 이미 지웠어도 실패하지 않는다 (M-1)
    conn.execute_batch("DROP TABLE IF EXISTS \"__roomrs_inv_log\"")?;
    Ok(())
}

/// 로그 테이블에 src 컬럼이 있는지 (신형 스키마 여부)
fn log_has_src(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('__roomrs_inv_log') WHERE name='src'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 로그 테이블 + 옵트인 테이블 트리거 설치, 구버전 트리거 정리 (명세 §9.5).
/// 두 프로세스가 동시에 구형 스키마를 감지해 drop/재생성하는 경합을 막기 위해
/// 전 과정을 단일 IMMEDIATE 트랜잭션으로 감싼다 (M-1)
pub(crate) fn install(conn: &Connection, version: u32, tables: &HashSet<String>) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match install_inner(conn, version, tables) {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            // 롤백 실패는 원인 에러를 대체하지 않는다 — 로그만 남긴다
            if let Err(re) = conn.execute_batch("ROLLBACK") {
                log::error!("multi-instance install rollback failed: {re}");
            }
            Err(e)
        }
    }
}

/// install 본문 — 호출자(install)가 트랜잭션을 관리한다.
/// 트리거 DDL의 식별자/리터럴은 이스케이프한다 (M-8)
fn install_inner(conn: &Connection, version: u32, tables: &HashSet<String>) -> Result<()> {
    migrate_log_shape(conn)?;
    conn.execute_batch(LOG_DDL)?;

    // 구버전/잔존 roomrs 트리거 정리 — 스키마 스큐 방지
    let stale: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE '__roomrs_inv_%'",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok())
            .filter(|name| !is_current_trigger(name, version, tables))
            .collect()
    };
    for name in stale {
        conn.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {}",
            crate::database::escape_ident(&name)
        ))?;
    }

    // 현재 버전 트리거 설치 — src는 커넥션별 함수 roomrs_src()가 채운다 (H-2)
    for table in tables {
        for (name, event) in trigger_names(version, table) {
            conn.execute_batch(&format!(
                "CREATE TRIGGER IF NOT EXISTS {} AFTER {event} ON {} \
                 BEGIN INSERT INTO \"__roomrs_inv_log\" (tbl, src) \
                 VALUES ({}, roomrs_src()); END",
                crate::database::escape_ident(&name),
                crate::database::escape_ident(table),
                crate::database::escape_literal(table),
            ))?;
        }
    }
    Ok(())
}

/// 이름이 현재 버전·옵트인 목록의 트리거인지
fn is_current_trigger(name: &str, version: u32, tables: &HashSet<String>) -> bool {
    tables
        .iter()
        .any(|t| trigger_names(version, t).iter().any(|(n, _)| n == name))
}

/// Validate 정책 — 로그 스키마·트리거 존재 검사 (명세 §9.5: 외부 도구의 drop/recreate 감지)
pub(crate) fn validate(conn: &Connection, version: u32, tables: &HashSet<String>) -> Result<()> {
    // 로그 테이블 부재/구형 = 에러 (H-2 스키마 범프)
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='__roomrs_inv_log'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 || !log_has_src(conn)? {
        return Err(crate::error::Error::Migration(
            "멀티 인스턴스 로그 테이블이 없거나 구형입니다 — Auto 정책으로 재설치가 필요합니다"
                .into(),
        ));
    }
    for table in tables {
        for (name, _) in trigger_names(version, table) {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name = ?1",
                [&name],
                |r| r.get(0),
            )?;
            if n == 0 {
                return Err(crate::error::Error::Migration(format!(
                    "멀티 인스턴스 트리거 소실: {name} — 외부 도구가 테이블을 재생성했을 수 있습니다"
                )));
            }
        }
    }
    Ok(())
}

/// 폴러 스레드 기동 — 로그 감시 → 트래커 무효화 (명세 §9.5 폴링 경로).
/// `my_src` = 이 인스턴스 id — 자기 write 행은 방출하지 않는다 (H-2).
/// 스레드 생성 실패는 panic 대신 에러로 전파한다 (L-6)
pub(crate) fn start_poller(
    conn: Connection,
    tracker: Arc<Tracker>,
    interval: Duration,
    tables: HashSet<String>,
    my_src: String,
) -> Result<Arc<MiState>> {
    // 시작 시점 seq — 과거 로그 재방출 방지
    let initial: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM \"__roomrs_inv_log\"",
        [],
        |r| r.get(0),
    )?;

    let state = Arc::new(MiState {
        stop: Mutex::new(false),
        cv: Condvar::new(),
        join: Mutex::new(None),
        tables,
    });

    let st = Arc::clone(&state);
    let handle = std::thread::Builder::new()
        .name("roomrs-mi-poller".into())
        .spawn(move || {
            let mut last_seen = initial;
            let mut prune_tick = 0u32;
            let mut fails = 0u32;
            loop {
                // 폴링 간격 대기 — 종료 신호에 즉시 깨어난다 (M-5).
                // 연속 실패 시 대기를 늘린다 (L-3 백오프)
                let wait = interval * (1 + fails.min(5));
                {
                    let g = plock(&st.stop);
                    let (g, _) = st
                        .cv
                        .wait_timeout_while(g, wait, |stop| !*stop)
                        .unwrap_or_else(PoisonError::into_inner);
                    if *g {
                        return;
                    }
                }
                // 새 로그 수집 — src 무관 전부 읽고 last_seen을 전진시킨다 (H-2:
                // 자기 행을 건너뛰기 위해 원격 행까지 건너뛰던 MAX(seq) 선점 제거)
                let fetched: std::result::Result<Vec<(i64, String, String)>, rusqlite::Error> =
                    (|| {
                        let mut stmt = conn.prepare_cached(
                            "SELECT seq, tbl, src FROM \"__roomrs_inv_log\" WHERE seq > ?1",
                        )?;
                        let mapped =
                            stmt.query_map([last_seen], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                        mapped.collect()
                    })();
                let rows = match fetched {
                    Ok(v) => {
                        fails = 0;
                        v
                    }
                    Err(_) => {
                        // 준비/조회 실패 — 백오프 후 재시도, 연속 실패 한도 초과 시
                        // 폴러 정지 (L-3: 조용한 무한 실패 방지)
                        fails += 1;
                        if fails >= MAX_CONSEC_FAILURES {
                            log::error!(
                                "MI poller stopped after {MAX_CONSEC_FAILURES} consecutive failures — \
                                 cross-process invalidation is no longer delivered"
                            );
                            return;
                        }
                        continue;
                    }
                };
                if rows.is_empty() {
                    continue;
                }
                let first_seq = rows.iter().map(|(s, _, _)| *s).min().unwrap_or(last_seen);
                last_seen = rows.iter().map(|(s, _, _)| *s).max().unwrap_or(last_seen);
                log::debug!(
                    "MI poller consumed seq {first_seq}..={last_seen} ({} rows)",
                    rows.len()
                );
                // 다른 인스턴스의 write만 방출 — 로컬 write는 문장 기반 경로가 처리[B-2]
                let remote: HashSet<String> = rows
                    .into_iter()
                    .filter(|(_, _, src)| *src != my_src)
                    .map(|(_, t, _)| t)
                    .collect();
                if !remote.is_empty() {
                    tracker.invalidate(Some(remote));
                }
                // 주기 정리 — 오래된 로그 삭제 (다른 인스턴스 소비분 여유 PRUNE_MARGIN)
                prune_tick += 1;
                if prune_tick % 40 == 0 {
                    let _ = conn.execute(
                        "DELETE FROM \"__roomrs_inv_log\" WHERE seq <= ?1 - ?2",
                        rusqlite::params![last_seen, PRUNE_MARGIN],
                    );
                }
            }
        })
        .map_err(|e| crate::error::Error::Internal(format!("MI 폴러 스레드 생성 실패: {e}")))?;
    *plock(&state.join) = Some(handle);
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::functions::FunctionFlags;

    /// 테스트용 인메모리 커넥션 — roomrs_src() 등록 포함
    fn test_conn(src: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let id = src.to_string();
        conn.create_scalar_function(
            "roomrs_src",
            0,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            move |_| Ok(id.clone()),
        )
        .unwrap();
        conn
    }

    /// 따옴표 포함 테이블명 — 트리거 DDL 식별자/리터럴 이스케이프 검증 (M-8)
    #[test]
    fn install_escapes_quoted_table_name() {
        let conn = test_conn("me");
        conn.execute_batch("CREATE TABLE \"we\"\"ird\" (id INTEGER PRIMARY KEY)")
            .unwrap();
        let tables: HashSet<String> = HashSet::from(["we\"ird".to_string()]);
        install(&conn, 1, &tables).unwrap();

        conn.execute("INSERT INTO \"we\"\"ird\" (id) VALUES (1)", [])
            .unwrap();
        let (tbl, src): (String, String) = conn
            .query_row("SELECT tbl, src FROM \"__roomrs_inv_log\"", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(tbl, "we\"ird", "리터럴 이스케이프");
        assert_eq!(src, "me", "roomrs_src() 기록");
    }

    /// 구형(src 없는) 로그 테이블 → install이 재생성 (H-2 스키마 범프)
    #[test]
    fn install_migrates_old_log_shape() {
        let conn = test_conn("me");
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY); \
             CREATE TABLE \"__roomrs_inv_log\" \
             (\"seq\" INTEGER PRIMARY KEY AUTOINCREMENT, \"tbl\" TEXT NOT NULL); \
             CREATE TRIGGER \"__roomrs_inv_v1_t_i\" AFTER INSERT ON t \
             BEGIN INSERT INTO \"__roomrs_inv_log\" (tbl) VALUES ('t'); END",
        )
        .unwrap();

        let tables: HashSet<String> = HashSet::from(["t".to_string()]);
        install(&conn, 1, &tables).unwrap();
        assert!(log_has_src(&conn).unwrap(), "src 컬럼 추가됨");

        // 신형 트리거가 src를 기록한다
        conn.execute("INSERT INTO t (id) VALUES (1)", []).unwrap();
        let src: String = conn
            .query_row("SELECT src FROM \"__roomrs_inv_log\"", [], |r| r.get(0))
            .unwrap();
        assert_eq!(src, "me");
    }

    /// 초기 seq 조회 실패는 0으로 진행하지 않고 폴러 시작을 거부한다.
    #[test]
    fn poller_rejects_initial_sequence_read_failure() {
        let (tracker, notifier) = Tracker::start(Connection::open_in_memory().unwrap()).unwrap();
        let result = start_poller(
            Connection::open_in_memory().unwrap(),
            Arc::clone(&tracker),
            Duration::from_millis(1),
            HashSet::new(),
            "me".into(),
        );
        assert!(result.is_err());
        tracker.shutdown();
        notifier.join().unwrap();
    }
}
