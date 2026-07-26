//! 라이브 쿼리 엔진 (명세 §5.6, §9) — feature `live`
//!
//! 주 경로: 문장 기반 무효화(commit 성공 후 방출) · 보조: preupdate_hook 행 매칭.
//! notifier는 이벤트 병합·observer별 고정 coalesce 예약·작업 제출만 담당한다.
//! 재조회는 `roomrs-live-worker-{n}` 이 통합 read/write 풀에서 checkout 해 수행한다 (결정 51).
//! 재조회·콜백은 레지스트리/콜백 락 밖에서 실행된다 — 콜백 내 재진입(구독 생성·해지) 허용.

use crate::error::{Error, Result};
use crate::pool::ConnectionPool;
use crate::query::IntoDbValue;
use crate::row::FromRow;
use rusqlite::types::Value;
use rusqlite::{Connection, ToSql};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Default live-query debounce window (decision 49).
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(250);

/// Read-only LiveQuery observability snapshot (spec §9.5 P2).
///
/// Counters never include SQL parameters or row payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveMetrics {
    /// Invalidation messages received by the notifier (`Invalidate` / `Changes`).
    pub events_received: u64,
    /// Extra invalidations merged into an already-open fixed coalesce window.
    pub coalesce_merged: u64,
    /// Current live-worker job queue depth (point-in-time).
    pub worker_queue_depth: u64,
    /// Refresh closures that published a successful result.
    pub refresh_ok: u64,
    /// Refresh failures (query error, pool checkout error, isolated panic).
    pub refresh_err: u64,
    /// In-flight refresh results discarded by epoch (rebind/unregister/newer schedule).
    pub stale_discarded: u64,
}

/// process-wide atomic counters shared by notifier / workers / refresh closures
struct LiveMetricsState {
    events_received: AtomicU64,
    coalesce_merged: AtomicU64,
    refresh_ok: AtomicU64,
    refresh_err: AtomicU64,
    stale_discarded: AtomicU64,
}

impl LiveMetricsState {
    /// zeroed counters
    fn new() -> Self {
        Self {
            events_received: AtomicU64::new(0),
            coalesce_merged: AtomicU64::new(0),
            refresh_ok: AtomicU64::new(0),
            refresh_err: AtomicU64::new(0),
            stale_discarded: AtomicU64::new(0),
        }
    }

    /// point-in-time snapshot with queue depth filled by caller
    fn snapshot(&self, worker_queue_depth: u64) -> LiveMetrics {
        LiveMetrics {
            events_received: self.events_received.load(Ordering::Relaxed),
            coalesce_merged: self.coalesce_merged.load(Ordering::Relaxed),
            worker_queue_depth,
            refresh_ok: self.refresh_ok.load(Ordering::Relaxed),
            refresh_err: self.refresh_err.load(Ordering::Relaxed),
            stale_discarded: self.stale_discarded.load(Ordering::Relaxed),
        }
    }
}

// ─────────────────── 행 무효화 필터 ───────────────────

/// Row-level invalidation condition for a live query.
///
/// This is not a SQL query builder. It determines only whether a changed row
/// can affect a subscription result. Predicates within a group are ANDed and
/// groups are ORed.
///
/// Multiple filters on one LiveQuery are OR-matched: any matching filter
/// triggers a re-query (decision 52). Filter-to-filter AND is not supported.
#[derive(Debug, Clone)]
pub struct InvalidationFilter {
    table: String,
    groups: Vec<InvalidationGroup>,
}

/// Converts a single filter or a collection into the list accepted by
/// `watch_*_filtered` (OR semantics, decision 52).
pub trait IntoInvalidationFilters {
    /// Owned filter list for registration.
    fn into_invalidation_filters(self) -> Vec<InvalidationFilter>;
}

impl IntoInvalidationFilters for InvalidationFilter {
    /// 단일 filter → 길이 1 목록
    fn into_invalidation_filters(self) -> Vec<InvalidationFilter> {
        vec![self]
    }
}

impl IntoInvalidationFilters for Vec<InvalidationFilter> {
    /// 소유 목록 그대로
    fn into_invalidation_filters(self) -> Vec<InvalidationFilter> {
        self
    }
}

impl IntoInvalidationFilters for &[InvalidationFilter] {
    /// 슬라이스 복제
    fn into_invalidation_filters(self) -> Vec<InvalidationFilter> {
        self.to_vec()
    }
}

impl<const N: usize> IntoInvalidationFilters for [InvalidationFilter; N] {
    /// 고정 배열 → 목록
    fn into_invalidation_filters(self) -> Vec<InvalidationFilter> {
        self.into()
    }
}

/// Builder for [`InvalidationFilter`].
#[derive(Debug, Clone)]
pub struct InvalidationFilterBuilder {
    table: String,
    groups: Vec<InvalidationGroup>,
}

/// Builder for an AND predicate group.
#[derive(Debug, Clone, Default)]
pub struct InvalidationGroupBuilder {
    predicates: Vec<InvalidationPredicate>,
}

#[derive(Debug, Clone)]
struct InvalidationGroup {
    predicates: Vec<InvalidationPredicate>,
}

#[derive(Debug, Clone)]
enum InvalidationPredicate {
    Eq { column: String, value: Value },
    Neq { column: String, value: Value },
    IsNull { column: String },
    IsNotNull { column: String },
}

/// Hook이 수집한 변경 행. `None`은 INSERT의 OLD 또는 DELETE의 NEW다.
#[derive(Debug, Clone)]
pub(crate) struct TableChange {
    pub(crate) table: String,
    pub(crate) old: Option<HashMap<String, Value>>,
    pub(crate) new: Option<HashMap<String, Value>>,
}

impl InvalidationFilter {
    /// Starts a row invalidation filter for a table.
    pub fn table(table: impl Into<String>) -> InvalidationFilterBuilder {
        InvalidationFilterBuilder { table: table.into(), groups: Vec::new() }
    }

    /// Returns target table name.
    pub fn table_name(&self) -> &str {
        &self.table
    }

    /// predicate 가 참조하는 컬럼명 집합 (중복 제거).
    fn referenced_columns(&self) -> HashSet<&str> {
        let mut out = HashSet::new();
        for group in &self.groups {
            for p in &group.predicates {
                out.insert(p.column_name());
            }
        }
        out
    }

    /// 실제 SQLite 스키마에 table·column 존재 여부를 검증한다 (명세 §9.5 P1).
    /// 오타는 조용히 무시하지 않고 [`Error::InvalidationFilter`] 로 반환한다.
    pub(crate) fn validate_against_schema(&self, conn: &Connection) -> Result<()> {
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type IN ('table', 'view') AND name = ?1 COLLATE NOCASE",
                [self.table.as_str()],
                |row| row.get(0),
            )
            .map_err(Error::from)?;
        if table_exists == 0 {
            return Err(Error::InvalidationFilter(format!("테이블 '{}' 이(가) 스키마에 없습니다 — InvalidationFilter::table 이름을 확인하세요", self.table)));
        }

        // pragma_table_info 는 테이블명 파라미터를 받아 컬럼 목록을 돌려준다.
        let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1)").map_err(Error::from)?;
        let col_rows = stmt.query_map([self.table.as_str()], |row| row.get::<_, String>(0)).map_err(Error::from)?;
        let mut schema_cols: HashSet<String> = HashSet::new();
        for col in col_rows {
            schema_cols.insert(col.map_err(Error::from)?.to_ascii_lowercase());
        }
        if schema_cols.is_empty() {
            return Err(Error::InvalidationFilter(format!("테이블 '{}' 의 컬럼 정보를 읽지 못했습니다 — 스키마를 확인하세요", self.table)));
        }

        for column in self.referenced_columns() {
            if !schema_cols.contains(&column.to_ascii_lowercase()) {
                return Err(Error::InvalidationFilter(format!("테이블 '{}' 에 컬럼 '{}' 이(가) 없습니다 — InvalidationFilter predicate 컬럼명을 확인하세요", self.table, column)));
            }
        }
        Ok(())
    }

    /// 변경 전 또는 후 행이 조건을 만족하면 true.
    fn matches_change(&self, change: &TableChange) -> bool {
        self.table.eq_ignore_ascii_case(&change.table) && [change.old.as_ref(), change.new.as_ref()].into_iter().flatten().any(|row| self.matches_row(row))
    }

    /// OR 그룹 중 하나가 행과 일치하면 true.
    fn matches_row(&self, row: &HashMap<String, Value>) -> bool {
        self.groups.iter().any(|group| group.predicates.iter().all(|p| p.matches(row)))
    }
}

/// 여러 filter 중 하나라도 변경 행에 매칭되면 true (filter 간 OR, 결정 52).
fn any_filter_matches(filters: &[InvalidationFilter], change: &TableChange) -> bool {
    filters.iter().any(|f| f.matches_change(change))
}

/// 필터 목록 전체 스키마 검증 — 하나라도 실패하면 첫 오류 반환.
pub(crate) fn validate_filters_against_schema(conn: &Connection, filters: &[InvalidationFilter]) -> Result<()> {
    if filters.is_empty() {
        return Err(Error::Config("InvalidationFilter 목록이 비어 있습니다 — 필터를 1개 이상 지정하세요".into()));
    }
    for f in filters {
        f.validate_against_schema(conn)?;
    }
    Ok(())
}

impl InvalidationFilterBuilder {
    /// Adds an AND group. Subsequent groups are ORed with prior groups.
    pub fn where_group(mut self, build: impl FnOnce(InvalidationGroupBuilder) -> InvalidationGroupBuilder) -> Self {
        self.groups.push(InvalidationGroup { predicates: build(InvalidationGroupBuilder::default()).predicates });
        self
    }

    /// Adds an AND group ORed with prior groups.
    pub fn or_where_group(self, build: impl FnOnce(InvalidationGroupBuilder) -> InvalidationGroupBuilder) -> Self {
        self.where_group(build)
    }

    /// Validates and builds filter.
    pub fn build(self) -> Result<InvalidationFilter> {
        if self.table.trim().is_empty() {
            return Err(Error::Config("무효화 필터 테이블명은 비어 있을 수 없습니다".into()));
        }
        if self.groups.is_empty() || self.groups.iter().any(|g| g.predicates.is_empty()) {
            return Err(Error::Config("무효화 필터에는 비어 있지 않은 조건 그룹이 필요합니다".into()));
        }
        Ok(InvalidationFilter { table: self.table, groups: self.groups })
    }
}

impl InvalidationGroupBuilder {
    /// Matches rows whose column equals value.
    pub fn eq(mut self, column: impl Into<String>, value: impl IntoDbValue) -> Self {
        self.predicates.push(InvalidationPredicate::Eq { column: column.into(), value: value.into_db_value() });
        self
    }

    /// Matches rows whose column differs from value. NULL never matches.
    pub fn neq(mut self, column: impl Into<String>, value: impl IntoDbValue) -> Self {
        self.predicates.push(InvalidationPredicate::Neq { column: column.into(), value: value.into_db_value() });
        self
    }

    /// Matches rows whose column is NULL.
    pub fn is_null(mut self, column: impl Into<String>) -> Self {
        self.predicates.push(InvalidationPredicate::IsNull { column: column.into() });
        self
    }

    /// Matches rows whose column is not NULL.
    pub fn is_not_null(mut self, column: impl Into<String>) -> Self {
        self.predicates.push(InvalidationPredicate::IsNotNull { column: column.into() });
        self
    }
}

impl InvalidationPredicate {
    /// predicate 가 참조하는 컬럼명.
    fn column_name(&self) -> &str {
        match self {
            Self::Eq { column, .. } | Self::Neq { column, .. } | Self::IsNull { column } | Self::IsNotNull { column } => column.as_str(),
        }
    }

    /// SQL WHERE의 NULL 3값 논리를 따라 predicate 하나를 평가한다.
    fn matches(&self, row: &HashMap<String, Value>) -> bool {
        match self {
            Self::Eq { column, value } => row.get(column).is_some_and(|v| value != &Value::Null && v != &Value::Null && v == value),
            Self::Neq { column, value } => row.get(column).is_some_and(|v| value != &Value::Null && v != &Value::Null && v != value),
            Self::IsNull { column } => matches!(row.get(column), Some(Value::Null)),
            Self::IsNotNull { column } => row.get(column).is_some_and(|v| v != &Value::Null),
        }
    }
}

/// poison 복구 락 — 콜백 panic 후에도 트래커/구독 상태는 계속 동작해야 한다 (H-4).
/// poison은 panic 직후에만 발생하므로 warn 로그가 스팸이 되지 않는다
fn plock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| {
        log::warn!("mutex poisoned in live-query state — recovering");
        e.into_inner()
    })
}

// ─────────────────── SQL 테이블 추출 ───────────────────

/// 쿼리 전체(WHERE/프로젝션/FROM 파생 테이블의 서브쿼리 포함) 참조 테이블 방문 —
/// CTE·미지원 테이블 팩터 발견 시 false(보수 처리: None/UnknownDependencies 유도) (H-2)
fn query_tables(q: &sqlparser::ast::Query, out: &mut HashSet<String>) -> bool {
    use core::ops::ControlFlow;
    use sqlparser::ast::{ObjectName, Query, TableFactor, Visit, Visitor};

    /// AST 방문자 — 서브쿼리 내부까지 실 테이블명 수집
    struct Deps<'a> {
        out: &'a mut HashSet<String>,
    }
    impl Visitor for Deps<'_> {
        type Break = ();
        /// CTE 포함 쿼리 — 별칭이 실 테이블과 구분되지 않아 보수 처리(실패)
        fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<()> {
            if q.with.is_some() {
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        }
        /// 실 테이블 이름 수집 (FROM/JOIN/서브쿼리)
        fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<()> {
            if let Some(last) = name.0.last() {
                self.out.insert(last.value.clone());
            }
            ControlFlow::Continue(())
        }
        /// 테이블 함수 등 미지원 팩터 = 의존 미상 — 실패로 보수 처리
        fn pre_visit_table_factor(&mut self, tf: &TableFactor) -> ControlFlow<()> {
            match tf {
                TableFactor::Table { .. } | TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => ControlFlow::Continue(()),
                _ => ControlFlow::Break(()),
            }
        }
    }
    q.visit(&mut Deps { out }).is_continue()
}

/// 테이블 팩터 — 실 테이블만 허용 (UPDATE/DELETE 대상)
fn table_factor(tf: &sqlparser::ast::TableFactor, out: &mut HashSet<String>) -> bool {
    match tf {
        sqlparser::ast::TableFactor::Table { name, .. } => {
            if let Some(last) = name.0.last() {
                out.insert(last.value.clone());
            }
            true
        }
        _ => false,
    }
}

/// SQL에서 참조/영향 테이블 추출 — 실패 = None(보수적 전체 무효화/UnknownDependencies)
pub(crate) fn extract_tables(sql: &str) -> Option<HashSet<String>> {
    use sqlparser::ast::Statement;
    use sqlparser::dialect::SQLiteDialect;
    use sqlparser::parser::Parser;

    let stmts = Parser::parse_sql(&SQLiteDialect {}, sql).ok()?;
    let mut out = HashSet::new();
    for stmt in &stmts {
        let ok = match stmt {
            // SELECT — 서브쿼리(WHERE IN/EXISTS/프로젝션) 포함 전체 방문 (H-2)
            Statement::Query(q) => query_tables(q, &mut out),
            Statement::Insert(ins) => {
                out.insert(ins.table_name.0.last()?.value.clone());
                true
            }
            Statement::Update { table, .. } => table_factor(&table.relation, &mut out),
            Statement::Delete(del) => {
                let from = match &del.from {
                    sqlparser::ast::FromTable::WithFromKeyword(v) | sqlparser::ast::FromTable::WithoutKeyword(v) => v,
                };
                from.iter().all(|t| table_factor(&t.relation, &mut out))
            }
            Statement::Pragma { .. } => true, // PRAGMA — 테이블 영향 없음
            // DDL(CREATE/ALTER/DROP 등)·기타 문장 — preupdate_hook도 발화하지 않으므로
            // 테이블 추출 실패로 처리해 보수적 전체 무효화를 유도한다 (M-3)
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    Some(out)
}

/// write 문장 무효화 분류 결과 (L-2) — 읽기 전용 문장은 방출하지 않는다
pub(crate) enum WriteTables {
    /// 읽기 전용 문장만(SELECT/EXPLAIN) — 문장 기반 무효화 없음
    ReadOnly,
    /// write 대상 테이블 집합
    Tables(HashSet<String>),
    /// 파싱 실패/DDL 등 — 보수적 전체 무효화
    Unknown,
}

/// 파서가 거부해도 실행 없는 단일 읽기 문장임이 명백한지 확인한다.
///
/// write 오분류는 허용하지 않는다. 따라서 세미콜론으로 이어진 문장이나 SELECT가
/// 아닌 SQLite 확장은 보수적으로 Unknown에 남긴다.
fn obvious_single_read(sql: &str) -> bool {
    let mut sql = sql.trim();
    loop {
        if let Some(comment) = sql.strip_prefix("--") {
            let Some(end) = comment.find('\n') else {
                return false;
            };
            sql = comment[end + 1..].trim_start();
        } else if let Some(comment) = sql.strip_prefix("/*") {
            let Some(end) = comment.find("*/") else {
                return false;
            };
            sql = comment[end + 2..].trim_start();
        } else {
            break;
        }
    }
    let sql = sql.strip_suffix(';').unwrap_or(sql).trim_end();
    if sql.contains(';') {
        return false;
    }
    let Some(keyword) = sql.split_ascii_whitespace().next() else {
        return false;
    };
    keyword.eq_ignore_ascii_case("SELECT") || keyword.eq_ignore_ascii_case("EXPLAIN")
}

/// write 경로(emit/collect) 전용 — SQL을 문장 종류로 분류해 영향 테이블 추출 (L-2).
/// SELECT류는 무효화를 만들지 않고, DDL·파싱 실패는 전체 무효화로 보수 처리한다
pub(crate) fn extract_write_tables(sql: &str) -> WriteTables {
    use sqlparser::ast::Statement;
    use sqlparser::dialect::SQLiteDialect;
    use sqlparser::parser::Parser;

    let Ok(stmts) = Parser::parse_sql(&SQLiteDialect {}, sql) else {
        return if obvious_single_read(sql) { WriteTables::ReadOnly } else { WriteTables::Unknown };
    };
    let mut out = HashSet::new();
    let mut any_write = false;
    for stmt in &stmts {
        match stmt {
            // sqlparser 0.52는 CTE-write(WITH … INSERT/UPDATE)를
            // Query(body=Insert/Update)로 파싱한다 — 읽기로 오분류하면 훅 미발화
            // 테이블(WITHOUT ROWID/FTS5)에서 무효화가 소실되므로 보수 처리 (R2-1)
            Statement::Query(q) => match q.body.as_ref() {
                sqlparser::ast::SetExpr::Insert(_) | sqlparser::ast::SetExpr::Update(_) | sqlparser::ast::SetExpr::Table(_) => return WriteTables::Unknown,
                // 읽기 전용 — 문장 기반 무효화 없음 (L-2)
                _ => {}
            },
            // 읽기 전용 — 문장 기반 무효화 없음 (L-2)
            Statement::Explain { .. } => {}
            // PRAGMA는 조회와 connection/DB 상태 변경을 AST만으로 확실히
            // 구분할 수 없어 전체 무효화로 보수 처리한다.
            Statement::Pragma { .. } => return WriteTables::Unknown,
            Statement::Insert(ins) => {
                any_write = true;
                match ins.table_name.0.last() {
                    Some(last) => {
                        out.insert(last.value.clone());
                    }
                    None => return WriteTables::Unknown,
                }
            }
            Statement::Update { table, .. } => {
                any_write = true;
                if !table_factor(&table.relation, &mut out) {
                    return WriteTables::Unknown;
                }
            }
            Statement::Delete(del) => {
                any_write = true;
                let from = match &del.from {
                    sqlparser::ast::FromTable::WithFromKeyword(v) | sqlparser::ast::FromTable::WithoutKeyword(v) => v,
                };
                if !from.iter().all(|t| table_factor(&t.relation, &mut out)) {
                    return WriteTables::Unknown;
                }
            }
            // DDL 등 — preupdate_hook 미발화 가능, 보수적 전체 무효화 (M-3)
            _ => return WriteTables::Unknown,
        }
    }
    if any_write { WriteTables::Tables(out) } else { WriteTables::ReadOnly }
}

// ─────────────────── 소유 파라미터 ───────────────────

/// 재조회 가능한 소유 파라미터 (rusqlite Params는 1회성이므로 자체 표현)
#[derive(Clone, Default)]
pub(crate) enum OwnedParams {
    #[default]
    None,
    Positional(Vec<Value>),
    Named(Vec<(String, Value)>),
}

impl OwnedParams {
    /// 빌린 positional 파라미터를 소유로 변환
    pub(crate) fn from_dyn(params: &[&dyn ToSql]) -> Result<Self> {
        if params.is_empty() {
            return Ok(Self::None);
        }
        let vals: Result<Vec<Value>> = params.iter().map(|p| crate::entity::to_owned_value(*p)).collect();
        Ok(Self::Positional(vals?))
    }

    /// 문장에 바인딩해 실행 준비 — 재조회마다 호출
    fn bind(&self, stmt: &mut rusqlite::Statement<'_>) -> Result<()> {
        match self {
            Self::None => {}
            Self::Positional(vals) => {
                for (i, v) in vals.iter().enumerate() {
                    stmt.raw_bind_parameter(i + 1, v)?;
                }
            }
            Self::Named(pairs) => {
                for (k, v) in pairs {
                    let idx = stmt.parameter_index(k)?.ok_or_else(|| Error::Config(format!("알 수 없는 파라미터: {k}")))?;
                    stmt.raw_bind_parameter(idx, v)?;
                }
            }
        }
        Ok(())
    }
}

/// 소유 파라미터로 N건 조회 (raw 바인딩 경로)
pub(crate) fn query_all_owned<T: FromRow>(conn: &Connection, sql: &str, params: &OwnedParams) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    params.bind(&mut stmt)?;
    let mut rows = stmt.raw_query();
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(T::from_row(row)?);
    }
    Ok(out)
}

/// 소유 파라미터로 0~1건 조회
pub(crate) fn query_optional_owned<T: FromRow>(conn: &Connection, sql: &str, params: &OwnedParams) -> Result<Option<T>> {
    let mut stmt = conn.prepare(sql)?;
    params.bind(&mut stmt)?;
    let mut rows = stmt.raw_query();
    match rows.next()? {
        Some(row) => Ok(Some(T::from_row(row)?)),
        None => Ok(None),
    }
}

/// 소유 파라미터로 스칼라 조회 — 0건 = NotFound
pub(crate) fn query_scalar_owned<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str, params: &OwnedParams) -> Result<T> {
    let mut stmt = conn.prepare(sql)?;
    params.bind(&mut stmt)?;
    let mut rows = stmt.raw_query();
    match rows.next()? {
        Some(row) => Ok(row.get(0)?),
        None => Err(Error::NotFound),
    }
}

// ─────────────────── 트래커 / 노티파이어 ───────────────────
// ─────────────────── 트래커 / 노티파이어 / live worker ───────────────────

/// 노티파이어 메시지
pub(crate) enum Msg {
    /// 테이블 집합 무효화 (None = 전체 — 파싱 실패 보수 경로)
    Invalidate(Option<HashSet<String>>),
    /// preupdate hook이 수집한 행 변경 무효화.
    Changes(Vec<TableChange>),
    /// 특정 구독 전체 재조회 (초기 emit·rebind·watching)
    Refresh(u64),
    /// 새 콜백 전용 — 캐시 값 전달, 기존 구독자 재-emit 없음 (L-7)
    RefreshNew(u64),
    /// 종료
    Shutdown,
}

/// 재조회 종류 — Full: 전체 팬아웃, NewOnly: 새 콜백에만 캐시 전달 (L-7)
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RefreshKind {
    Full,
    NewOnly,
}

/// 재조회 클로저 — `job_gen` 은 Full 스케줄 시점 세대. 스테일 결과 폐기용 (결정 51)
type RefreshFn = Arc<dyn Fn(&Connection, RefreshKind, u64) + Send + Sync>;

/// worker 제출 작업
struct RefreshJob {
    id: u64,
    refresh: RefreshFn,
    kind: RefreshKind,
    /// Full 스케줄 세대. NewOnly 는 현재 세대 스냅샷(bump 없음).
    job_gen: u64,
}

/// live worker 공유 작업 큐 (결정 51)
struct JobQueue {
    inner: Mutex<JobQueueInner>,
    cv: Condvar,
}

struct JobQueueInner {
    jobs: VecDeque<RefreshJob>,
    closed: bool,
}

impl JobQueue {
    /// 빈 큐 생성
    fn new() -> Self {
        Self {
            inner: Mutex::new(JobQueueInner { jobs: VecDeque::new(), closed: false }),
            cv: Condvar::new(),
        }
    }

    /// 작업 제출 — 종료 후면 폐기
    fn push(&self, job: RefreshJob) {
        let mut inner = plock(&self.inner);
        if inner.closed {
            log::trace!("live-worker job discarded after close: id={}", job.id);
            return;
        }
        let id = job.id;
        inner.jobs.push_back(job);
        log::trace!("live-worker job queued: id={id}, depth={}", inner.jobs.len());
        drop(inner);
        self.cv.notify_one();
    }

    /// 다음 작업 대기. `None` = 큐 종료.
    fn pop(&self) -> Option<RefreshJob> {
        let mut inner = plock(&self.inner);
        loop {
            if let Some(job) = inner.jobs.pop_front() {
                return Some(job);
            }
            if inner.closed {
                return None;
            }
            inner = self.cv.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// 종료 — 대기 작업 폐기 후 worker 전원 깨움
    fn close(&self) {
        let mut inner = plock(&self.inner);
        inner.closed = true;
        let dropped = inner.jobs.len();
        inner.jobs.clear();
        drop(inner);
        if dropped > 0 {
            log::debug!("live-worker queue closed: dropped_jobs={dropped}");
        }
        self.cv.notify_all();
    }

    /// 현재 대기 작업 수 (metrics 스냅샷용)
    fn depth(&self) -> u64 {
        plock(&self.inner).jobs.len() as u64
    }
}

/// 구독 엔트리 — 타입 소거 재조회 클로저
struct SubEntry {
    /// 의존 테이블 (None = 미상 — UnknownDependencies 상태)
    tables: Option<HashSet<String>>,
    /// 명시 행 필터. 없으면 기존 테이블 단위 무효화다.
    /// 행 필터 목록 — `None` = 테이블 단위 무효화. 복수 시 OR 매칭 (결정 52).
    filter: Option<Vec<InvalidationFilter>>,
    /// 통합 풀 checkout 재조회 + 팬아웃 (worker 실행)
    refresh: RefreshFn,
    /// 재조회 세대 — Full 스케줄·rebind 시 증가, 스테일 결과 폐기
    epoch: Arc<AtomicU64>,
    /// DB 종료 통지 — 대기 중인 recv가 깨어나 Closed 에러를 받게 한다 (M-7)
    close: Box<dyn Fn() + Send + Sync>,
    /// observer별 무효화 debounce (결정 49) — 등록 시 DB 전역값, `.debounce` 로 override
    debounce: Duration,
    /// 고정 coalesce 창 만료 시각. 첫 무효화에만 설정, 창 안 추가 무효화는 연장 없이 병합.
    pending_due: Option<Instant>,
}

/// 무효화 트래커 (명세 §9.3) — 레지스트리 + 노티파이어 채널 + live worker 큐
pub(crate) struct Tracker {
    subs: Mutex<HashMap<u64, SubEntry>>,
    next_id: AtomicU64,
    tx: Sender<Msg>,
    /// DB 전역 debounce 기본값 (결정 49). 구독 등록 시 observer 기본으로 복사.
    default_debounce: Duration,
    jobs: Arc<JobQueue>,
    metrics: Arc<LiveMetricsState>,
    notifier_thread: Arc<std::sync::OnceLock<std::thread::ThreadId>>,
}

impl Tracker {
    /// 트래커 + notifier + live worker pool 기동 (결정 51).
    /// 전용 read-only 연결 없음 — worker 가 통합 풀에서 checkout.
    /// `default_debounce` 는 Builder `live_debounce` (미설정 시 [`DEFAULT_DEBOUNCE`]).
    /// join 핸들 반환 — DB drop 시 join (M-5). 스레드 생성 실패는 에러 (L-6).
    pub(crate) fn start(pool: Arc<ConnectionPool>, worker_count: usize, default_debounce: Duration) -> Result<(Arc<Tracker>, Vec<std::thread::JoinHandle<()>>)> {
        let workers = worker_count.max(1);
        let (tx, rx) = channel::<Msg>();
        let jobs = Arc::new(JobQueue::new());
        let metrics = Arc::new(LiveMetricsState::new());
        let tracker = Arc::new(Tracker {
            subs: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            tx,
            default_debounce,
            jobs: Arc::clone(&jobs),
            metrics: Arc::clone(&metrics),
            notifier_thread: Arc::new(std::sync::OnceLock::new()),
        });

        let mut joins = Vec::with_capacity(workers + 1);
        let t2 = Arc::clone(&tracker);
        let jobs_for_notifier = Arc::clone(&jobs);
        joins.push(std::thread::Builder::new().name("roomrs-notifier".into()).spawn(move || t2.notifier_loop(rx, jobs_for_notifier)).map_err(|e| Error::Internal(format!("노티파이어 스레드 생성 실패: {e}")))?);

        for i in 0..workers {
            let pool = Arc::clone(&pool);
            let jobs = Arc::clone(&jobs);
            let metrics = Arc::clone(&metrics);
            let name = format!("roomrs-live-worker-{i}");
            joins.push(std::thread::Builder::new().name(name.clone()).spawn(move || worker_loop(pool, jobs, metrics)).map_err(|e| Error::Internal(format!("live worker 스레드 생성 실패 ({name}): {e}")))?);
        }

        log::info!("live-query notifier started: workers={workers}, default_debounce_ms={}", default_debounce.as_millis());
        Ok((tracker, joins))
    }

    /// 읽기 전용 metrics 스냅샷 (명세 §9.5 P2).
    pub(crate) fn metrics_snapshot(&self) -> LiveMetrics {
        self.metrics.snapshot(self.jobs.depth())
    }

    /// refresh 클로저가 공유하는 metrics 핸들
    fn metrics_handle(&self) -> Arc<LiveMetricsState> {
        Arc::clone(&self.metrics)
    }

    /// 노티파이어 루프 — 수신·고정 coalesce 예약·만료 작업 제출 (결정 49/51). DB 재조회 없음.
    fn notifier_loop(&self, rx: Receiver<Msg>, jobs: Arc<JobQueue>) {
        let _ = self.notifier_thread.set(std::thread::current().id());
        log::debug!("live-query notifier loop entered");
        loop {
            // 가장 이른 pending 만료까지 대기 (없으면 블로킹 recv)
            let wait = {
                let subs = plock(&self.subs);
                let now = Instant::now();
                subs.values().filter_map(|e| e.pending_due).min().map(|due| due.saturating_duration_since(now))
            };

            let first = match wait {
                Some(d) if d.is_zero() => None,
                Some(d) => match rx.recv_timeout(d) {
                    Ok(m) => Some(m),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        log::debug!("live-query notifier channel disconnected");
                        jobs.close();
                        return;
                    }
                },
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => {
                        log::debug!("live-query notifier channel disconnected");
                        jobs.close();
                        return;
                    }
                },
            };

            let mut immediate_full: HashSet<u64> = HashSet::new();
            let mut immediate_new: HashSet<u64> = HashSet::new();
            let mut schedule_all = false;
            let mut schedule_tables: HashSet<String> = HashSet::new();
            let mut schedule_changes: Vec<TableChange> = Vec::new();
            let mut shutdown = false;

            if let Some(first_msg) = first {
                let mut msg = Some(first_msg);
                loop {
                    match msg.take() {
                        Some(Msg::Shutdown) => {
                            shutdown = true;
                            break;
                        }
                        Some(Msg::Invalidate(None)) => {
                            self.metrics.events_received.fetch_add(1, Ordering::Relaxed);
                            schedule_all = true;
                        }
                        Some(Msg::Invalidate(Some(ts))) => {
                            self.metrics.events_received.fetch_add(1, Ordering::Relaxed);
                            schedule_tables.extend(ts);
                        }
                        Some(Msg::Changes(cs)) => {
                            self.metrics.events_received.fetch_add(1, Ordering::Relaxed);
                            schedule_changes.extend(cs);
                        }
                        Some(Msg::Refresh(id)) => {
                            immediate_full.insert(id);
                        }
                        Some(Msg::RefreshNew(id)) => {
                            immediate_new.insert(id);
                        }
                        None => {}
                    }
                    if shutdown {
                        break;
                    }
                    match rx.try_recv() {
                        Ok(m) => msg = Some(m),
                        Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                    }
                }
            }

            if shutdown {
                log::info!("live-query notifier stopping");
                jobs.close();
                return;
            }

            // 무효화 영향 observer — 고정 coalesce: 첫 무효화만 창 시작, 추가분은 병합만
            if schedule_all || !schedule_tables.is_empty() || !schedule_changes.is_empty() {
                let now = Instant::now();
                let mut subs = plock(&self.subs);
                for (id, e) in subs.iter_mut() {
                    if e.tables.is_none() {
                        continue;
                    }
                    let table_match = e.tables.as_ref().is_some_and(|entry_tables| entry_tables.iter().any(|table| schedule_tables.iter().any(|t| t.eq_ignore_ascii_case(table))));
                    let change_match = e.tables.as_ref().is_some_and(|entry_tables| schedule_changes.iter().any(|change| e.filter.as_ref().map_or_else(|| entry_tables.iter().any(|table| table.eq_ignore_ascii_case(&change.table)), |filters| any_filter_matches(filters, change))));
                    if schedule_all || table_match || change_match {
                        if e.pending_due.is_none() {
                            let due = now + e.debounce;
                            e.pending_due = Some(due);
                            log::trace!("live query debounce scheduled: id={id}, delay_ms={}", e.debounce.as_millis());
                        } else {
                            self.metrics.coalesce_merged.fetch_add(1, Ordering::Relaxed);
                            log::trace!("live query debounce coalesced: id={id}");
                        }
                    }
                }
            }

            // 즉시 재조회 + 만료 pending → worker 제출 (notifier 는 실행하지 않음)
            let targets: Vec<RefreshJob> = {
                let now = Instant::now();
                let mut subs = plock(&self.subs);
                let mut out = Vec::new();
                for (id, e) in subs.iter_mut() {
                    if immediate_full.contains(id) {
                        e.pending_due = None;
                        let job_gen = e.epoch.fetch_add(1, Ordering::AcqRel) + 1;
                        out.push(RefreshJob {
                            id: *id,
                            refresh: Arc::clone(&e.refresh),
                            kind: RefreshKind::Full,
                            job_gen,
                        });
                        continue;
                    }
                    if immediate_new.contains(id) {
                        let job_gen = e.epoch.load(Ordering::Acquire);
                        out.push(RefreshJob {
                            id: *id,
                            refresh: Arc::clone(&e.refresh),
                            kind: RefreshKind::NewOnly,
                            job_gen,
                        });
                    }
                    if e.pending_due.is_some_and(|due| due <= now) {
                        e.pending_due = None;
                        log::trace!("live query debounce fired: id={id}");
                        let job_gen = e.epoch.fetch_add(1, Ordering::AcqRel) + 1;
                        out.push(RefreshJob {
                            id: *id,
                            refresh: Arc::clone(&e.refresh),
                            kind: RefreshKind::Full,
                            job_gen,
                        });
                    }
                }
                out
            };

            if !targets.is_empty() {
                log::trace!("live-query refresh jobs submitted: count={}", targets.len());
            }
            for job in targets {
                jobs.push(job);
            }
        }
    }

    /// 구독 등록 — id 반환. debounce 기본값은 DB 전역 `default_debounce` (결정 49).
    pub(crate) fn register(&self, tables: Option<HashSet<String>>, filter: Option<Vec<InvalidationFilter>>, refresh: RefreshFn, epoch: Arc<AtomicU64>, close: Box<dyn Fn() + Send + Sync>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let table_count = tables.as_ref().map_or(0, HashSet::len);
        let filtered = filter.is_some();
        let debounce = self.default_debounce;
        plock(&self.subs).insert(id, SubEntry { tables, filter, refresh, epoch, close, debounce, pending_due: None });
        log::debug!("live query registered: id={id}, tables={table_count}, filtered={filtered}, debounce_ms={}", debounce.as_millis());
        id
    }

    /// observer debounce 설정 (결정 49). `Duration::ZERO` = 무효화 즉시 재조회.
    /// 이미 열린 고정 창은 만료 시각을 바꾸지 않는다 (추가 무효화 병합과 동일).
    pub(crate) fn set_debounce(&self, id: u64, debounce: Duration) {
        let mut subs = plock(&self.subs);
        if let Some(e) = subs.get_mut(&id) {
            e.debounce = debounce;
            log::debug!("live query debounce set: id={id}, debounce_ms={}", debounce.as_millis());
        }
    }

    /// 구독 해제 — 이후 Full 스케줄 epoch 불일치로 in-flight 결과 폐기
    pub(crate) fn unregister(&self, id: u64) {
        let removed = {
            let mut subs = plock(&self.subs);
            if let Some(e) = subs.remove(&id) {
                // in-flight worker 결과 폐기
                e.epoch.fetch_add(1, Ordering::AcqRel);
                true
            } else {
                false
            }
        };
        log::debug!("live query unregistered: id={id}, removed={removed}");
    }

    /// 의존 테이블 갱신 (watching)
    pub(crate) fn set_tables(&self, id: u64, tables: HashSet<String>) {
        let table_count = tables.len();
        let updated = {
            let mut subs = plock(&self.subs);
            if let Some(e) = subs.get_mut(&id) {
                e.tables = Some(tables);
                true
            } else {
                false
            }
        };
        if updated {
            log::debug!("live query dependencies updated: id={id}, tables={table_count}");
        }
    }

    /// 무효화 방출 — commit 성공 후 호출 (명세 §9.2)
    pub(crate) fn invalidate(&self, tables: Option<HashSet<String>>) {
        log::debug!("invalidation emitted: tables={tables:?}");
        let _ = self.tx.send(Msg::Invalidate(tables));
    }

    /// preupdate hook 변경을 commit 성공 뒤 전달한다.
    pub(crate) fn invalidate_changes(&self, changes: Vec<TableChange>) {
        if !changes.is_empty() {
            log::trace!("preupdate changes emitted: count={}", changes.len());
            let _ = self.tx.send(Msg::Changes(changes));
        }
    }

    /// 특정 구독 재조회 요청 (초기 emit / rebind[C-8] / watching)
    pub(crate) fn request_refresh(&self, id: u64) {
        log::trace!("live query refresh requested: id={id}");
        let _ = self.tx.send(Msg::Refresh(id));
    }

    /// 새 콜백 전용 재조회 요청 — 기존 구독자 재-emit 없음 (L-7)
    pub(crate) fn request_refresh_new(&self, id: u64) {
        log::trace!("live query new-subscriber refresh requested: id={id}");
        let _ = self.tx.send(Msg::RefreshNew(id));
    }

    /// 종료 — 레지스트리 청산으로 각 구독 recv가 Closed를 받게 하고 (M-7)
    /// 노티파이어에 종료 신호를 보낸다
    pub(crate) fn shutdown(&self) {
        let entries: Vec<SubEntry> = plock(&self.subs).drain().map(|(_, e)| e).collect();
        for e in &entries {
            e.epoch.fetch_add(1, Ordering::AcqRel);
            (e.close)();
        }
        let _ = self.tx.send(Msg::Shutdown);
    }
}

/// live worker 루프 — 통합 풀 checkout 후 재조회, 즉시 반납 (결정 51)
fn worker_loop(pool: Arc<ConnectionPool>, jobs: Arc<JobQueue>, metrics: Arc<LiveMetricsState>) {
    let name = std::thread::current().name().unwrap_or("roomrs-live-worker").to_string();
    log::debug!("live-worker started: name={name}");
    while let Some(job) = jobs.pop() {
        let guard = match pool.acquire() {
            Ok(g) => g,
            Err(e) => {
                log::error!("live-worker pool checkout failed: name={name}, id={}, err={e}", job.id);
                metrics.refresh_err.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        let conn = match guard.conn() {
            Ok(c) => c,
            Err(e) => {
                log::error!("live-worker connection access failed: name={name}, id={}, err={e}", job.id);
                metrics.refresh_err.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (job.refresh)(conn, job.kind, job.job_gen))).is_err() {
            log::warn!("live query refresh panicked — isolated, worker continues: name={name}, id={}", job.id);
            metrics.refresh_err.fetch_add(1, Ordering::Relaxed);
        }
        // guard drop = 풀 반납
        drop(guard);
    }
    log::debug!("live-worker stopped: name={name}");
}

// ─────────────────── LiveQuery ───────────────────

/// 콜백 목록 타입 — (id, 새 콜백 여부[L-7 초기 전달용], 콜백)
type CallbackList<T> = Vec<(u64, bool, Box<dyn FnMut(T) + Send>)>;

/// callback 전달과 close 반환 사이 수명 동기화 상태.
struct DeliveryState {
    closed: bool,
    /// 중첩 deliver 깊이 (동일 스레드 재진입 포함)
    active: usize,
    /// 현재 deliver 를 수행 중인 스레드 — 콜백 안 close 대기 교착 방지 (H-3)
    owner: Option<std::thread::ThreadId>,
}

/// LiveQuery 공유 상태
struct SubShared<T> {
    /// recv/Iterator/Stream 공용 keep-latest 단일 슬롯
    value_slot: Mutex<Option<Result<T>>>,
    value_cv: Condvar,
    #[cfg(feature = "stream")]
    stream_waker: Mutex<Option<std::task::Waker>>,
    /// 콜백 목록 — deliver가 락 밖으로 체크아웃해 실행 (H-1/M-1 재진입 허용)
    callbacks: Mutex<CallbackList<T>>,
    delivery: Mutex<DeliveryState>,
    delivery_cv: Condvar,
    next_cb_id: AtomicU64,
    /// 지연 해지 목록 — 콜백 실행(체크아웃) 중 drop된 가드 반영용
    deferred_remove: Mutex<Vec<u64>>,
    /// 재조회 세대 — Full 스케줄·rebind·unregister 시 증가, 이전 세대 결과 폐기 (명세 §5.6, 결정 51)
    epoch: Arc<AtomicU64>,
    /// 미상 의존 상태 — 첫 recv에 UnknownDependencies 반환 (M-2 지연 통지)
    unknown_deps: AtomicBool,
    /// DB 종료 상태 — 이후 recv는 Closed (M-7)
    closed: AtomicBool,
    /// 마지막 emit 값 캐시 — 새 콜백 초기 전달용 (L-7)
    last_value: Mutex<Option<T>>,
}

impl<T: Clone> SubShared<T> {
    /// 미소비 값을 최신 결과로 덮어쓰고 대기자를 깨운다.
    fn publish(&self, value: Result<T>) {
        let mut slot = plock(&self.value_slot);
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        *slot = Some(value);
        drop(slot);
        self.value_cv.notify_all();
        #[cfg(feature = "stream")]
        if let Some(waker) = plock(&self.stream_waker).take() {
            waker.wake();
        }
    }

    /// Closed를 terminal 값으로 설치하고 이후 publish를 차단한다.
    fn close_terminal(&self, wait_callbacks: bool) {
        {
            let mut delivery = plock(&self.delivery);
            delivery.closed = true;
        }
        let mut slot = plock(&self.value_slot);
        self.closed.store(true, Ordering::Release);
        *slot = Some(Err(Error::Closed));
        drop(slot);
        self.value_cv.notify_all();
        #[cfg(feature = "stream")]
        if let Some(waker) = plock(&self.stream_waker).take() {
            waker.wake();
        }
        if wait_callbacks {
            let me = std::thread::current().id();
            let mut delivery = plock(&self.delivery);
            while delivery.active != 0 {
                // 자기 스레드가 deliver 중이면 대기 금지 — worker/notifier 콜백 안
                // Database drop → shutdown → close_terminal 교착 (H-3, 결정 51)
                if delivery.owner == Some(me) {
                    break;
                }
                delivery = self.delivery_cv.wait(delivery).unwrap_or_else(|e| e.into_inner());
            }
        }
    }
    /// 콜백 팬아웃 — 목록을 락 밖으로 체크아웃해 실행 (H-1/M-1:
    /// 콜백 내 subscribe/가드 drop 재진입 교착 방지)
    fn deliver(&self, v: &T, fresh_only: bool) {
        {
            let mut delivery = plock(&self.delivery);
            if delivery.closed {
                return;
            }
            delivery.active += 1;
            delivery.owner = Some(std::thread::current().id());
        }
        let mut cbs: CallbackList<T> = {
            let mut g = plock(&self.callbacks);
            let deferred = std::mem::take(&mut *plock(&self.deferred_remove));
            g.retain(|(id, _, _)| !deferred.contains(id));
            std::mem::take(&mut *g)
        };
        for (id, fresh, cb) in cbs.iter_mut() {
            if fresh_only && !*fresh {
                continue;
            }
            // 호출 직전 지연 해지 재확인 — 체크아웃 중 다른 스레드에서 drop된
            // 가드의 콜백을 스킵한다 (M-4). 락은 확인 즉시 놓는다 — 콜백 실행 중
            // 락 보유 없음(재진입 교착 없음)
            if plock(&self.deferred_remove).contains(id) {
                continue;
            }
            *fresh = false;
            // 콜백 panic 은 live worker/notifier 를 죽이지 않는다 (H-4)
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(v.clone()))).is_err() {
                log::warn!("live query callback panicked — isolated, other callbacks continue");
            }
        }
        // 체크아웃 복귀 — 실행 중 등록된 콜백은 뒤에 붙이고 해지분은 제거
        let mut g = plock(&self.callbacks);
        let newly = std::mem::take(&mut *g);
        cbs.extend(newly);
        let deferred = std::mem::take(&mut *plock(&self.deferred_remove));
        cbs.retain(|(id, _, _)| !deferred.contains(id));
        *g = cbs;
        let mut delivery = plock(&self.delivery);
        delivery.active = delivery.active.saturating_sub(1);
        if delivery.active == 0 {
            delivery.owner = None;
            self.delivery_cv.notify_all();
        }
    }
}

/// UnknownDependencies 에러 생성 (M-2 지연 통지)
fn unknown_deps_err() -> Error {
    Error::UnknownDependencies("쿼리의 의존 테이블을 추출하지 못했습니다 — .watching(&[…]) 필요".into())
}

/// 결과가 SQLITE_LOCKED(공유 캐시 테이블 락)인지 — 재시도 판단 (M-6)
fn is_table_locked<T>(out: &Result<T>) -> bool {
    matches!(
        out,
        Err(Error::Sqlite(rusqlite::Error::SqliteFailure(fe, _)))
            if fe.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

/// 라이브 쿼리 — 단일 구체 타입 (명세 §5.6).
/// 의존 테이블 write 시 자동 재조회 emit. drop = 구독 해제.
/// `recv`/`recv_timeout`은 호출 스레드를 블로킹한다. async에서는 `into_stream`을 쓴다.
/// 마지막 DB 핸들 drop은 notifier·live worker 종료까지 join할 수 있다.
pub struct LiveQuery<T> {
    id: u64,
    tracker: Arc<Tracker>,
    shared: Arc<SubShared<T>>,
    params: Arc<Mutex<OwnedParams>>,
}

impl<T: Clone + Send + 'static> LiveQuery<T> {
    /// 내부 생성 — watch_* 전용
    pub(crate) fn new(tracker: Arc<Tracker>, sql: String, params: OwnedParams, tables: Option<HashSet<String>>, run: impl Fn(&Connection, &str, &OwnedParams) -> Result<T> + Send + Sync + 'static) -> Self {
        Self::new_filtered(tracker, sql, params, tables, None, run)
    }

    /// 내부 생성 — 명시 행 필터를 가진 watch_* 전용.
    pub(crate) fn new_filtered(tracker: Arc<Tracker>, sql: String, params: OwnedParams, tables: Option<HashSet<String>>, filter: Option<Vec<InvalidationFilter>>, run: impl Fn(&Connection, &str, &OwnedParams) -> Result<T> + Send + Sync + 'static) -> Self {
        let params = Arc::new(Mutex::new(params));
        let unknown = tables.is_none();
        let validate_names = tables.clone();
        let validate_pending = Arc::new(AtomicBool::new(validate_names.is_some()));
        let epoch = Arc::new(AtomicU64::new(0));
        let shared = Arc::new(SubShared {
            value_slot: Mutex::new(None),
            value_cv: Condvar::new(),
            #[cfg(feature = "stream")]
            stream_waker: Mutex::new(None),
            callbacks: Mutex::new(Vec::new()),
            delivery: Mutex::new(DeliveryState { closed: false, active: 0, owner: None }),
            delivery_cv: Condvar::new(),
            next_cb_id: AtomicU64::new(1),
            deferred_remove: Mutex::new(Vec::new()),
            epoch: Arc::clone(&epoch),
            unknown_deps: AtomicBool::new(unknown),
            closed: AtomicBool::new(false),
            last_value: Mutex::new(None),
        });

        // 타입 소거 재조회 클로저 — live worker 가 통합 풀 checkout 후 실행 (결정 51)
        let refresh: RefreshFn = {
            let shared = Arc::clone(&shared);
            let params = Arc::clone(&params);
            let validate_pending = Arc::clone(&validate_pending);
            let metrics = tracker.metrics_handle();
            Arc::new(move |conn: &Connection, kind: RefreshKind, job_gen: u64| {
                // 새 콜백 전용 경로 — 캐시 값 전달, 기존 구독자 재-emit 없음 (L-7)
                if kind == RefreshKind::NewOnly {
                    let cached = plock(&shared.last_value).clone();
                    if let Some(v) = cached {
                        shared.deliver(&v, true);
                        return;
                    }
                    // 캐시 없음(초기 emit 전) — 전체 재조회로 폴백
                }
                // 추출 이름이 view·미존재 객체면 기저 테이블을 알 수 없다.
                // 첫 조회에서 UnknownDependencies를 전달하고 watching() 명시를 기다린다.
                if validate_pending.swap(false, Ordering::AcqRel) {
                    if let Some(names) = &validate_names {
                        let all_tables = names.iter().all(|name| {
                            conn.query_row(
                                "SELECT count(*) FROM sqlite_master \
                                 WHERE type='table' AND name=?1 COLLATE NOCASE",
                                [name],
                                |row| row.get::<_, i64>(0),
                            ) == Ok(1)
                        });
                        if !all_tables {
                            metrics.refresh_err.fetch_add(1, Ordering::Relaxed);
                            shared.publish(Err(unknown_deps_err()));
                            return;
                        }
                    }
                }
                let p = plock(&params).clone();
                let mut out = run(conn, &sql, &p);
                // 공유 캐시 인메모리의 SQLITE_LOCKED는 busy 핸들러가 개입하지
                // 않는다 — 짧게 대기 후 1회 재시도 (M-6)
                if is_table_locked(&out) {
                    log::warn!("SQLITE_LOCKED during live refresh — retrying once");
                    std::thread::sleep(Duration::from_millis(10));
                    out = run(conn, &sql, &p);
                }
                // 스테일 폐기 — 이후 Full 스케줄·rebind·unregister 가 세대를 올렸으면 버림
                if shared.epoch.load(Ordering::Acquire) != job_gen {
                    metrics.stale_discarded.fetch_add(1, Ordering::Relaxed);
                    log::trace!("live query stale refresh discarded: job_gen={job_gen}");
                    return;
                }
                // recv 슬롯을 먼저 갱신한다. 콜백은 실행 중 다른 스레드가 즉시
                // try_recv할 수 있으므로, 콜백보다 앞서 최신값을 관측 가능하게 한다.
                match out {
                    Ok(v) => {
                        metrics.refresh_ok.fetch_add(1, Ordering::Relaxed);
                        *plock(&shared.last_value) = Some(v.clone());
                        shared.publish(Ok(v.clone()));
                        shared.deliver(&v, false);
                    }
                    Err(e) => {
                        metrics.refresh_err.fetch_add(1, Ordering::Relaxed);
                        // 재시도 후에도 실패 — 에러는 구독자에게 전달되지만 로그도 남긴다
                        log::error!("live query refresh failed: {e}");
                        shared.publish(Err(e));
                    }
                }
            })
        };

        // 종료 통지 클로저 — DB drop 시 recv가 Closed를 받게 한다 (M-7)
        let close: Box<dyn Fn() + Send + Sync> = {
            let shared = Arc::clone(&shared);
            let notifier_thread = Arc::clone(&tracker.notifier_thread);
            Box::new(move || {
                let on_notifier = notifier_thread.get() == Some(&std::thread::current().id());
                shared.close_terminal(!on_notifier);
            })
        };

        let id = tracker.register(tables, filter, refresh, Arc::clone(&epoch), close);
        let lq = LiveQuery { id, tracker, shared, params };

        if !unknown {
            // 구독 즉시 1회 emit (명세 §9.1) — 노티파이어 경유로 순차성 보장.
            // 의존 미상이면 통지를 미룬다 — watching() 체이닝이 상태를 해소하면
            // 스테일 에러 없이 첫 값이 emit된다 (M-2)
            lq.tracker.request_refresh(lq.id);
        }
        lq
    }

    /// 수신 전 공통 상태 검사 — 미상 의존 1회 통지 (M-2)
    fn take_unknown_deps(&self) -> bool {
        self.shared.unknown_deps.swap(false, Ordering::AcqRel)
    }

    /// 블로킹 수신 — 다음 emit까지 대기
    ///
    /// Shutdown is terminal: an in-flight refresh cannot overwrite `Closed`,
    /// and no value is observed after `Err(Error::Closed)`.
    pub fn recv(&self) -> Result<T> {
        if self.take_unknown_deps() {
            return Err(unknown_deps_err());
        }
        let mut slot = plock(&self.shared.value_slot);
        loop {
            if let Some(value) = slot.take() {
                return value;
            }
            if self.shared.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            slot = self.shared.value_cv.wait(slot).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// 타임아웃 수신 — 없으면 Ok(None)
    pub fn recv_timeout(&self, d: Duration) -> Result<Option<T>> {
        if self.take_unknown_deps() {
            return Err(unknown_deps_err());
        }
        let mut slot = plock(&self.shared.value_slot);
        if let Some(value) = slot.take() {
            return value.map(Some);
        }
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let (mut slot, _) = self.shared.value_cv.wait_timeout_while(slot, d, |slot| slot.is_none() && !self.shared.closed.load(Ordering::Acquire)).unwrap_or_else(|e| e.into_inner());
        if let Some(value) = slot.take() {
            value.map(Some)
        } else if self.shared.closed.load(Ordering::Acquire) {
            Err(Error::Closed)
        } else {
            Ok(None)
        }
    }

    /// 논블로킹 수신
    pub fn try_recv(&self) -> Result<Option<T>> {
        if self.take_unknown_deps() {
            return Err(unknown_deps_err());
        }
        if let Some(value) = plock(&self.shared.value_slot).take() {
            value.map(Some)
        } else if self.shared.closed.load(Ordering::Acquire) {
            Err(Error::Closed)
        } else {
            Ok(None)
        }
    }

    /// 무한 이터레이터 — emit마다 1건.
    ///
    /// Fused on shutdown (L-4): after yielding one `Err(Error::Closed)` the
    /// iterator returns `None` instead of repeating the error forever.
    pub fn iter(&self) -> impl Iterator<Item = Result<T>> + '_ {
        let mut closed = false;
        std::iter::from_fn(move || {
            if closed {
                return None;
            }
            let v = self.recv();
            if matches!(v, Err(Error::Closed)) {
                closed = true;
            }
            Some(v)
        })
    }

    /// 콜백 구독 — 노티파이어 스레드에서 호출 (명세 §5.6).
    /// 반환 가드 drop = 해지. `let _ = q.subscribe(…)`는 즉시 해지됨[C-7] — 가드를 보관할 것.
    ///
    /// Delivery contract (M-4): after dropping the returned guard from another
    /// thread, at most one in-flight notification may still be delivered to
    /// the callback.
    #[must_use = "가드를 버리면 구독이 즉시 해지됩니다 (명세 C-7)"]
    pub fn subscribe(&self, f: impl FnMut(T) + Send + 'static) -> SubscriptionGuard<T> {
        let cb_id = self.shared.next_cb_id.fetch_add(1, Ordering::Relaxed);
        plock(&self.shared.callbacks).push((cb_id, true, Box::new(f)));
        log::debug!("live query callback subscribed: query_id={}, callback_id={cb_id}", self.id);
        // 새 콜백에만 현재 값 전달 — 기존 구독자 재-emit 없음 (L-7)
        self.tracker.request_refresh_new(self.id);
        SubscriptionGuard { shared: Arc::clone(&self.shared), cb_id, detached: false }
    }

    /// 같은 SQL, 바인딩 교체 (명세 §5.6b) — 재조회는 live worker 라우팅[C-8]
    pub fn rebind(&self, params: &[&dyn ToSql]) -> Result<()> {
        let owned = OwnedParams::from_dyn(params)?;
        *plock(&self.params) = owned;
        // epoch 증가 — 진행 중 재조회 결과 폐기, 이전 바인딩 캐시도 폐기 (L-7 보완)
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        *plock(&self.shared.last_value) = None;
        log::debug!("live query parameters rebound: id={}, parameter_count={}", self.id, params.len());
        self.tracker.request_refresh(self.id);
        Ok(())
    }

    /// 의존 테이블 명시 — 직접 쿼리의 추출 실패 해소 (명세 §5.7).
    /// 미상 의존 상태를 지우므로 첫 recv는 스테일 에러 없이 값을 받는다 (M-2)
    pub fn watching(self, tables: &[&str]) -> Self {
        let set: HashSet<String> = tables.iter().map(|s| s.to_string()).collect();
        self.tracker.set_tables(self.id, set);
        self.shared.unknown_deps.store(false, Ordering::Release);
        log::debug!("live query explicit dependencies set: id={}, tables={}", self.id, tables.len());
        self.tracker.request_refresh(self.id);
        self
    }

    /// Sets the per-observer invalidation debounce window (decision 49).
    ///
    /// Overrides the DB-wide default from
    /// [`crate::DatabaseBuilder::live_debounce`] (which itself defaults to
    /// [`DEFAULT_DEBOUNCE`] / 250ms). The window starts on the first
    /// invalidation and does not slide when more invalidations arrive inside
    /// the same window — they are coalesced only. `Duration::ZERO` refreshes
    /// on the next notifier turn. Initial subscription emit, `rebind`, and
    /// `watching` still refresh immediately.
    pub fn debounce(self, delay: Duration) -> Self {
        self.tracker.set_debounce(self.id, delay);
        self
    }
}

impl<T> Drop for LiveQuery<T> {
    /// 구독 해제 — 이후 emit 0 (명세 §5.6 수명 계약)
    fn drop(&mut self) {
        self.tracker.unregister(self.id);
    }
}

#[cfg(feature = "stream")]
impl<T: Clone + Send + 'static> LiveQuery<T> {
    /// 비동기 Stream 소비 (명세 §5.6, feature `async`) — 런타임 무관.
    /// 별도 스레드 없이 keep-latest 슬롯을 직접 poll한다.
    pub fn into_stream(self) -> impl futures_core::Stream<Item = Result<T>> + Send {
        LiveStream { query: self }
    }
}

/// LiveQuery 단일 슬롯을 직접 poll하는 런타임 무관 Stream.
#[cfg(feature = "stream")]
struct LiveStream<T> {
    query: LiveQuery<T>,
}

#[cfg(feature = "stream")]
impl<T: Clone + Send + 'static> futures_core::Stream for LiveStream<T> {
    type Item = Result<T>;

    /// 최신 슬롯을 소비하고 빈 슬롯이면 publish wake를 등록한다.
    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.query.try_recv() {
            Ok(Some(value)) => std::task::Poll::Ready(Some(Ok(value))),
            Err(Error::Closed) => std::task::Poll::Ready(None),
            Err(error) => std::task::Poll::Ready(Some(Err(error))),
            Ok(None) => {
                *plock(&this.query.shared.stream_waker) = Some(cx.waker().clone());
                match this.query.try_recv() {
                    Ok(Some(value)) => std::task::Poll::Ready(Some(Ok(value))),
                    Err(Error::Closed) => std::task::Poll::Ready(None),
                    Err(error) => std::task::Poll::Ready(Some(Err(error))),
                    Ok(None) => std::task::Poll::Pending,
                }
            }
        }
    }
}

/// 콜백 구독 가드 — drop = 해지
///
/// Delivery contract (M-4): after `drop` completes on another thread, at most
/// one notification that was already in flight on the notifier thread may
/// still be delivered to the callback.
pub struct SubscriptionGuard<T> {
    shared: Arc<SubShared<T>>,
    cb_id: u64,
    detached: bool,
}

impl<T> SubscriptionGuard<T> {
    /// 앱 수명 구독 — 가드 없이 유지 (명세 §5.6b)
    pub fn detach(mut self) {
        self.detached = true;
    }
}

impl<T> Drop for SubscriptionGuard<T> {
    /// 해지 (명세 §5.6 수명 계약) —
    /// 목록에서 즉시 제거하고, deliver가 목록을 체크아웃해 실행 중인 경우를 대비해
    /// 지연 해지 목록에도 기록한다(복귀 시 제거). 콜백 내 self-drop도 같은 경로 —
    /// deliver는 락을 잡지 않고 실행하므로 교착 없음 (H-1/H-4)
    fn drop(&mut self) {
        if self.detached {
            return;
        }
        plock(&self.shared.callbacks).retain(|(id, _, _)| *id != self.cb_id);
        plock(&self.shared.deferred_remove).push(self.cb_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// close 이후 in-flight refresh publish가 terminal Closed를 덮어쓰지 못한다.
    #[test]
    fn close_is_terminal_against_late_publish() {
        let shared = SubShared {
            value_slot: Mutex::new(None),
            value_cv: Condvar::new(),
            #[cfg(feature = "stream")]
            stream_waker: Mutex::new(None),
            callbacks: Mutex::new(Vec::new()),
            delivery: Mutex::new(DeliveryState { closed: false, active: 0, owner: None }),
            delivery_cv: Condvar::new(),
            next_cb_id: AtomicU64::new(1),
            deferred_remove: Mutex::new(Vec::new()),
            epoch: Arc::new(AtomicU64::new(0)),
            unknown_deps: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            last_value: Mutex::new(None::<i64>),
        };
        shared.close_terminal(true);
        shared.publish(Ok(99));
        assert!(matches!(plock(&shared.value_slot).take(), Some(Err(Error::Closed))));
    }

    /// 외부 close는 실행 중 callback 종료를 기다리고 반환 뒤 새 callback을 막는다.
    #[test]
    fn close_waits_for_in_flight_callback() {
        let shared = Arc::new(SubShared {
            value_slot: Mutex::new(None),
            value_cv: Condvar::new(),
            #[cfg(feature = "stream")]
            stream_waker: Mutex::new(None),
            callbacks: Mutex::new(Vec::new()),
            delivery: Mutex::new(DeliveryState { closed: false, active: 0, owner: None }),
            delivery_cv: Condvar::new(),
            next_cb_id: AtomicU64::new(1),
            deferred_remove: Mutex::new(Vec::new()),
            epoch: Arc::new(AtomicU64::new(0)),
            unknown_deps: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            last_value: Mutex::new(None::<i64>),
        });
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        plock(&shared.callbacks).push((
            1,
            false,
            Box::new(move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }),
        ));
        std::thread::scope(|scope| {
            let delivering = Arc::clone(&shared);
            scope.spawn(move || delivering.deliver(&1, false));
            entered_rx.recv().unwrap();

            let closing = Arc::clone(&shared);
            let (closed_tx, closed_rx) = std::sync::mpsc::channel();
            scope.spawn(move || {
                closing.close_terminal(true);
                closed_tx.send(()).unwrap();
            });
            assert!(closed_rx.recv_timeout(Duration::from_millis(20)).is_err());
            release_tx.send(()).unwrap();
            closed_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        });
        shared.deliver(&2, false);
        assert_eq!(plock(&shared.delivery).active, 0);
    }

    /// CTE-write(WITH … INSERT/UPDATE)는 ReadOnly가 아닌 Unknown으로 분류 (R2-1)
    #[test]
    fn cte_write_classified_unknown() {
        let w = extract_write_tables("WITH x AS (SELECT 1 AS a) INSERT INTO t (a) SELECT a FROM x");
        assert!(matches!(w, WriteTables::Unknown), "CTE-INSERT = Unknown");

        // CTE-UPDATE — 파싱 결과가 Query(body=Update)든 파싱 실패든 Unknown이어야 한다
        let u = extract_write_tables("WITH x AS (SELECT 1 AS a) UPDATE t SET a = 1");
        assert!(matches!(u, WriteTables::Unknown), "CTE-UPDATE = Unknown");

        // 순수 읽기 CTE는 여전히 ReadOnly — 방출 없음 (L-2)
        let r = extract_write_tables("WITH x AS (SELECT 1 AS a) SELECT a FROM x");
        assert!(matches!(r, WriteTables::ReadOnly), "읽기 CTE = ReadOnly");
    }

    /// sqlparser가 거부하는 SQLite 전용 SELECT도 write로 오인하지 않는다.
    #[test]
    fn sqlite_select_parse_failure_is_read_only() {
        let result = extract_write_tables("SELECT * FROM t INDEXED BY idx_t");
        assert!(matches!(result, WriteTables::ReadOnly));

        let commented = extract_write_tables("-- 조회\nSELECT * FROM t INDEXED BY idx_t");
        assert!(matches!(commented, WriteTables::ReadOnly));
    }

    /// 내부 SQLite 확장 때문에 파싱 실패한 EXPLAIN도 실행 없는 읽기로 분류한다.
    #[test]
    fn sqlite_explain_parse_failure_is_read_only() {
        let result = extract_write_tables("EXPLAIN SELECT * FROM t INDEXED BY idx_t");
        assert!(matches!(result, WriteTables::ReadOnly));

        let write = extract_write_tables("EXPLAIN INSERT OR CUSTOM INTO t VALUES (1)");
        assert!(matches!(write, WriteTables::ReadOnly));
    }

    /// 파싱 실패 write는 계속 전체 무효화 대상으로 보수 분류한다.
    #[test]
    fn malformed_write_stays_unknown() {
        let result = extract_write_tables("INSERT OR CUSTOM INTO t VALUES (1)");
        assert!(matches!(result, WriteTables::Unknown));
    }

    /// 상태를 바꾸는 PRAGMA와 읽기 PRAGMA 모두 보수적으로 전체 무효화한다.
    #[test]
    fn pragma_stays_unknown() {
        assert!(matches!(extract_write_tables("PRAGMA user_version = 2"), WriteTables::Unknown));
        assert!(matches!(extract_write_tables("PRAGMA user_version"), WriteTables::Unknown));
    }

    /// EXPLAIN은 포함된 write를 실행하지 않으므로 읽기 전용이다.
    #[test]
    fn explain_write_is_read_only() {
        assert!(matches!(extract_write_tables("EXPLAIN INSERT INTO t VALUES (1)"), WriteTables::ReadOnly));
    }

    /// 다중문에 write가 하나라도 있으면 해당 테이블을 무효화한다.
    #[test]
    fn multi_statement_with_write_collects_table() {
        let WriteTables::Tables(tables) = extract_write_tables("SELECT 1; INSERT INTO t VALUES (1)") else {
            panic!("SELECT + INSERT는 Tables여야 함");
        };
        assert_eq!(tables, HashSet::from(["t".to_owned()]));
    }
}
