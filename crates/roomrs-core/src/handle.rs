//! 동기 실행 표면 — `SyncHandle` · `Tx` · `SqlContext` (명세 §5.0/§5.5/§5.7/§5.9)

use crate::database::DatabaseInner;
use crate::error::{Error, Result};
use crate::row::FromRow;
use rusqlite::types::FromSql;
use rusqlite::{Connection, Params};
use std::sync::Arc;

/// 커넥션 하나 위에서의 공통 쿼리 구현 — 풀 체크아웃과 tx가 공유
mod on_conn {
    use super::*;

    /// 쓰기 실행 — 영향 행 수 반환
    pub(super) fn execute<P: Params>(conn: &Connection, sql: &str, params: P) -> Result<u64> {
        let n = conn.execute(sql, params)?;
        Ok(n as u64)
    }

    /// INSERT 실행 — 새 rowid 반환 (명세 §12c)
    pub(super) fn insert<P: Params>(conn: &Connection, sql: &str, params: P) -> Result<i64> {
        if conn.execute(sql, params)? == 0 {
            return Err(Error::NotFound);
        }
        Ok(conn.last_insert_rowid())
    }

    /// N건 조회
    pub(super) fn query_all<T: FromRow, P: Params>(
        conn: &Connection,
        sql: &str,
        params: P,
    ) -> Result<Vec<T>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, |r| T::from_row(r))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 0~1건 조회
    pub(super) fn query_optional<T: FromRow, P: Params>(
        conn: &Connection,
        sql: &str,
        params: P,
    ) -> Result<Option<T>> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        match rows.next()? {
            Some(row) => Ok(Some(T::from_row(row)?)),
            None => Ok(None),
        }
    }

    /// 스칼라 1건 조회 — 0건이면 NotFound
    pub(super) fn query_scalar<T: FromSql, P: Params>(
        conn: &Connection,
        sql: &str,
        params: P,
    ) -> Result<T> {
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        match rows.next()? {
            Some(row) => Ok(row.get(0)?),
            None => Err(Error::NotFound),
        }
    }
}

// ─────────────────────── SqlContext ───────────────────────

/// DAO 공용 실행 컨텍스트 (명세 §5.9) —
/// 풀-바운드(`SyncHandle`: 호출마다 통합 풀 체크아웃)와
/// tx-바운드(`Tx`: 트랜잭션 커넥션 고정)를 하나의 trait로 통일한다.
pub trait SqlContext {
    /// 쓰기 실행 — 영향 행 수
    fn ctx_execute<P: Params>(&self, sql: &str, params: P) -> Result<u64>;
    /// INSERT — 새 rowid
    fn ctx_insert<P: Params>(&self, sql: &str, params: P) -> Result<i64>;
    /// N건 조회
    fn ctx_query_all<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Vec<T>>;
    /// 0~1건 조회
    fn ctx_query_optional<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Option<T>>;
    /// 정확히 1건 조회 (0건 = NotFound)
    fn ctx_query_one<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        self.ctx_query_optional(sql, params)?.ok_or(Error::NotFound)
    }
    /// 스칼라 1건 조회
    fn ctx_query_scalar<T: FromSql, P: Params>(&self, sql: &str, params: P) -> Result<T>;

    /// 트랜잭션 실행 (명세 §5.9 한 메커니즘) —
    /// 풀-바운드 = BEGIN/COMMIT, tx-바운드 = SAVEPOINT(중첩)
    fn ctx_transaction<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R>;
}

/// 참조 위임 — `&Tx` 도 컨텍스트로 쓸 수 있게
impl<C: SqlContext> SqlContext for &C {
    fn ctx_execute<P: Params>(&self, sql: &str, params: P) -> Result<u64> {
        (**self).ctx_execute(sql, params)
    }
    fn ctx_insert<P: Params>(&self, sql: &str, params: P) -> Result<i64> {
        (**self).ctx_insert(sql, params)
    }
    fn ctx_query_all<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Vec<T>> {
        (**self).ctx_query_all(sql, params)
    }
    fn ctx_query_optional<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Option<T>> {
        (**self).ctx_query_optional(sql, params)
    }
    fn ctx_query_scalar<T: FromSql, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        (**self).ctx_query_scalar(sql, params)
    }
    fn ctx_transaction<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R> {
        (**self).ctx_transaction(f)
    }
}

// ─────────────────────── SyncHandle ───────────────────────

/// 동기 핸들 — `db.run_sync()` 반환 타입 (명세 §5.0).
/// 모든 작업은 read/write 통합 풀에서 커넥션을 체크아웃한다.
///
/// With the `live` feature, SQL classification is conservative. `PRAGMA`
/// statements, including read-only forms, trigger full invalidation because
/// SQLite state-changing forms cannot be distinguished reliably by the parser.
#[derive(Clone, Copy)]
pub struct SyncHandle<'db> {
    pub(crate) inner: &'db Arc<DatabaseInner>,
}

impl<'db> SyncHandle<'db> {
    /// 쓰기 실행 (명세 §5.7)
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<u64> {
        self.ctx_execute(sql, params)
    }

    /// 정확히 1건 조회 — 0건이면 `Error::NotFound`
    pub fn query_one<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        self.ctx_query_one(sql, params)
    }

    /// 0~1건 조회
    pub fn query_optional<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Option<T>> {
        self.ctx_query_optional(sql, params)
    }

    /// 스칼라 1건 조회
    pub fn query_scalar<T: FromSql, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        self.ctx_query_scalar(sql, params)
    }

    /// N건 조회
    pub fn query_all<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Vec<T>> {
        self.ctx_query_all(sql, params)
    }

    /// 클로저 트랜잭션 (명세 §5.5) — 에러 시 롤백, 성공 시 커밋
    pub fn transaction<R>(&self, f: impl FnOnce(&mut Tx<'_>) -> Result<R>) -> Result<R> {
        let mut tx = self.begin()?;
        match f(&mut tx) {
            Ok(v) => {
                tx.commit()?;
                Ok(v)
            }
            Err(e) => {
                // 롤백 실패는 원인 에러를 대체하지 않는다 (L-1) —
                // 실패 시 rollback()이 Tx를 open 상태로 drop해 롤백을 재시도한다
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    /// RAII 트랜잭션 — drop 시 미커밋이면 롤백 (명세 §5.5)
    pub fn begin(&self) -> Result<Tx<'db>> {
        let guard = self.inner.pool.connections.acquire()?;
        // WAL에서 deferred BEGIN은 read→write 승격 시 SQLITE_BUSY_SNAPSHOT을
        // 반환해 busy_timeout을 우회한다 — 쓰기 트랜잭션은 IMMEDIATE로 시작 (H-3)
        guard.conn().execute_batch("BEGIN IMMEDIATE")?;
        log::debug!("transaction begin");
        Ok(Tx {
            inner: self.inner,
            guard,
            open: true,
            sp_depth: std::cell::Cell::new(0),
            #[cfg(feature = "live")]
            pending: std::sync::Mutex::new(vec![TxPending::default()]),
        })
    }

    /// Runs a closure with one exclusively checked-out read/write connection.
    ///
    /// Do not acquire another connection from the same database inside the
    /// closure when every pooled connection may already be checked out. Doing
    /// so can block indefinitely (or until the configured queue timeout)
    /// because the outer checkout cannot return.
    ///
    /// With the `live` feature, direct writes are observed through SQLite's
    /// update hook. SQLite does not invoke that hook for every change: notably,
    /// truncate-optimized `DELETE` statements and changes to `WITHOUT ROWID`
    /// tables may be missed. Use the handle's SQL methods when live-query
    /// invalidation must be guaranteed.
    ///
    /// Installing an SQLite `preupdate_hook` replaces roomrs' live-query hook on
    /// that connection. With the `live` feature, call [`Self::rearm_hooks`]
    /// afterwards.
    pub fn with_connection<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let guard = self.inner.pool.connections.acquire()?;
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = f(guard.conn());
        #[cfg(feature = "live")]
        {
            let capture = self.inner.take_hook_capture();
            if !capture.changes.is_empty() {
                self.inner.tracker.invalidate_changes(capture.changes);
            } else if !capture.tables.is_empty() {
                self.inner.tracker.invalidate(Some(capture.tables));
            }
        }
        out
    }

    /// Reinstalls roomrs' update hook on every pooled connection.
    ///
    /// Waits for all checked-out connections to return, then updates the
    /// complete pool while new checkouts remain blocked.
    #[cfg(feature = "live")]
    pub fn rearm_hooks(&self) -> Result<()> {
        self.inner.pool.connections.for_each_connection(|conn| {
            crate::database::install_preupdate_hook(conn, Arc::clone(&self.inner.hook_columns))
        })
    }
}

// ─────────────────────── watch (live) ───────────────────────

/// 라이브 쿼리 생성 공용 구현 (명세 §5.7) — Sync/Async 핸들이 공유
#[cfg(feature = "live")]
pub(crate) mod watch_impl {
    use super::*;
    use crate::live::{InvalidationFilter, LiveQuery, OwnedParams, extract_tables};
    use rusqlite::ToSql;
    use std::collections::HashSet;

    /// 파라미터 변환 실패를 첫 emit 에러로 바꾸는 생성 헬퍼
    fn make<T: Clone + Send + 'static>(
        inner: &Arc<DatabaseInner>,
        sql: String,
        params: Result<OwnedParams>,
        tables: Option<HashSet<String>>,
        run: impl Fn(&Connection, &str, &OwnedParams) -> Result<T> + Send + Sync + 'static,
    ) -> LiveQuery<T> {
        let tracker = Arc::clone(&inner.tracker);
        match params {
            Ok(p) => LiveQuery::new(tracker, sql, p, tables, run),
            Err(e) => {
                // 변환 실패 — 빈 의존으로 등록하고 첫 emit이 에러가 되게 한다
                let msg = e.to_string();
                LiveQuery::new(
                    tracker,
                    sql,
                    OwnedParams::None,
                    Some(HashSet::new()),
                    move |_, _, _| Err(Error::Config(msg.clone())),
                )
            }
        }
    }

    /// N건 라이브
    pub(crate) fn watch_all<T: FromRow + Clone + Send + 'static>(
        inner: &Arc<DatabaseInner>,
        sql: &str,
        params: Result<OwnedParams>,
        tables: Option<HashSet<String>>,
    ) -> LiveQuery<Vec<T>> {
        let tables = tables.or_else(|| extract_tables(sql));
        make(
            inner,
            sql.to_string(),
            params,
            tables,
            crate::live::query_all_owned::<T>,
        )
    }

    /// 0~1건 라이브
    pub(crate) fn watch_optional<T: FromRow + Clone + Send + 'static>(
        inner: &Arc<DatabaseInner>,
        sql: &str,
        params: Result<OwnedParams>,
        tables: Option<HashSet<String>>,
    ) -> LiveQuery<Option<T>> {
        let tables = tables.or_else(|| extract_tables(sql));
        make(
            inner,
            sql.to_string(),
            params,
            tables,
            crate::live::query_optional_owned::<T>,
        )
    }

    /// 스칼라 라이브
    pub(crate) fn watch_scalar<T: FromSql + Clone + Send + 'static>(
        inner: &Arc<DatabaseInner>,
        sql: &str,
        params: Result<OwnedParams>,
        tables: Option<HashSet<String>>,
    ) -> LiveQuery<T> {
        let tables = tables.or_else(|| extract_tables(sql));
        make(
            inner,
            sql.to_string(),
            params,
            tables,
            crate::live::query_scalar_owned::<T>,
        )
    }

    pub(crate) fn watch_scalar_filtered<T: FromSql + Clone + Send + 'static>(
        inner: &Arc<DatabaseInner>,
        sql: &str,
        params: Result<OwnedParams>,
        filter: InvalidationFilter,
    ) -> LiveQuery<T> {
        let tables = Some(HashSet::from([filter.table_name().to_string()]));
        let tracker = Arc::clone(&inner.tracker);
        match params {
            Ok(p) => LiveQuery::new_filtered(
                tracker,
                sql.to_string(),
                p,
                tables,
                Some(filter),
                crate::live::query_scalar_owned::<T>,
            ),
            Err(e) => LiveQuery::new(
                tracker,
                sql.to_string(),
                OwnedParams::None,
                Some(HashSet::new()),
                move |_, _, _| Err(Error::Config(e.to_string())),
            ),
        }
    }

    /// DAO 힌트 테이블 목록 → 집합 (빈 목록 = 런타임 추출 위임)
    pub(crate) fn tables_from_hint(hint: &[&str]) -> Option<HashSet<String>> {
        if hint.is_empty() {
            None
        } else {
            Some(hint.iter().map(|s| s.to_string()).collect())
        }
    }

    /// 빌린 positional 파라미터 변환 래퍼
    pub(crate) fn params_from_dyn(params: &[&dyn ToSql]) -> Result<OwnedParams> {
        OwnedParams::from_dyn(params)
    }

    /// 매크로 생성 코드용 — 명명 파라미터 소유 변환 결과 래핑
    pub(crate) fn params_named(
        pairs: Result<Vec<(String, rusqlite::types::Value)>>,
    ) -> Result<OwnedParams> {
        pairs.map(OwnedParams::Named)
    }
}

#[cfg(feature = "live")]
impl SyncHandle<'_> {
    /// N건 라이브 쿼리 (명세 §5.7) — 의존 추출 실패 시 첫 수신이 `UnknownDependencies`
    pub fn watch_all<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> crate::live::LiveQuery<Vec<T>> {
        watch_impl::watch_all(self.inner, sql, watch_impl::params_from_dyn(params), None)
    }

    /// 0~1건 라이브 쿼리
    pub fn watch_optional<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> crate::live::LiveQuery<Option<T>> {
        watch_impl::watch_optional(self.inner, sql, watch_impl::params_from_dyn(params), None)
    }

    /// 스칼라 라이브 쿼리 (COUNT 등)
    pub fn watch_scalar<T: FromSql + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> crate::live::LiveQuery<T> {
        watch_impl::watch_scalar(self.inner, sql, watch_impl::params_from_dyn(params), None)
    }

    pub fn watch_scalar_filtered<T: FromSql + Clone + Send + 'static>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        filter: crate::live::InvalidationFilter,
    ) -> crate::live::LiveQuery<T> {
        watch_impl::watch_scalar_filtered(
            self.inner,
            sql,
            watch_impl::params_from_dyn(params),
            filter,
        )
    }
}

/// roomrs-async 전용 watch 진입점 — 직접 사용 금지
#[cfg(feature = "live")]
#[doc(hidden)]
impl DatabaseInner {
    pub fn __watch_all_dyn<T: FromRow + Clone + Send + 'static>(
        self: &Arc<Self>,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> crate::live::LiveQuery<Vec<T>> {
        watch_impl::watch_all(self, sql, watch_impl::params_from_dyn(params), None)
    }
    pub fn __watch_optional_dyn<T: FromRow + Clone + Send + 'static>(
        self: &Arc<Self>,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> crate::live::LiveQuery<Option<T>> {
        watch_impl::watch_optional(self, sql, watch_impl::params_from_dyn(params), None)
    }
    pub fn __watch_scalar_dyn<T: FromSql + Clone + Send + 'static>(
        self: &Arc<Self>,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> crate::live::LiveQuery<T> {
        watch_impl::watch_scalar(self, sql, watch_impl::params_from_dyn(params), None)
    }
    pub fn __watch_all_named<T: FromRow + Clone + Send + 'static>(
        self: &Arc<Self>,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Vec<T>> {
        watch_impl::watch_all(
            self,
            sql,
            watch_impl::params_named(params),
            watch_impl::tables_from_hint(tables),
        )
    }
    pub fn __watch_optional_named<T: FromRow + Clone + Send + 'static>(
        self: &Arc<Self>,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Option<T>> {
        watch_impl::watch_optional(
            self,
            sql,
            watch_impl::params_named(params),
            watch_impl::tables_from_hint(tables),
        )
    }
    pub fn __watch_scalar_named<T: FromSql + Clone + Send + 'static>(
        self: &Arc<Self>,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<T> {
        watch_impl::watch_scalar(
            self,
            sql,
            watch_impl::params_named(params),
            watch_impl::tables_from_hint(tables),
        )
    }
}

/// DAO watch 컨텍스트 (명세 §5.6) — 풀-바운드 전용.
/// Tx 구현은 "트랜잭션에서 라이브 불가" 에러를 첫 emit으로 전달한다.
#[cfg(feature = "live")]
pub trait WatchContext {
    /// 매크로 생성 코드 전용 — 직접 사용 금지
    #[doc(hidden)]
    fn ctx_watch_all_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Vec<T>>;
    #[doc(hidden)]
    fn ctx_watch_optional_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Option<T>>;
    #[doc(hidden)]
    fn ctx_watch_scalar_named<T: FromSql + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<T>;
}

#[cfg(feature = "live")]
impl<C: WatchContext> WatchContext for &C {
    fn ctx_watch_all_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Vec<T>> {
        (**self).ctx_watch_all_named(sql, params, tables)
    }
    fn ctx_watch_optional_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Option<T>> {
        (**self).ctx_watch_optional_named(sql, params, tables)
    }
    fn ctx_watch_scalar_named<T: FromSql + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<T> {
        (**self).ctx_watch_scalar_named(sql, params, tables)
    }
}

#[cfg(feature = "live")]
impl WatchContext for SyncHandle<'_> {
    fn ctx_watch_all_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Vec<T>> {
        watch_impl::watch_all(
            self.inner,
            sql,
            watch_impl::params_named(params),
            watch_impl::tables_from_hint(tables),
        )
    }
    fn ctx_watch_optional_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<Option<T>> {
        watch_impl::watch_optional(
            self.inner,
            sql,
            watch_impl::params_named(params),
            watch_impl::tables_from_hint(tables),
        )
    }
    fn ctx_watch_scalar_named<T: FromSql + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        params: Result<Vec<(String, rusqlite::types::Value)>>,
        tables: &[&str],
    ) -> crate::live::LiveQuery<T> {
        watch_impl::watch_scalar(
            self.inner,
            sql,
            watch_impl::params_named(params),
            watch_impl::tables_from_hint(tables),
        )
    }
}

#[cfg(feature = "live")]
impl WatchContext for Tx<'_> {
    /// 트랜잭션 컨텍스트 = 라이브 불가 — 첫 emit이 에러 (graceful)
    fn ctx_watch_all_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        _params: Result<Vec<(String, rusqlite::types::Value)>>,
        _tables: &[&str],
    ) -> crate::live::LiveQuery<Vec<T>> {
        watch_impl::watch_all(
            self.inner,
            sql,
            Err(Error::Config(
                "트랜잭션 컨텍스트에서는 라이브 쿼리를 만들 수 없습니다".into(),
            )),
            Some(std::collections::HashSet::new()),
        )
    }
    fn ctx_watch_optional_named<T: FromRow + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        _params: Result<Vec<(String, rusqlite::types::Value)>>,
        _tables: &[&str],
    ) -> crate::live::LiveQuery<Option<T>> {
        watch_impl::watch_optional(
            self.inner,
            sql,
            Err(Error::Config(
                "트랜잭션 컨텍스트에서는 라이브 쿼리를 만들 수 없습니다".into(),
            )),
            Some(std::collections::HashSet::new()),
        )
    }
    fn ctx_watch_scalar_named<T: FromSql + Clone + Send + 'static>(
        &self,
        sql: &'static str,
        _params: Result<Vec<(String, rusqlite::types::Value)>>,
        _tables: &[&str],
    ) -> crate::live::LiveQuery<T> {
        watch_impl::watch_scalar(
            self.inner,
            sql,
            Err(Error::Config(
                "트랜잭션 컨텍스트에서는 라이브 쿼리를 만들 수 없습니다".into(),
            )),
            Some(std::collections::HashSet::new()),
        )
    }
}

impl SqlContext for SyncHandle<'_> {
    fn ctx_execute<P: Params>(&self, sql: &str, params: P) -> Result<u64> {
        let guard = self.inner.pool.connections.acquire()?;
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::execute(guard.conn(), sql, params));
        // 자동 커밋 write — 즉시 방출 (명세 §9.2). OR FAIL/RAISE(FAIL)은 문장
        // 에러여도 선행 행 변경이 영속되므로 성패 무관 방출 (R3-1)
        #[cfg(feature = "live")]
        self.inner.emit_after_write(sql);
        out
    }
    fn ctx_insert<P: Params>(&self, sql: &str, params: P) -> Result<i64> {
        let guard = self.inner.pool.connections.acquire()?;
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::insert(guard.conn(), sql, params));
        // 성패 무관 방출 — OR FAIL 부분 적용 대비 (R3-1)
        #[cfg(feature = "live")]
        self.inner.emit_after_write(sql);
        out
    }
    fn ctx_query_all<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Vec<T>> {
        let guard = self.inner.pool.connections.acquire()?;
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::query_all(guard.conn(), sql, params));
        #[cfg(feature = "live")]
        self.inner.emit_after_write(sql);
        out
    }
    fn ctx_query_optional<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Option<T>> {
        let guard = self.inner.pool.connections.acquire()?;
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::query_optional(guard.conn(), sql, params));
        #[cfg(feature = "live")]
        self.inner.emit_after_write(sql);
        out
    }
    fn ctx_query_scalar<T: FromSql, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        let guard = self.inner.pool.connections.acquire()?;
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::query_scalar(guard.conn(), sql, params));
        #[cfg(feature = "live")]
        self.inner.emit_after_write(sql);
        out
    }
    /// 풀-바운드 트랜잭션 — BEGIN/COMMIT (에러 시 롤백)
    fn ctx_transaction<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R> {
        let tx = self.begin()?;
        match f(&tx) {
            Ok(v) => {
                tx.commit()?;
                Ok(v)
            }
            Err(e) => {
                // 롤백 실패는 원인 에러를 대체하지 않는다 (L-1) — drop 경로가 재시도
                let _ = tx.rollback();
                Err(e)
            }
        }
    }
}

// ─────────────────────── Tx ───────────────────────

/// A synchronous transaction bound to its checked-out connection and thread.
///
/// `Tx` is intentionally not `Send`: SQLite transaction work and rollback
/// must remain on the checkout owner thread. Use the async handle to schedule
/// database work instead of moving a synchronous transaction between threads.
/// An uncommitted transaction is rolled back when dropped.
/// With the `live` feature, `PRAGMA` statements conservatively accumulate full
/// invalidation, including read-only forms.
pub struct Tx<'db> {
    inner: &'db Arc<DatabaseInner>,
    guard: crate::pool::ConnectionGuard<'db>,
    open: bool,
    /// savepoint 중첩 깊이 — 이름 생성용 (명세 §5.9 중첩)
    sp_depth: std::cell::Cell<u32>,
    /// tx 동안 누적된 무효화 대상 — savepoint 깊이별 레벨 스택 (L-8),
    /// commit 성공 후에만 방출 (명세 §9.2)
    #[cfg(feature = "live")]
    pending: std::sync::Mutex<Vec<TxPending>>,
}

/// tx-로컬 무효화 누적 상태 (savepoint 레벨 단위)
#[cfg(feature = "live")]
#[derive(Default)]
struct TxPending {
    /// 파싱 실패 발생 = 전체 무효화 필요
    all: bool,
    tables: std::collections::HashSet<String>,
    changes: Vec<crate::live::TableChange>,
}

#[cfg(feature = "live")]
impl Tx<'_> {
    /// write 문장 1건의 무효화 대상 누적 — 문장 파싱 ∪ 훅 (현재 savepoint 레벨에).
    /// 읽기 전용 문장은 문장 기반 누적을 하지 않는다 (L-2) — 훅 수집분만 병합
    fn collect_write(&self, sql: &str) {
        let capture = self.inner.take_hook_capture();
        let mut levels = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // begin이 기본 레벨 1개를 보장 — 비어 있으면 방어적으로 무시
        let Some(p) = levels.last_mut() else { return };
        p.tables.extend(capture.tables);
        p.changes.extend(capture.changes);
        match crate::live::extract_write_tables(sql) {
            crate::live::WriteTables::ReadOnly => {}
            crate::live::WriteTables::Tables(t) => p.tables.extend(t),
            crate::live::WriteTables::Unknown => p.all = true,
        }
    }

    /// 부분 실패 배치 등 결과를 알 수 없는 write — 훅 수집 ∪ 전체 무효화를
    /// 현재 레벨에 보수 누적한다 (R2-3). 커밋되지 않으면 방출되지 않으므로 무해
    fn collect_write_all(&self) {
        let capture = self.inner.take_hook_capture();
        let mut levels = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(p) = levels.last_mut() else { return };
        p.tables.extend(capture.tables);
        p.changes.extend(capture.changes);
        p.all = true;
    }

    /// commit 성공 후 방출 — 남은 레벨 전부 병합
    fn emit_pending(&self) {
        let levels = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut all = false;
        let mut tables = std::collections::HashSet::new();
        let mut changes = Vec::new();
        for l in levels {
            all |= l.all;
            tables.extend(l.tables);
            changes.extend(l.changes);
        }
        if all {
            self.inner.tracker.invalidate(None);
        } else {
            let changed_tables: std::collections::HashSet<String> = changes
                .iter()
                .map(|change: &crate::live::TableChange| change.table.clone())
                .collect();
            if !changes.is_empty() {
                self.inner.tracker.invalidate_changes(changes);
            }
            tables.retain(|table| !changed_tables.contains(table));
            if !tables.is_empty() {
                self.inner.tracker.invalidate(Some(tables));
            }
        }
    }
}

impl Tx<'_> {
    /// 트랜잭션 커넥션 — 크레이트 내부 전용 (마이그레이션 러너)
    pub(crate) fn raw_conn(&self) -> &Connection {
        self.guard.conn()
    }

    /// 커밋 — 이 Tx 소비. 성공 시 누적 무효화 방출 (명세 §9.2 — commit API 성공 반환 후)
    pub fn commit(mut self) -> Result<()> {
        self.guard.conn().execute_batch("COMMIT")?;
        log::debug!("transaction commit");
        self.open = false;
        #[cfg(feature = "live")]
        self.emit_pending();
        Ok(())
    }

    /// 명시 롤백 — 이 Tx 소비
    pub fn rollback(mut self) -> Result<()> {
        self.guard.conn().execute_batch("ROLLBACK")?;
        log::debug!("transaction rollback");
        self.open = false;
        Ok(())
    }

    /// 쓰기 실행 (트랜잭션 커넥션)
    pub fn execute<P: Params>(&self, sql: &str, params: P) -> Result<u64> {
        self.ctx_execute(sql, params)
    }

    /// 정확히 1건 조회
    pub fn query_one<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        self.ctx_query_one(sql, params)
    }

    /// 0~1건 조회
    pub fn query_optional<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Option<T>> {
        self.ctx_query_optional(sql, params)
    }

    /// 스칼라 1건 조회
    pub fn query_scalar<T: FromSql, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        self.ctx_query_scalar(sql, params)
    }

    /// N건 조회
    pub fn query_all<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Vec<T>> {
        self.ctx_query_all(sql, params)
    }

    /// 배치 실행 — 여러 문장 세미콜론 구분 (마이그레이션·일괄 write용).
    ///
    /// Batch writes participate in invalidation (M-2): affected tables are
    /// collected per statement and emitted after the transaction commits. A
    /// statement that cannot be parsed degrades to a conservative
    /// full invalidation.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let r = self.raw_conn().execute_batch(sql);
        #[cfg(feature = "live")]
        match &r {
            // 배치 write도 커밋 시 무효화에 포함 (M-2)
            Ok(()) => self.collect_write(sql),
            // 부분 실패 — 선행 문장은 이미 적용됐을 수 있다: 사용자가 에러를
            // 삼키고 커밋해도 무효화가 소실되지 않게 보수 누적 (R2-3)
            Err(_) => self.collect_write_all(),
        }
        r?;
        Ok(())
    }

    /// 중첩 트랜잭션 — SAVEPOINT (명세 §5.9).
    /// 실패 시 savepoint까지만 롤백, 외부 트랜잭션은 계속된다.
    /// 무효화 누적도 레벨 단위 — 롤백된 write는 방출하지 않는다 (L-8)
    pub fn savepoint<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R> {
        let depth = self.sp_depth.get();
        let name = format!("roomrs_sp_{depth}");
        self.guard
            .conn()
            .execute_batch(&format!("SAVEPOINT {name}"))?;
        self.sp_depth.set(depth + 1);
        // savepoint 레벨 격리 — 롤백 시 이 레벨 무효화 대상을 폐기 (L-8)
        #[cfg(feature = "live")]
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(TxPending::default());

        let out = f(self);

        self.sp_depth.set(depth);
        // 레벨 회수 — 성공 시 부모로 병합, 실패 시 폐기 (L-8)
        #[cfg(feature = "live")]
        let level = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or_default();
        match out {
            Ok(v) => {
                // RELEASE 실패 시에도 savepoint 데이터는 남아 있으므로 먼저 병합
                #[cfg(feature = "live")]
                {
                    let mut levels = self
                        .pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(parent) = levels.last_mut() {
                        parent.all |= level.all;
                        parent.tables.extend(level.tables);
                        parent.changes.extend(level.changes);
                    }
                }
                self.guard
                    .conn()
                    .execute_batch(&format!("RELEASE {name}"))?;
                Ok(v)
            }
            Err(e) => {
                // ROLLBACK TO는 savepoint를 유지하므로 RELEASE로 제거까지.
                // 롤백 실패는 원인 에러를 대체하지 않는다 (M-3) — 로그 후 원본 반환
                if let Err(re) = self
                    .guard
                    .conn()
                    .execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"))
                {
                    log::error!("savepoint rollback failed: {re}");
                }
                Err(e)
            }
        }
    }
}

impl SqlContext for Tx<'_> {
    fn ctx_execute<P: Params>(&self, sql: &str, params: P) -> Result<u64> {
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::execute(self.guard.conn(), sql, params));
        // OR FAIL/RAISE(FAIL)은 문장 에러여도 선행 행 변경이 남는다 —
        // 성패 무관 수집(롤백되면 방출 안 됨) (R3-1)
        #[cfg(feature = "live")]
        self.collect_write(sql);
        out
    }
    fn ctx_insert<P: Params>(&self, sql: &str, params: P) -> Result<i64> {
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::insert(self.guard.conn(), sql, params));
        // 성패 무관 수집 — OR FAIL 부분 적용 대비 (R3-1)
        #[cfg(feature = "live")]
        self.collect_write(sql);
        out
    }
    fn ctx_query_all<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Vec<T>> {
        // 쓰기 가능 SQL(INSERT…RETURNING 등)은 execute와 동일한 무효화 수집 (H-1).
        // RETURNING은 첫 step에서 DML이 완결되므로 매핑 실패에도 write가 영속될
        // 수 있다 — 성패 무관 수집(롤백되면 방출 안 됨) (R2-2)
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self
            .inner
            .log_query(sql, || on_conn::query_all(self.guard.conn(), sql, params));
        #[cfg(feature = "live")]
        self.collect_write(sql);
        out
    }
    fn ctx_query_optional<T: FromRow, P: Params>(&self, sql: &str, params: P) -> Result<Option<T>> {
        // 쓰기 가능 SQL은 execute와 동일한 무효화 수집 (H-1).
        // 매핑 실패에도 write가 영속될 수 있어 성패 무관 수집 (R2-2)
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self.inner.log_query(sql, || {
            on_conn::query_optional(self.guard.conn(), sql, params)
        });
        #[cfg(feature = "live")]
        self.collect_write(sql);
        out
    }
    fn ctx_query_scalar<T: FromSql, P: Params>(&self, sql: &str, params: P) -> Result<T> {
        // 쓰기 가능 SQL은 execute와 동일한 무효화 수집 (H-1).
        // 매핑 실패에도 write가 영속될 수 있어 성패 무관 수집 (R2-2)
        #[cfg(feature = "live")]
        let _hook_capture = self.inner.begin_hook_capture();
        let out = self.inner.log_query(sql, || {
            on_conn::query_scalar(self.guard.conn(), sql, params)
        });
        #[cfg(feature = "live")]
        self.collect_write(sql);
        out
    }
    /// tx-바운드 트랜잭션 = 중첩 savepoint (명세 §5.9)
    fn ctx_transaction<R>(&self, f: impl FnOnce(&Tx<'_>) -> Result<R>) -> Result<R> {
        self.savepoint(f)
    }
}

impl Drop for Tx<'_> {
    /// 미커밋 drop = 롤백 — 실패해도 panic하지 않는다(커넥션 반납이 우선)
    fn drop(&mut self) {
        if self.open {
            log::debug!("transaction rollback (uncommitted drop)");
            let _ = self.guard.conn().execute_batch("ROLLBACK");
        }
    }
}
