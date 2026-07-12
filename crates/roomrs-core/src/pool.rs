//! 자체 미니 풀 (명세 §10, 결정 로그 1b/11)
//!
//! - pool: N개 read/write 커넥션 체크아웃.

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

// ─────────────────────────── connection pool ───────────────────────────

/// read/write 커넥션 풀 상태
struct ConnectionState {
    idle: VecDeque<Connection>,
    fatal: Option<String>,
    next_ticket: u64,
    now_serving: u64,
    abandoned: HashSet<u64>,
    /// 전체 connection 유지보수 중 신규 checkout 차단.
    maintenance: bool,
    /// checkout을 시작한 thread별 보유 수. guard 이동 후에도 원 owner로 감소.
    owners: HashMap<std::thread::ThreadId, usize>,
}

impl ConnectionState {
    /// 다음 유효 체크아웃 티켓으로 진행한다.
    fn advance(&mut self) {
        self.now_serving += 1;
        while self.abandoned.remove(&self.now_serving) {
            self.now_serving += 1;
        }
    }
}

pub(crate) struct ConnectionPool {
    state: Mutex<ConnectionState>,
    #[cfg(any(feature = "live", test))]
    /// 전체 풀 유지보수 호출을 하나씩 실행한다.
    maintenance_lock: Mutex<()>,
    cv: Condvar,
    #[cfg(any(feature = "live", test))]
    total: usize,
    read_uncommitted: bool,
    preserve_on_reopen: bool,
    queue_timeout: Option<Duration>,
    reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync>,
    #[cfg(test)]
    pub(crate) force_restore_failure: AtomicBool,
    #[cfg(test)]
    force_log_panic: AtomicBool,
}

impl ConnectionPool {
    /// 격리된 커넥션을 1회 재오픈하고 실패 시 fatal로 전환한다.
    #[cfg(test)]
    fn reopen_or_fatal(&self, state: &mut ConnectionState, message: String) {
        match (self.reopen)() {
            Ok(conn) => state.idle.push_back(conn),
            Err(e) => state.fatal = Some(format!("{message}; 재오픈 실패: {e}")),
        }
    }

    /// 초기 read/write 커넥션들과 재오픈 수명 정책으로 풀을 구성한다.
    pub(crate) fn new_with_preservation(
        conns: Vec<Connection>,
        read_uncommitted: bool,
        preserve_on_reopen: bool,
        queue_timeout: Option<Duration>,
        reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync>,
    ) -> Self {
        #[cfg(any(feature = "live", test))]
        let total = conns.len();
        Self {
            state: Mutex::new(ConnectionState {
                idle: conns.into(),
                fatal: None,
                next_ticket: 0,
                now_serving: 0,
                abandoned: HashSet::new(),
                maintenance: false,
                owners: HashMap::new(),
            }),
            #[cfg(any(feature = "live", test))]
            maintenance_lock: Mutex::new(()),
            cv: Condvar::new(),
            #[cfg(any(feature = "live", test))]
            total,
            read_uncommitted,
            preserve_on_reopen,
            queue_timeout,
            reopen,
            #[cfg(test)]
            force_restore_failure: AtomicBool::new(false),
            #[cfg(test)]
            force_log_panic: AtomicBool::new(false),
        }
    }

    /// 파일 DB와 같은 drop-before-open 정책의 테스트 풀을 구성한다.
    #[cfg(test)]
    fn new(
        conns: Vec<Connection>,
        read_uncommitted: bool,
        queue_timeout: Option<Duration>,
        reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync>,
    ) -> Self {
        Self::new_with_preservation(conns, read_uncommitted, false, queue_timeout, reopen)
    }

    /// 커넥션 체크아웃 — 전부 사용 중이면 반납까지 블로킹
    pub(crate) fn acquire(&self) -> Result<ConnectionGuard<'_>> {
        let mut state: MutexGuard<'_, ConnectionState> =
            self.state.lock().expect("connection pool 락 poisoned");
        let owner = std::thread::current().id();
        if state.maintenance && state.owners.get(&owner).copied().unwrap_or(0) != 0 {
            return Err(Error::Internal(
                "커넥션을 보유한 흐름에서 풀 유지보수 중 재진입할 수 없습니다".into(),
            ));
        }
        let my_ticket = state.next_ticket;
        state.next_ticket += 1;
        let deadline = self
            .queue_timeout
            .map(|timeout| std::time::Instant::now() + timeout);
        loop {
            if let Some(message) = state.fatal.clone() {
                return Err(Error::Internal(message));
            }
            if state.maintenance && state.owners.get(&owner).copied().unwrap_or(0) != 0 {
                if state.now_serving == my_ticket {
                    state.advance();
                } else {
                    state.abandoned.insert(my_ticket);
                }
                drop(state);
                self.cv.notify_all();
                return Err(Error::Internal(
                    "커넥션을 보유한 흐름에서 풀 유지보수 중 재진입할 수 없습니다".into(),
                ));
            }
            if !state.maintenance && state.now_serving == my_ticket {
                if let Some(conn) = state.idle.pop_front() {
                    *state.owners.entry(owner).or_insert(0) += 1;
                    state.advance();
                    self.cv.notify_all();
                    return Ok(ConnectionGuard {
                        pool: self,
                        conn: Some(conn),
                        owner,
                        _not_send: std::marker::PhantomData,
                    });
                }
            }
            state = match deadline {
                None => self.cv.wait(state).expect("connection pool 락 poisoned"),
                Some(deadline) => {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        if state.now_serving == my_ticket {
                            state.advance();
                        } else {
                            state.abandoned.insert(my_ticket);
                        }
                        drop(state);
                        self.cv.notify_all();
                        return Err(Error::QueueTimeout(
                            self.queue_timeout.expect("deadline 존재"),
                        ));
                    }
                    let (state, _) = self
                        .cv
                        .wait_timeout(state, deadline - now)
                        .expect("connection pool 락 poisoned");
                    state
                }
            };
        }
    }

    /// 빌드 중 idle 커넥션 전부에 초기화 함수를 적용한다.
    pub(crate) fn for_each_idle(&self, mut f: impl FnMut(&Connection) -> Result<()>) -> Result<()> {
        let state = self.state.lock().expect("connection pool 락 poisoned");
        if let Some(message) = &state.fatal {
            return Err(Error::Internal(message.clone()));
        }
        for conn in state.idle.iter() {
            f(conn)?;
        }
        Ok(())
    }

    /// 모든 checkout 반납을 기다린 뒤 전체 connection에 함수를 적용한다.
    #[cfg(any(feature = "live", test))]
    pub(crate) fn for_each_connection(
        &self,
        mut f: impl FnMut(&Connection) -> Result<()>,
    ) -> Result<()> {
        let caller = std::thread::current().id();
        {
            let state = self.state.lock().expect("connection pool 락 poisoned");
            if state.owners.get(&caller).copied().unwrap_or(0) != 0 {
                return Err(Error::Internal(
                    "커넥션을 보유한 흐름에서 전체 풀 유지보수를 실행할 수 없습니다".into(),
                ));
            }
        }
        let _maintenance = self
            .maintenance_lock
            .lock()
            .expect("connection pool maintenance 락 poisoned");
        let mut state = self.state.lock().expect("connection pool 락 poisoned");
        if let Some(message) = &state.fatal {
            return Err(Error::Internal(message.clone()));
        }
        if state.owners.get(&caller).copied().unwrap_or(0) != 0 {
            return Err(Error::Internal(
                "커넥션을 보유한 흐름에서 전체 풀 유지보수를 실행할 수 없습니다".into(),
            ));
        }
        state.maintenance = true;
        self.cv.notify_all();
        let deadline = self
            .queue_timeout
            .map(|timeout| std::time::Instant::now() + timeout);
        while state.idle.len() != self.total {
            if let Some(message) = state.fatal.clone() {
                state.maintenance = false;
                drop(state);
                self.cv.notify_all();
                return Err(Error::Internal(message));
            }
            state = match deadline {
                None => self.cv.wait(state).expect("connection pool 락 poisoned"),
                Some(deadline) => {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        state.maintenance = false;
                        drop(state);
                        self.cv.notify_all();
                        return Err(Error::QueueTimeout(
                            self.queue_timeout.expect("deadline 존재"),
                        ));
                    }
                    self.cv
                        .wait_timeout(state, deadline - now)
                        .expect("connection pool 락 poisoned")
                        .0
                }
            };
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.idle.iter().try_for_each(&mut f)
        }));
        state.maintenance = false;
        drop(state);
        self.cv.notify_all();
        match result {
            Ok(result) => result,
            Err(payload) => {
                // maintenance mutex가 poison되지 않도록 guard 해제 뒤 panic을 재개한다.
                drop(_maintenance);
                std::panic::resume_unwind(payload)
            }
        }
    }
}

/// read/write 커넥션 RAII 가드
pub(crate) struct ConnectionGuard<'p> {
    pool: &'p ConnectionPool,
    conn: Option<Connection>,
    owner: std::thread::ThreadId,
    /// checkout owner thread 밖으로 guard가 이동하지 않게 한다.
    _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ConnectionGuard<'_> {
    /// 커넥션 참조
    pub(crate) fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("drop 전에는 항상 Some")
    }
}

impl Drop for ConnectionGuard<'_> {
    /// 트랜잭션 상태 복구 후 반납 + 대기자 깨우기
    fn drop(&mut self) {
        let mut restore_error = None;
        if let Some(conn) = self.conn.as_ref() {
            if !conn.is_autocommit() {
                if let Err(e) = conn.execute_batch("ROLLBACK") {
                    restore_error = Some(format!("connection rollback 실패: {e}"));
                } else if !conn.is_autocommit() {
                    restore_error = Some("connection rollback 후 트랜잭션 잔류".into());
                }
            }
            if restore_error.is_none() {
                if let Err(e) = conn.pragma_update(None, "query_only", "OFF") {
                    restore_error = Some(format!("connection query_only 복구 실패: {e}"));
                }
            }
            if restore_error.is_none() {
                if let Err(e) = conn.pragma_update(None, "foreign_keys", "ON") {
                    restore_error = Some(format!("connection foreign_keys 복구 실패: {e}"));
                }
            }
            if restore_error.is_none() && self.pool.read_uncommitted {
                if let Err(e) = conn.pragma_update(None, "read_uncommitted", "ON") {
                    restore_error = Some(format!("connection read_uncommitted 복구 실패: {e}"));
                }
            }
            if restore_error.is_none() {
                if let Err(e) = conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0)) {
                    restore_error = Some(format!("connection health check 실패: {e}"));
                }
            }
        }
        #[cfg(test)]
        if self
            .pool
            .force_restore_failure
            .swap(false, Ordering::AcqRel)
        {
            restore_error = Some("테스트 강제 복구 실패".into());
        }
        let replacement = restore_error.as_ref().map(|_| {
            if self.pool.preserve_on_reopen {
                // 단일 shared-cache 인메모리 DB는 마지막 연결이 닫히면 사라진다.
                // 대체 연결을 먼저 열어 DB 수명을 유지한다.
                let replacement = (self.pool.reopen)();
                drop(self.conn.take());
                replacement
            } else {
                // 파일 DB는 격리 연결의 lock과 connection-local 상태를 먼저 폐기한다.
                drop(self.conn.take());
                (self.pool.reopen)()
            }
        });
        let mut state = self.pool.state.lock().expect("connection pool 락 poisoned");
        if let Some(count) = state.owners.get_mut(&self.owner) {
            *count -= 1;
            if *count == 0 {
                state.owners.remove(&self.owner);
            }
        }
        if let Some(message) = restore_error {
            match replacement.expect("복구 실패 시 대체 결과 존재") {
                Ok(conn) => state.idle.push_back(conn),
                Err(e) => state.fatal = Some(format!("{message}; 재오픈 실패: {e}")),
            }
            drop(state);
            self.pool.cv.notify_all();
            #[cfg(test)]
            if self.pool.force_log_panic.swap(false, Ordering::AcqRel) {
                panic!("테스트 logger panic");
            }
            log::error!("pool connection quarantined after restore failure: {message}");
        } else {
            state.idle.push_back(self.conn.take().expect("drop은 1회"));
            drop(state);
            // 티켓 head가 아닌 waiter만 깨우면 head가 잠든 채 교착 가능하다.
            self.pool.cv.notify_all();
        }
    }
}

// ─────────────────────────── pool ───────────────────────────

/// read/write 커넥션 N개 통합 풀
pub(crate) struct Pool {
    pub(crate) connections: ConnectionPool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 테스트용 새 인메모리 connection factory.
    fn factory() -> Arc<dyn Fn() -> Result<Connection> + Send + Sync> {
        Arc::new(|| Ok(Connection::open_in_memory()?))
    }

    /// 풀 fatal 상태는 대기하지 않고 즉시 명시 오류로 반환된다.
    #[test]
    fn connection_fatal_state_returns_error() {
        let connections = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            None,
            factory(),
        );
        connections.state.lock().unwrap().fatal = Some("connection 격리".into());
        assert!(
            matches!(connections.acquire(), Err(Error::Internal(message)) if message == "connection 격리")
        );
    }

    /// 풀 고갈 시 queue_timeout 안에 명시 오류를 반환한다.
    #[test]
    fn connection_checkout_times_out() {
        let connections = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_millis(20)),
            factory(),
        );
        let _held = connections.acquire().unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(connections.acquire(), Err(Error::QueueTimeout(_))));
        assert!(started.elapsed() >= Duration::from_millis(15));
    }

    /// 커넥션 보유 흐름의 전체 풀 유지보수 재진입은 즉시 오류다.
    #[test]
    fn maintenance_reentry_returns_error() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            None,
            factory(),
        );
        let _held = pool.acquire().unwrap();
        assert!(matches!(
            pool.for_each_connection(|_| Ok(())),
            Err(Error::Internal(message)) if message.contains("유지보수")
        ));
    }

    /// 다른 maintenance가 drain 중이어도 guard 보유 호출은 mutex 앞에서 즉시 오류다.
    #[test]
    fn maintenance_owner_does_not_wait_for_other_maintenance() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(5)),
            factory(),
        );
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (run_tx, run_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let owner_pool = &pool;
            scope.spawn(move || {
                let held = owner_pool.acquire().unwrap();
                held_tx.send(()).unwrap();
                run_rx.recv().unwrap();
                let result = owner_pool.for_each_connection(|_| Ok(()));
                result_tx.send(result).unwrap();
                drop(held);
            });
            held_rx.recv().unwrap();
            let maintenance = scope.spawn(|| pool.for_each_connection(|_| Ok(())));
            loop {
                if pool.state.lock().unwrap().maintenance {
                    break;
                }
                std::thread::yield_now();
            }

            run_tx.send(()).unwrap();
            assert!(matches!(
                result_rx.recv_timeout(Duration::from_secs(1)),
                Ok(Err(Error::Internal(message))) if message.contains("유지보수")
            ));
            assert!(matches!(maintenance.join().unwrap(), Ok(())));
        });
    }

    /// checkout 대기 중 maintenance가 시작되어도 보유 흐름은 즉시 오류로 빠진다.
    #[test]
    fn waiting_owner_rechecks_maintenance() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(1)),
            factory(),
        );
        std::thread::scope(|scope| {
            let maintenance_pool = &pool;
            let maintenance = scope.spawn(move || {
                loop {
                    let waiting = maintenance_pool.state.lock().unwrap().next_ticket >= 2;
                    if waiting {
                        break;
                    }
                    std::thread::yield_now();
                }
                maintenance_pool.for_each_connection(|_| Ok(()))
            });

            let held = pool.acquire().unwrap();
            assert!(matches!(
                pool.acquire(),
                Err(Error::Internal(message)) if message.contains("유지보수")
            ));
            drop(held);
            maintenance.join().unwrap().unwrap();
        });
    }

    /// 동시에 요청된 maintenance callback은 겹쳐 실행되지 않는다.
    #[test]
    fn concurrent_maintenance_is_serialized() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(1)),
            factory(),
        );
        let active = std::sync::atomic::AtomicUsize::new(0);
        let max_active = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    pool.for_each_connection(|_| {
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_active.fetch_max(current, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .unwrap();
                });
            }
        });
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    /// maintenance callback panic 뒤에도 상태와 직렬화 락이 복구된다.
    #[test]
    fn maintenance_callback_panic_restores_pool_state() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(1)),
            factory(),
        );
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = pool.for_each_connection(|_| -> Result<()> {
                panic!("테스트 maintenance callback panic")
            });
        }));
        assert!(panic.is_err());
        assert!(!pool.state.lock().unwrap().maintenance);
        assert!(pool.acquire().is_ok());
        assert!(pool.for_each_connection(|_| Ok(())).is_ok());
    }

    /// logger panic 뒤에도 pool mutex는 poison되지 않고 재사용된다.
    #[test]
    fn logger_panic_does_not_poison_pool_mutex() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(1)),
            factory(),
        );
        let guard = pool.acquire().unwrap();
        pool.force_restore_failure.store(true, Ordering::Release);
        pool.force_log_panic.store(true, Ordering::Release);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(guard)));
        assert!(panic.is_err());
        assert!(pool.acquire().is_ok());
    }

    /// 기존 checkout이 반환되지 않으면 maintenance도 queue_timeout으로 끝난다.
    #[test]
    fn maintenance_drain_times_out() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_millis(20)),
            factory(),
        );
        let held = pool.acquire().unwrap();
        std::thread::scope(|scope| {
            let result = scope
                .spawn(|| pool.for_each_connection(|_| Ok(())))
                .join()
                .unwrap();
            assert!(matches!(result, Err(Error::QueueTimeout(_))));
        });
        drop(held);
        assert!(
            pool.acquire().is_ok(),
            "timeout 뒤 maintenance가 해제되어야 함"
        );
    }

    /// maintenance 실행 중 신규 checkout은 완료 전까지 차단된다.
    #[test]
    fn maintenance_blocks_new_checkout() {
        let pool = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(1)),
            factory(),
        );
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let maintenance_pool = &pool;
            scope.spawn(move || {
                maintenance_pool
                    .for_each_connection(|_| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(())
                    })
                    .unwrap();
            });
            entered_rx.recv().unwrap();
            let checkout_pool = &pool;
            scope.spawn(move || {
                let _guard = checkout_pool.acquire().unwrap();
                acquired_tx.send(()).unwrap();
            });
            assert!(acquired_rx.recv_timeout(Duration::from_millis(20)).is_err());
            release_tx.send(()).unwrap();
            acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        });
    }

    /// 풀의 모든 checkout 커넥션은 CUD를 실행할 수 있다.
    #[test]
    fn every_pool_connection_is_writable() {
        let first = Connection::open_in_memory().unwrap();
        first.execute("CREATE TABLE items(id INTEGER)", []).unwrap();
        let connections = ConnectionPool::new(vec![first], false, None, factory());
        let guard = connections.acquire().unwrap();
        guard
            .conn()
            .execute("INSERT INTO items(id) VALUES (1)", [])
            .unwrap();
        let count: i64 = guard
            .conn()
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// 대기 요청은 티켓 발급 순서대로 connection을 획득한다.
    #[test]
    fn connection_checkout_is_fifo() {
        let connections = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            Some(Duration::from_secs(1)),
            factory(),
        );
        let held = connections.acquire().unwrap();
        let order = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for id in 1u64..=3 {
                let connections_ref = &connections;
                let order_ref = &order;
                scope.spawn(move || {
                    let guard = connections_ref.acquire().unwrap();
                    order_ref.lock().unwrap().push(id);
                    drop(guard);
                });
                while connections.state.lock().unwrap().next_ticket < id + 1 {
                    std::thread::yield_now();
                }
            }
            drop(held);
        });
        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
    }

    /// 커넥션 재오픈 실패 시 무한 재시도 없이 fatal 오류로 전환한다.
    #[test]
    fn connection_reopen_failure_becomes_fatal() {
        let reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync> =
            Arc::new(|| Err(Error::Internal("주입 실패".into())));
        let connections = ConnectionPool::new(vec![], false, None, reopen);
        let mut state = connections.state.lock().unwrap();
        connections.reopen_or_fatal(&mut state, "격리".into());
        assert!(state.idle.is_empty());
        assert!(state.fatal.as_deref().unwrap().contains("주입 실패"));
    }

    /// 실제 ConnectionGuard::drop 복구 실패가 factory를 호출하고 용량을 복원한다.
    #[test]
    fn connection_guard_drop_reopens_connection() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&calls);
        let reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync> = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Connection::open_in_memory()?)
        });
        let connections = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            None,
            reopen,
        );
        connections
            .force_restore_failure
            .store(true, Ordering::Release);
        drop(connections.acquire().unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(connections.acquire().is_ok());
    }

    /// 단일 shared-cache 인메모리 연결 교체 중 기존 연결이 DB 수명을 유지한다.
    #[test]
    fn connection_reopen_preserves_single_in_memory_database() {
        let uri = "file:roomrs_pool_restore?mode=memory&cache=shared";
        let first = Connection::open(uri).unwrap();
        first
            .execute_batch("CREATE TABLE items(id INTEGER); INSERT INTO items VALUES (7)")
            .unwrap();
        let reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync> =
            Arc::new(move || Ok(Connection::open(uri)?));
        let connections =
            ConnectionPool::new_with_preservation(vec![first], true, true, None, reopen);
        connections
            .force_restore_failure
            .store(true, Ordering::Release);
        drop(connections.acquire().unwrap());

        let guard = connections.acquire().unwrap();
        let value: i64 = guard
            .conn()
            .query_row("SELECT id FROM items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 7);
    }

    /// 파일 DB 재오픈 정책은 factory 호출 전에 기존 연결을 폐기한다.
    #[test]
    fn non_preserving_reopen_drops_old_connection_first() {
        let marker = Arc::new(());
        let hook_marker = Arc::clone(&marker);
        let first = Connection::open_in_memory().unwrap();
        let _previous = first.update_hook(Some(
            move |_action, _database: &str, _table: &str, _rowid| {
                let _keep_alive = &hook_marker;
            },
        ));
        let weak_marker = Arc::downgrade(&marker);
        let old_alive_during_reopen = Arc::new(AtomicBool::new(true));
        let observed = Arc::clone(&old_alive_during_reopen);
        let reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync> = Arc::new(move || {
            observed.store(weak_marker.strong_count() > 1, Ordering::Release);
            Ok(Connection::open_in_memory()?)
        });
        let connections = ConnectionPool::new(vec![first], false, None, reopen);
        connections
            .force_restore_failure
            .store(true, Ordering::Release);
        drop(connections.acquire().unwrap());

        assert!(!old_alive_during_reopen.load(Ordering::Acquire));
    }

    /// 커넥션 탈출구가 query_only를 켜도 반납 시 OFF로 복구한다.
    #[test]
    fn connection_guard_restores_query_only_off() {
        let connections = ConnectionPool::new(
            vec![Connection::open_in_memory().unwrap()],
            false,
            None,
            factory(),
        );
        {
            let guard = connections.acquire().unwrap();
            guard
                .conn()
                .pragma_update(None, "query_only", "ON")
                .unwrap();
        }
        let guard = connections.acquire().unwrap();
        let value: i64 = guard
            .conn()
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 0);
    }

    /// 커넥션 탈출구가 foreign_keys를 꺼도 반납 시 ON으로 복구한다.
    #[test]
    fn connection_guard_restores_foreign_keys_on() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        let connections = ConnectionPool::new(vec![connection], false, None, factory());
        {
            let guard = connections.acquire().unwrap();
            guard
                .conn()
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
        }
        let guard = connections.acquire().unwrap();
        let value: i64 = guard
            .conn()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
    }
}
