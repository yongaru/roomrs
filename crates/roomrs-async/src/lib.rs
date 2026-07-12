//! roomrs-async — runtime-agnostic async facade (명세 §2.4).
//!
//! Reads and writes are offloaded to a blocking worker pool (or
//! `tokio::task::spawn_blocking` with the `tokio` feature); completion is
//! signalled through a runtime-neutral oneshot channel, so the returned
//! futures can be awaited on any executor (tokio, smol, `futures::executor`).
//! The pool is selected when the returned future is **first polled**: with
//! the `tokio` feature, a job polled outside a tokio runtime falls back to
//! the self-managed pool.
//!
//! Internal crate — use the `roomrs` facade instead.
#![deny(unsafe_code)]

use roomrs_core::rusqlite::types::FromSql;
use roomrs_core::{Database, DatabaseInner, Error, FromRow, Params, Result, SyncHandle};
use std::sync::Arc;

pub use roomrs_core::rusqlite;

// ─────────────────────── 워커 풀 ───────────────────────

/// 블로킹 작업 오프로드 — tokio 런타임 안이면 spawn_blocking, 아니면 자체 풀
mod offload {
    /// 작업 타입
    pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

    /// 자체 워커 풀 — `tokio` feature에서도 런타임 밖 폴백용으로 항상 컴파일 (H-6)
    mod pool {
        use super::Job;
        use std::sync::LazyLock;
        use std::sync::mpsc::{Sender, channel};

        /// 워커 수 상한 — 환경변수 폭주로 인한 스레드 폭탄·초기화 지연 방지 (M-7)
        const MAX_WORKERS: usize = 1024;

        /// 워커 수 결정(순수 함수) — env 값 우선(0·비숫자는 무시), 1024 초과는 클램프,
        /// 기본 max(4, parallelism). 단위 테스트 대상 (M-7)
        fn effective_worker_count(env_val: Option<&str>, parallelism: usize) -> usize {
            if let Some(v) = env_val {
                if let Ok(n) = v.trim().parse::<usize>() {
                    if n > MAX_WORKERS {
                        // 상한 초과 — 클램프하고 경고
                        log::warn!(
                            "ROOMRS_ASYNC_WORKERS={n} exceeds the cap; \
                             clamped to {MAX_WORKERS}"
                        );
                        return MAX_WORKERS;
                    }
                    if n > 0 {
                        return n;
                    }
                }
            }
            parallelism.max(4)
        }

        /// 워커 수 결정 — 환경변수 `ROOMRS_ASYNC_WORKERS` 우선(0·비숫자는 무시),
        /// 기본 max(4, 코어 수), 상한 1024 (L-10, M-7)
        fn worker_count() -> usize {
            let parallelism = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1);
            effective_worker_count(
                std::env::var("ROOMRS_ASYNC_WORKERS").ok().as_deref(),
                parallelism,
            )
        }

        /// 전역 워커 풀 — 프로세스 수명, 프로세스 내 모든 Database 인스턴스 공유 (명세 §2.4)
        static POOL: LazyLock<Sender<Job>> = LazyLock::new(|| {
            let (tx, rx) = channel::<Job>();
            let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
            for i in 0..worker_count() {
                let rx = std::sync::Arc::clone(&rx);
                // 스레드 생성 실패는 무시 — 전부 실패하면 수신단이 닫혀
                // send 실패 → oneshot 취소 → Error::Internal 로 감지된다
                let _ = std::thread::Builder::new()
                    .name(format!("roomrs-worker-{i}"))
                    .spawn(move || {
                        loop {
                            // 락 안에서 recv — 경합은 작업 분배 시점뿐이라 병목 아님
                            let job = {
                                rx.lock()
                                    .expect(
                                        "논리적 불가능: 락 구간은 recv뿐이고 사용자 코드는 \
                                         catch_unwind로 격리되어 poisoned 될 수 없음",
                                    )
                                    .recv()
                            };
                            match job {
                                // 사용자 클로저 패닉 격리 — 워커는 죽지 않고 다음 작업 계속 (H-5).
                                // 패닉한 job은 oneshot 송신단을 drop → 수신측이 Internal로 매핑
                                Ok(job) => {
                                    if let Err(payload) =
                                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                                    {
                                        // 패닉 메시지 추출 — &str/String 페이로드만,
                                        // 그 외 타입은 대체 문구 (L-9)
                                        let msg = payload
                                            .downcast_ref::<&str>()
                                            .map(|s| (*s).to_owned())
                                            .or_else(|| payload.downcast_ref::<String>().cloned())
                                            .unwrap_or_else(|| {
                                                "<non-string panic payload>".to_owned()
                                            });
                                        // 로거 백엔드가 패닉해도 워커가 죽지 않도록
                                        // warn 호출 자체도 격리 (정보-1)
                                        let _ = std::panic::catch_unwind(move || {
                                            log::warn!(
                                                "async job panicked — isolated, \
                                                 worker continues: {msg}"
                                            );
                                        });
                                    }
                                }
                                Err(_) => break, // 송신단 소멸 = 프로세스 종료 중
                            }
                        }
                    });
            }
            tx
        });

        /// 작업 제출 — 실패 시 job drop → oneshot 취소로 호출측 Err 반환(패닉 금지, H-5)
        pub(crate) fn spawn(job: Job) {
            let _ = POOL.send(job);
        }

        /// effective_worker_count 단위 테스트 (M-7)
        #[cfg(test)]
        mod tests {
            use super::{MAX_WORKERS, effective_worker_count};

            // env 미설정 — 기본 max(4, parallelism)
            #[test]
            fn default_is_max_of_4_and_parallelism() {
                assert_eq!(effective_worker_count(None, 1), 4);
                assert_eq!(effective_worker_count(None, 4), 4);
                assert_eq!(effective_worker_count(None, 16), 16);
            }

            // 0·비숫자·음수 값 무시 — 기본값 사용
            #[test]
            fn zero_and_invalid_are_ignored() {
                assert_eq!(effective_worker_count(Some("0"), 8), 8);
                assert_eq!(effective_worker_count(Some("abc"), 8), 8);
                assert_eq!(effective_worker_count(Some(""), 8), 8);
                assert_eq!(effective_worker_count(Some("-3"), 8), 8);
            }

            // 유효 값 채택 — 앞뒤 공백 트림 포함
            #[test]
            fn valid_value_is_used() {
                assert_eq!(effective_worker_count(Some("2"), 8), 2);
                assert_eq!(effective_worker_count(Some(" 32 "), 8), 32);
            }

            // 1024 초과 클램프 — 스레드 폭탄 방지 (M-7)
            #[test]
            fn huge_value_is_clamped_to_1024() {
                assert_eq!(effective_worker_count(Some("1024"), 8), 1024);
                assert_eq!(effective_worker_count(Some("1025"), 8), MAX_WORKERS);
                assert_eq!(effective_worker_count(Some("999999999"), 8), MAX_WORKERS);
            }
        }
    }

    #[cfg(not(feature = "tokio"))]
    pub(crate) use pool::spawn;

    /// tokio 통합 — 런타임 안이면 spawn_blocking, 밖이면 자체 풀 폴백 (명세 §2.4, H-6).
    /// feature는 가산적 — 의존성이 tokio feature를 켜도 smol/futures 사용자가 깨지면 안 된다
    #[cfg(feature = "tokio")]
    pub(crate) fn spawn(job: Job) {
        match tokio::runtime::Handle::try_current() {
            // spawn_blocking 핸들은 버린다 — 완료 통지는 oneshot이 담당
            Ok(h) => drop(h.spawn_blocking(job)),
            // tokio 런타임 밖 — 자체 풀로 폴백
            Err(_) => pool::spawn(job),
        }
    }
}

/// oneshot 송신단 drop(작업이 결과 없이 소멸) → Internal 에러 생성 —
/// run_on_worker · build_async 공용 매핑 (L-11, H-5)
fn job_lost_error() -> Error {
    Error::Internal("비동기 작업이 결과 없이 종료되었습니다(클로저 패닉 또는 워커 풀 소멸)".into())
}

/// 워커에 클로저를 제출하고 완료를 await — 모든 async 표면의 공통 기반
async fn run_on_worker<R, F>(inner: Arc<DatabaseInner>, f: F) -> Result<R>
where
    R: Send + 'static,
    F: FnOnce(SyncHandle<'_>) -> Result<R> + Send + 'static,
{
    let (tx, rx) = futures_channel::oneshot::channel::<Result<R>>();
    offload::spawn(Box::new(move || {
        let out = f(inner.sync_handle());
        // 수신단 drop(Future 취소) = 결과 폐기 — 에러 아님
        let _ = tx.send(out);
    }));
    // 송신단 drop = 작업이 결과 없이 사라짐(클로저 패닉 또는 워커 풀 소멸) → Internal (H-5)
    rx.await.map_err(|_| job_lost_error())?
}

// ─────────────────────── AsyncHandle ───────────────────────

/// Async handle returned by `db.run_async()`.
///
/// SQL and parameters are owned so submitted jobs satisfy the `'static` bound.
///
/// # Cancellation
///
/// Dropping a future returned by this handle **before it is first polled**
/// means the job is never submitted. Dropping it **after** it has been polled
/// does not cancel the operation: the work (including a transaction commit)
/// still runs to completion on the worker and only the result is discarded.
///
/// # Worker pool
///
/// Without the `tokio` feature — or with it, when the returned future is
/// first polled outside a tokio runtime — jobs run on a self-managed blocking
/// pool that is **global to the process** and shared by every `Database`
/// instance. Long-running jobs on one database can therefore delay jobs for
/// other databases. The pool size defaults to
/// `max(4, available parallelism)` and can be overridden with the
/// `ROOMRS_ASYNC_WORKERS` environment variable (invalid or zero values are
/// ignored; values above 1024 are clamped to 1024).
/// The variable is read once when the self-managed pool is first used; later
/// environment changes do not resize the process-global pool. With `tokio`,
/// this initialization may be deferred until the first fallback submission.
///
/// ## Connection-pool contention
///
/// Every operation exclusively checks out one read/write connection. When all
/// connections for a database are busy, waiting jobs can occupy every worker
/// in the self-managed pool and delay jobs for other databases. Configure
/// `DatabaseBuilder::queue_timeout` to bound checkout waits, keep transactions
/// short, and/or increase `ROOMRS_ASYNC_WORKERS`. SQLite lock contention is
/// governed separately by the configured `busy_timeout`. The tokio
/// `spawn_blocking` path is less susceptible to worker starvation because its
/// blocking pool can grow independently.
#[derive(Clone)]
pub struct AsyncHandle {
    inner: Arc<DatabaseInner>,
}

impl AsyncHandle {
    /// Constructs a handle from a database.
    ///
    /// This method is intended for code generated by `#[database]`.
    #[doc(hidden)]
    pub fn from_database(db: &Database) -> Self {
        Self {
            inner: db.inner_arc(),
        }
    }

    /// Runs an arbitrary synchronous operation on a worker.
    ///
    /// This method is intended for generated DAO code. Its cancellation
    /// semantics match the [`AsyncHandle`] cancellation section: dropping the
    /// future after its first poll does not cancel the operation.
    #[doc(hidden)]
    pub fn run<R, F>(&self, f: F) -> impl Future<Output = Result<R>> + Send + use<R, F>
    where
        R: Send + 'static,
        F: FnOnce(SyncHandle<'_>) -> Result<R> + Send + 'static,
    {
        run_on_worker(Arc::clone(&self.inner), f)
    }

    /// Executes a statement and returns the affected row count.
    ///
    /// Dropping the returned future after it has been polled does not cancel
    /// the write; it still runs on the worker and only the result is
    /// discarded (see the [`AsyncHandle`] cancellation notes).
    pub fn execute<S: Into<String>, P>(
        &self,
        sql: S,
        params: P,
    ) -> impl Future<Output = Result<u64>> + Send + use<S, P>
    where
        P: Params + Send + 'static,
    {
        let sql = sql.into();
        self.run(move |h| h.execute(&sql, params))
    }

    /// Queries exactly one row, returning `Error::NotFound` when no row exists.
    pub fn query_one<S: Into<String>, T, P>(
        &self,
        sql: S,
        params: P,
    ) -> impl Future<Output = Result<T>> + Send + use<S, T, P>
    where
        T: FromRow + Send + 'static,
        P: Params + Send + 'static,
    {
        let sql = sql.into();
        self.run(move |h| h.query_one(&sql, params))
    }

    /// Queries zero or one row.
    pub fn query_optional<S: Into<String>, T, P>(
        &self,
        sql: S,
        params: P,
    ) -> impl Future<Output = Result<Option<T>>> + Send + use<S, T, P>
    where
        T: FromRow + Send + 'static,
        P: Params + Send + 'static,
    {
        let sql = sql.into();
        self.run(move |h| h.query_optional(&sql, params))
    }

    /// Queries one scalar value.
    pub fn query_scalar<S: Into<String>, T, P>(
        &self,
        sql: S,
        params: P,
    ) -> impl Future<Output = Result<T>> + Send + use<S, T, P>
    where
        T: FromSql + Send + 'static,
        P: Params + Send + 'static,
    {
        let sql = sql.into();
        self.run(move |h| h.query_scalar(&sql, params))
    }

    /// Queries zero or more rows.
    pub fn query_all<S: Into<String>, T, P>(
        &self,
        sql: S,
        params: P,
    ) -> impl Future<Output = Result<Vec<T>>> + Send + use<S, T, P>
    where
        T: FromRow + Send + 'static,
        P: Params + Send + 'static,
    {
        let sql = sql.into();
        self.run(move |h| h.query_all(&sql, params))
    }

    /// Runs a transaction with a synchronous closure on a worker.
    ///
    /// The closure keeps the same checked-out connection from `BEGIN
    /// IMMEDIATE` through commit or rollback. The closure cannot await.
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future **before it is first polled** means the
    /// transaction is never started. Dropping it **after** it has been polled
    /// does not cancel the operation: the transaction still runs to completion
    /// on the worker — including the commit (or rollback) — and only the
    /// result is discarded. The transaction always terminates with a commit or
    /// rollback; it is never left open.
    pub fn transaction<R, F>(&self, f: F) -> impl Future<Output = Result<R>> + Send + use<R, F>
    where
        R: Send + 'static,
        F: FnOnce(&mut roomrs_core::Tx<'_>) -> Result<R> + Send + 'static,
    {
        self.run(move |h| h.transaction(f))
    }
}

// ─────────────────────── watch (live) ───────────────────────

/// Live-query methods for [`AsyncHandle`].
///
/// `LiveQuery` is shared with the synchronous API. Prefer non-blocking
/// `into_stream()` consumption. `recv` and `recv_timeout` block the executor
/// thread and should not be called directly inside async tasks. Dropping the
/// last database handle joins the notifier during shutdown.
#[cfg(feature = "live")]
impl AsyncHandle {
    /// Creates a live query that returns zero or more rows.
    pub fn watch_all<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> roomrs_core::LiveQuery<Vec<T>> {
        self.inner.__watch_all_dyn(sql, params)
    }

    /// Creates a live query that returns zero or one row.
    pub fn watch_optional<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> roomrs_core::LiveQuery<Option<T>> {
        self.inner.__watch_optional_dyn(sql, params)
    }

    /// Creates a live query that returns one scalar value.
    pub fn watch_scalar<T: rusqlite::types::FromSql + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> roomrs_core::LiveQuery<T> {
        self.inner.__watch_scalar_dyn(sql, params)
    }
}

/// Implements the watch context used by generated DAO code.
#[cfg(feature = "live")]
impl roomrs_core::WatchContext for AsyncHandle {
    fn ctx_watch_all_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> roomrs_core::LiveQuery<Vec<T>> {
        self.inner.__watch_all_named(sql, params, tables)
    }
    fn ctx_watch_optional_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> roomrs_core::LiveQuery<Option<T>> {
        self.inner.__watch_optional_named(sql, params, tables)
    }
    fn ctx_watch_scalar_named<T: rusqlite::types::FromSql + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> roomrs_core::LiveQuery<T> {
        self.inner.__watch_scalar_named(sql, params, tables)
    }
}

// ─────────────────────── 쿼리빌더 실행 (명세 §5.3 [C-6]) ───────────────────────

/// Provides boxed futures for generated async DAO code.
impl roomrs_core::Execute for &AsyncHandle {
    type Out<R: Send + 'static> =
        std::pin::Pin<Box<dyn Future<Output = Result<R>> + Send + 'static>>;

    fn run_all<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<rusqlite::types::Value>,
    ) -> Self::Out<Vec<T>> {
        Box::pin(self.query_all(sql, roomrs_core::params_from_iter(params)))
    }
    fn run_optional<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<rusqlite::types::Value>,
    ) -> Self::Out<Option<T>> {
        Box::pin(self.query_optional(sql, roomrs_core::params_from_iter(params)))
    }
    fn run_one<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<rusqlite::types::Value>,
    ) -> Self::Out<T> {
        Box::pin(self.query_one(sql, roomrs_core::params_from_iter(params)))
    }
    fn run_scalar(self, sql: String, params: Vec<rusqlite::types::Value>) -> Self::Out<i64> {
        Box::pin(self.query_scalar(sql, roomrs_core::params_from_iter(params)))
    }
    fn fail<R: Send + 'static>(e: Error) -> Self::Out<R> {
        Box::pin(async move { Err(e) })
    }
}

// ─────────────────────── build_async ───────────────────────

/// Asynchronous build extensions for `DatabaseBuilder`.
pub trait BuildAsyncExt<T> {
    /// Opens the database on a blocking worker.
    fn build_async(self) -> impl Future<Output = Result<T>> + Send;
}

impl<T> BuildAsyncExt<T> for roomrs_core::DatabaseBuilder<T>
where
    T: roomrs_core::DatabaseSpec + Send + 'static,
{
    /// Opens the database on a blocking worker.
    async fn build_async(self) -> Result<T> {
        let (tx, rx) = futures_channel::oneshot::channel::<Result<T>>();
        offload::spawn(Box::new(move || {
            let _ = tx.send(self.build());
        }));
        // 송신단 drop = 작업이 결과 없이 사라짐(패닉 또는 워커 풀 소멸) → Internal (H-5, L-11)
        rx.await.map_err(|_| job_lost_error())?
    }
}
