//! Database · 빌더 · 스키마 정의 (명세 §5.4, §10)

use crate::error::{Error, Result};
use crate::handle::SyncHandle;
use crate::pool::{ConnectionPool, Pool};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 콜백 타입 별칭
pub(crate) type ConnCallback = Arc<dyn Fn(&Connection) -> Result<()> + Send + Sync>;
type QueryLogger = Box<dyn Fn(&str, Duration) + Send + Sync>;

#[cfg(feature = "live")]
thread_local! {
    /// 현재 스레드의 중첩 SQL 실행별 preupdate_hook 수집 버퍼.
    static HOOK_CAPTURES: std::cell::RefCell<Vec<HookCaptureFrame>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// preupdate_hook 수집 프레임.
#[cfg(feature = "live")]
#[derive(Default)]
pub(crate) struct HookCaptureFrame {
    pub(crate) tables: std::collections::HashSet<String>,
    pub(crate) changes: Vec<crate::live::TableChange>,
}

/// 현재 실행 중인 가장 안쪽 SQL의 행 변경을 기록한다.
#[cfg(feature = "live")]
pub(crate) fn record_hook_change(change: crate::live::TableChange) {
    HOOK_CAPTURES.with(|captures| {
        if let Some(current) = captures.borrow_mut().last_mut() {
            current.tables.insert(change.table.clone());
            current.changes.push(change);
        }
    });
}

/// preupdate_hook capture 프레임을 unwind에서도 제거하는 RAII guard.
#[cfg(feature = "live")]
pub(crate) struct HookCapture {
    depth: usize,
}

#[cfg(feature = "live")]
impl Drop for HookCapture {
    /// 아직 남은 현재 프레임과 그 아래 비정상 중첩 프레임을 제거한다.
    fn drop(&mut self) {
        HOOK_CAPTURES.with(|captures| captures.borrow_mut().truncate(self.depth));
    }
}

/// 스키마 컬럼 메타를 사용해 preupdate hook을 설치한다.
#[cfg(feature = "live")]
pub(crate) fn install_preupdate_hook(
    conn: &Connection,
    columns: Arc<std::collections::HashMap<String, Vec<String>>>,
) -> Result<()> {
    use rusqlite::hooks::PreUpdateCase;
    use rusqlite::types::Value;

    conn.preupdate_hook(Some(
        move |_action, _db: &str, table: &str, change: &PreUpdateCase| {
            let Some(column_names) = columns.get(&table.to_ascii_lowercase()) else {
                return;
            };
            let read_old = |accessor: &rusqlite::hooks::PreUpdateOldValueAccessor| {
                let mut row = std::collections::HashMap::new();
                for (index, name) in column_names.iter().enumerate() {
                    let Ok(value) = accessor.get_old_column_value(index as i32) else {
                        return None;
                    };
                    row.insert(name.clone(), Value::from(value));
                }
                Some(row)
            };
            let read_new = |accessor: &rusqlite::hooks::PreUpdateNewValueAccessor| {
                let mut row = std::collections::HashMap::new();
                for (index, name) in column_names.iter().enumerate() {
                    let Ok(value) = accessor.get_new_column_value(index as i32) else {
                        return None;
                    };
                    row.insert(name.clone(), Value::from(value));
                }
                Some(row)
            };
            let (old, new) = match change {
                PreUpdateCase::Insert(new) => (None, read_new(new)),
                PreUpdateCase::Delete(old) => (read_old(old), None),
                PreUpdateCase::Update {
                    old_value_accessor,
                    new_value_accessor,
                } => (read_old(old_value_accessor), read_new(new_value_accessor)),
                PreUpdateCase::Unknown => return,
            };
            if old.is_some() || new.is_some() {
                record_hook_change(crate::live::TableChange {
                    table: table.to_string(),
                    old,
                    new,
                });
            }
        },
    ))?;
    Ok(())
}

/// pool 재오픈에 필요한 connection 로컬 설정 소유본.
#[derive(Clone)]
struct ConnectionSettings {
    path: Option<PathBuf>,
    mem_name: Option<String>,
    busy_timeout: Duration,
    on_open: Option<ConnCallback>,
    #[cfg(feature = "cipher")]
    encryption_key: Option<String>,
}

impl ConnectionSettings {
    /// on_open과 반환 불변식을 적용한다.
    fn initialize(&self, conn: &Connection) -> Result<()> {
        if let Some(cb) = &self.on_open {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(conn)))
                .map_err(|_| Error::Internal("on_open 콜백 panic".into()))
                .and_then(|result| result);
            if !conn.is_autocommit() {
                conn.execute_batch("ROLLBACK")?;
            }
            conn.pragma_update(None, "query_only", "OFF")?;
            if self.mem_name.is_some() {
                conn.pragma_update(None, "read_uncommitted", "ON")?;
            }
            result?;
        }
        Ok(())
    }

    /// 새 connection을 열고 공통 PRAGMA·선택 callback을 적용한다.
    fn open(&self, initialize: bool) -> Result<Connection> {
        let conn = match (&self.mem_name, &self.path) {
            (Some(uri), _) => {
                use rusqlite::OpenFlags;
                Connection::open_with_flags(
                    uri,
                    OpenFlags::SQLITE_OPEN_READ_WRITE
                        | OpenFlags::SQLITE_OPEN_CREATE
                        | OpenFlags::SQLITE_OPEN_URI
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?
            }
            (None, Some(path)) => Connection::open(path)?,
            (None, None) => return Err(Error::Config("DB 경로가 설정되지 않았습니다".into())),
        };
        #[cfg(feature = "cipher")]
        if let Some(key) = &self.encryption_key {
            conn.pragma_update(None, "key", key)?;
        }
        conn.busy_timeout(self.busy_timeout)?;
        if self.mem_name.is_none() {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "query_only", "OFF")?;
        if self.mem_name.is_some() {
            conn.pragma_update(None, "read_uncommitted", "ON")?;
        }
        if initialize {
            self.initialize(&conn)?;
        }
        Ok(conn)
    }
}

/// SQL 식별자 이스케이프 — `"` 배증 후 따옴표로 감싼다 (M-8/M-9)
pub(crate) fn escape_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 컬럼 메타 — `#[entity]` 생성, 스냅샷 대조·생성에 사용 (명세 §7)
#[derive(Debug, Clone, Copy)]
pub struct ColumnMeta {
    pub name: &'static str,
    /// SQLite 타입 (빈 문자열 = typeless)
    pub sql_type: &'static str,
    pub not_null: bool,
    pub pk: bool,
    /// rename 힌트 (명세 §8.3) — diff 초안 전용
    pub renamed_from: Option<&'static str>,
}

/// 테이블 메타 — `#[database]`가 엔티티에서 수집
pub struct TableMeta {
    pub name: &'static str,
    pub columns: &'static [ColumnMeta],
    /// 테이블·인덱스 DDL
    pub ddl: &'static [&'static str],
}

/// 테이블 스키마 정의 — `#[database]` 매크로가 엔티티 메타에서 구성
pub struct SchemaDef {
    /// 스키마 버전 (`#[database(version = N)]`)
    pub version: u32,
    /// 테이블·인덱스 DDL (실행 순서대로)
    pub ddl: Vec<&'static str>,
    /// 테이블·컬럼 메타 — 스냅샷 생성·해시 대조용 (명세 §7.4)
    pub tables: Vec<TableMeta>,
}

impl SchemaDef {
    /// 같은 SQLite 테이블 이름을 가리키는 엔티티 중복을 검증한다.
    fn validate_unique_tables(&self) -> Result<()> {
        for (index, table) in self.tables.iter().enumerate() {
            if self.tables[..index]
                .iter()
                .any(|previous| previous.name.eq_ignore_ascii_case(table.name))
            {
                return Err(Error::Config(format!(
                    "database entities에 SQLite 테이블 이름 중복: {}",
                    table.name
                )));
            }
        }
        Ok(())
    }

    /// 엔티티 메타 → 스냅샷 변환 (매크로·런타임 공유 모델, 명세 §3)
    pub fn to_snapshot(&self) -> roomrs_migrate::SchemaSnapshot {
        roomrs_migrate::SchemaSnapshot {
            version: self.version,
            tables: self
                .tables
                .iter()
                .map(|t| roomrs_migrate::TableSnapshot {
                    name: t.name.to_string(),
                    columns: t
                        .columns
                        .iter()
                        .map(|c| roomrs_migrate::ColumnSnapshot {
                            name: c.name.to_string(),
                            sql_type: c.sql_type.to_string(),
                            not_null: c.not_null,
                            pk: c.pk,
                            renamed_from: c.renamed_from.map(str::to_string),
                        })
                        .collect(),
                    ddl: t.ddl.iter().map(|d| d.to_string()).collect(),
                })
                .collect(),
        }
    }
}

/// Compile-time embedded schema snapshot (spec §8.4, decision 21c).
///
/// `#[database]` reads every committed snapshot file
/// (`migrations/schema/{db}.{version}.json`), compresses it with
/// miniz_oxide and embeds it into the binary. The full set is exposed via
/// [`DatabaseSpec::EMBEDDED_SCHEMAS`] in ascending version order.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedSchema {
    /// Schema version of this snapshot.
    pub version: u32,
    /// Compressed snapshot JSON (raw deflate — see
    /// `roomrs_migrate::compress_snapshot`).
    pub compressed: &'static [u8],
}

impl EmbeddedSchema {
    /// Decompress and parse the embedded snapshot.
    ///
    /// Fails with [`Error::Migration`] when the embedded blob is corrupt.
    pub fn snapshot(&self) -> Result<roomrs_migrate::SchemaSnapshot> {
        let raw = roomrs_migrate::decompress_snapshot(self.compressed).map_err(|e| {
            Error::Migration(format!(
                "내장 스냅샷(v{}) 압축 해제 실패: {e}",
                self.version
            ))
        })?;
        roomrs_migrate::SchemaSnapshot::from_slice(&raw)
            .map_err(|e| Error::Migration(format!("내장 스냅샷(v{}) 파스 실패: {e}", self.version)))
    }
}

/// `#[database]` 생성물이 구현하는 스펙 trait —
/// core 빌더가 타입드 DB를 돌려줄 수 있게 한다
pub trait DatabaseSpec: Sized {
    /// 스키마 버전
    const VERSION: u32;
    /// Snake-case database name — snapshot files are named
    /// `{DB_NAME}.{version}.json` (spec §7.2, decision 21). The
    /// `#[database]` macro derives this from the struct identifier.
    const DB_NAME: &'static str;
    /// 컴파일 타임에 읽은 현재 버전 스냅샷 파일 해시 — 파일 부재 시 None (명세 §7.4b)
    const SNAPSHOT_HASH: Option<u64> = None;
    /// Embedded snapshots in ascending version order (spec §8.4,
    /// decision 21c). Filled in by the `#[database]` macro; empty when no
    /// snapshot files exist.
    const EMBEDDED_SCHEMAS: &'static [EmbeddedSchema] = &[];
    /// Whether the current-version snapshot file existed when the
    /// `#[database]` macro expanded (decision 28, D-3b). Defaults to `true`
    /// so manual `DatabaseSpec` impls are unaffected. When `false`, the
    /// export test keeps failing even after the file exists and matches,
    /// until a rebuild re-expands the macro and embeds the snapshot —
    /// closing the fail-open window where `SNAPSHOT_HASH` and the embedded
    /// chain silently stay stale.
    const SNAPSHOT_FILE_SEEN: bool = true;
    /// 엔티티들의 DDL 수집
    fn schema() -> SchemaDef;
    /// core Database를 감싸 사용자 DB 타입 생성
    fn from_database(db: Database) -> Self;
}

/// 스냅샷 파일 생성/갱신 — 개발 플로우용 (명세 §7.4a).
/// 경로: `resolve_schema_dir(manifest_dir)/{DB_NAME}.{VERSION}.json`
pub fn write_schema_snapshot<T: DatabaseSpec>(manifest_dir: &str) -> Result<std::path::PathBuf> {
    let schema = T::schema();
    schema.validate_unique_tables()?;
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let path = roomrs_migrate::snapshot_path(&dir, T::DB_NAME, T::VERSION);
    schema
        .to_snapshot()
        .write_to(&path)
        .map_err(|e| Error::Config(format!("스냅샷 저장 실패: {e}")))?;
    Ok(path)
}

/// 스냅샷 ↔ 엔티티 메타 일치 검사 — CI/check용 (명세 §7.4a)
pub fn check_schema_snapshot<T: DatabaseSpec>(manifest_dir: &str) -> Result<()> {
    let schema = T::schema();
    schema.validate_unique_tables()?;
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let path = roomrs_migrate::snapshot_path(&dir, T::DB_NAME, T::VERSION);
    let file = roomrs_migrate::SchemaSnapshot::read_from(&path)
        .map_err(|e| Error::SnapshotStale(format!("스냅샷 파일을 읽을 수 없습니다: {e}")))?;
    let code = schema.to_snapshot();
    if file.hash() != code.hash() {
        return Err(Error::SnapshotStale(format!(
            "스냅샷과 엔티티 정의가 다릅니다 — `write_schema_snapshot`으로 재생성 필요 (파일: {})",
            path.display()
        )));
    }
    Ok(())
}

/// Snapshot export/stale-check helper called by the test that
/// `#[database]` generates (spec §7.4, decisions 21b/28).
///
/// Behavior for the current-version snapshot file:
/// - `ROOMRS_SCHEMA_EXPORT=0` in the environment: no-op, returns `Ok`.
/// - File missing: write it, then return [`Error::SnapshotStale`] asking to
///   commit and **rebuild** (`cargo clean -p <crate>` or touching a source
///   file — cargo does not re-expand the macro for a newly created file,
///   so `SNAPSHOT_HASH` and the embedded snapshot chain stay stale until
///   then; decision 28, H-6).
/// - File exists and matches the entity metadata hash, but
///   [`DatabaseSpec::SNAPSHOT_FILE_SEEN`] is `false` (the macro expanded
///   before the file existed): still [`Error::SnapshotStale`] until a
///   rebuild embeds it (D-3b).
/// - File matches and was seen at expansion: `Ok`.
/// - File stale or corrupt: **rewrite** the file, then return
///   [`Error::SnapshotStale`] so CI fails until the regenerated snapshot is
///   committed.
pub fn export_schema_for_test<T: DatabaseSpec>(manifest_dir: &str) -> Result<()> {
    // 옵트아웃 (명세 §7.4) — CI/리포 설정으로 export 비활성 가능
    if std::env::var("ROOMRS_SCHEMA_EXPORT").as_deref() == Ok("0") {
        return Ok(());
    }
    let schema = T::schema();
    schema.validate_unique_tables()?;
    let code = schema.to_snapshot();
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let path = roomrs_migrate::snapshot_path(&dir, T::DB_NAME, T::VERSION);
    // 저장 헬퍼 — 실패는 설정 에러로 승격
    let write = |snap: &roomrs_migrate::SchemaSnapshot| {
        snap.write_to(&path)
            .map_err(|e| Error::Config(format!("스냅샷 저장 실패 ({}): {e}", path.display())))
    };
    if !path.exists() {
        write(&code)?;
        // 신규 파일은 include_bytes! 의존성에 미등록이라 cargo가 재전개하지
        // 않는다 — 여기서 성공 처리하면 SNAPSHOT_HASH·내장 체인이 스테일한 채
        // 남는다. 구체적 복구 행동까지 안내한다 (결정 28, H-6/D-3a)
        return Err(Error::SnapshotStale(format!(
            "스냅샷을 생성했습니다 — 커밋 후 `cargo clean -p <크레이트>` 또는 소스 touch 후 재빌드하세요: {}",
            path.display()
        )));
    }
    match roomrs_migrate::SchemaSnapshot::read_from(&path) {
        // 파손 파일 = 재생성 후 실패 반환 — 부재와 구분 (M-19)
        Err(e) => {
            write(&code)?;
            Err(Error::SnapshotStale(format!(
                "스냅샷 파일 파손({e}) — 재생성했습니다, 커밋하세요: {}",
                path.display()
            )))
        }
        Ok(file) if file.hash() == code.hash() => {
            // 파일은 일치하지만 매크로 전개 시점엔 없었다(생성 직후 재빌드 전) —
            // 성공 처리하면 SNAPSHOT_HASH=None·내장 체인 누락이 침묵 통과하는
            // fail-open 창이 열린다. 재전개 전까지 계속 실패시킨다 (D-3b)
            if !T::SNAPSHOT_FILE_SEEN {
                return Err(Error::SnapshotStale(format!(
                    "스냅샷이 아직 바이너리에 반영되지 않았습니다 — `cargo clean -p <크레이트>` 또는 소스 touch 후 재빌드하세요: {}",
                    path.display()
                )));
            }
            Ok(())
        }
        // 스테일 = 재생성 후 실패 반환 (CI에서 미커밋 차단, 로컬 재생성)
        Ok(_) => {
            write(&code)?;
            Err(Error::SnapshotStale(format!(
                "스냅샷이 스테일하여 재생성했습니다 — 커밋하세요: {} (반복되면 다른 #[database]와 DB_NAME 충돌 가능 — 구조체명 snake_case가 크레이트 내에서 유일해야 합니다, M-11)",
                path.display()
            )))
        }
    }
}

/// 마이그레이션 정책 (명세 §8 — M1은 Auto 최소 동작만, M3에서 완성)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MigrationPolicy {
    /// 자동: 신규 DB면 DDL 실행, 버전 일치면 통과, 불일치면 에러(M3에서 diff 실행으로 대체)
    #[default]
    Auto,
    /// 검증만: 버전 불일치 시 에러
    Validate,
}

/// DB 내부 공유 상태 — 핸들·트랜잭션이 참조
pub struct DatabaseInner {
    pub(crate) pool: Pool,
    query_logger: Option<QueryLogger>,
    /// 무효화 트래커 + 노티파이어 (feature live, 명세 §9)
    #[cfg(feature = "live")]
    pub(crate) tracker: Arc<crate::live::Tracker>,
    /// preupdate hook이 행 값을 이름으로 복원할 때 사용하는 컬럼 순서.
    #[cfg(feature = "live")]
    pub(crate) hook_columns: Arc<std::collections::HashMap<String, Vec<String>>>,
    /// 노티파이어 스레드 join 핸들 — drop 시 join으로 커넥션 잔류 방지 (M-5)
    #[cfg(feature = "live")]
    notifier_join: Option<std::thread::JoinHandle<()>>,
}

/// 종료 로그 — live 미사용 빌드에도 close 로그를 남긴다 (지시서 logging-log-crate)
#[cfg(not(feature = "live"))]
impl Drop for DatabaseInner {
    /// DB 종료 로그만 수행 (정리할 백그라운드 스레드 없음)
    fn drop(&mut self) {
        log::info!("database closed");
    }
}

#[cfg(feature = "live")]
impl Drop for DatabaseInner {
    /// 노티파이어·MI 폴러 종료 신호 + join — 스레드/커넥션 잔류 방지 (M-5).
    /// 마지막 Arc가 노티파이어 스레드 자신(구독 콜백 등)에서 drop되면 self-join이
    /// 교착이므로 그 경우엔 join을 생략하고 분리한다 (H-3)
    fn drop(&mut self) {
        log::info!("database closing — shutting down notifier");
        // 트래커 종료 — 레지스트리 청산으로 대기 중 recv를 깨운다 (M-7)
        self.tracker.shutdown();
        if let Some(h) = self.notifier_join.take() {
            if h.thread().id() == std::thread::current().id() {
                // 노티파이어 스레드 위에서의 drop — self-join 교착 방지, 분리 (H-3)
                log::warn!(
                    "database dropped on notifier thread — detaching notifier instead of joining"
                );
            } else {
                let _ = h.join();
            }
        }
        log::info!("database closed");
    }
}

#[cfg(feature = "live")]
impl DatabaseInner {
    /// SQL 실행별 hook 수집 프레임을 시작한다.
    pub(crate) fn begin_hook_capture(&self) -> HookCapture {
        let depth = HOOK_CAPTURES.with(|captures| {
            let mut captures = captures.borrow_mut();
            let depth = captures.len();
            captures.push(Default::default());
            depth
        });
        HookCapture { depth }
    }

    /// 훅 수집분 회수
    pub(crate) fn take_hook_capture(&self) -> HookCaptureFrame {
        HOOK_CAPTURES.with(|captures| captures.borrow_mut().pop().unwrap_or_default())
    }

    /// 단문 write 성공 후 무효화 방출 — 문장 파싱 ∪ 훅 (명세 §9.2).
    /// 확실한 읽기 전용 문장(SELECT/EXPLAIN)은 문장 기반 방출을 하지 않는다
    /// (L-2). PRAGMA는 상태 변경 여부를 확실히 구분할 수 없어 전체 무효화한다.
    /// 읽기 전용 문장은 훅 수집분(트리거 write)만 방출한다.
    pub(crate) fn emit_after_write(&self, sql: &str) {
        let capture = self.take_hook_capture();
        let changed_tables: std::collections::HashSet<String> = capture
            .changes
            .iter()
            .map(|change| change.table.clone())
            .collect();
        if !capture.changes.is_empty() {
            self.tracker.invalidate_changes(capture.changes);
        }
        match crate::live::extract_write_tables(sql) {
            crate::live::WriteTables::ReadOnly => {
                let tables: std::collections::HashSet<String> = capture
                    .tables
                    .difference(&changed_tables)
                    .cloned()
                    .collect();
                if !tables.is_empty() {
                    self.tracker.invalidate(Some(tables));
                }
            }
            crate::live::WriteTables::Tables(mut t) => {
                t.extend(capture.tables);
                t.retain(|table| !changed_tables.contains(table));
                if !t.is_empty() {
                    self.tracker.invalidate(Some(t));
                }
            }
            // 파싱 실패/DDL = 보수적 전체 무효화
            crate::live::WriteTables::Unknown => self.tracker.invalidate(None),
        }
    }
}

impl DatabaseInner {
    /// 쿼리 로거 래핑 실행 — 로거 없으면 오버헤드 없이 통과.
    /// log 파사드에는 SQL 문자열만 남긴다 — 파라미터 값 금지 (민감정보)
    pub(crate) fn log_query<R>(&self, sql: &str, f: impl FnOnce() -> Result<R>) -> Result<R> {
        log::trace!("SQL: {sql}");
        match &self.query_logger {
            None => f(),
            Some(logger) => {
                let start = std::time::Instant::now();
                let out = f();
                logger(sql, start.elapsed());
                out
            }
        }
    }
}

/// roomrs 코어 데이터베이스 — 사용자는 `#[database]` 생성 타입으로 감싸 쓴다
pub struct Database {
    inner: Arc<DatabaseInner>,
}

impl Database {
    /// 동기 핸들 (명세 §5.0)
    pub fn run_sync(&self) -> SyncHandle<'_> {
        SyncHandle { inner: &self.inner }
    }

    /// 내부 상태 Arc — roomrs-async 전용 (직접 사용 금지)
    #[doc(hidden)]
    pub fn inner_arc(&self) -> Arc<DatabaseInner> {
        Arc::clone(&self.inner)
    }
}

impl DatabaseInner {
    /// Arc에서 동기 핸들 구성 — roomrs-async 워커 전용 (직접 사용 금지)
    #[doc(hidden)]
    pub fn sync_handle(self: &Arc<Self>) -> SyncHandle<'_> {
        SyncHandle { inner: self }
    }
}

/// 빌더 (명세 §5.4)
pub struct DatabaseBuilder<T: DatabaseSpec> {
    path: Option<PathBuf>,
    in_memory: bool,
    connections: usize,
    busy_timeout: Duration,
    queue_timeout: Option<Duration>,
    migrate: MigrationPolicy,
    migrations: Vec<crate::migration::Migration>,
    auto_migrate: bool,
    destructive_fallback: bool,
    #[cfg(feature = "cipher")]
    encryption_key: Option<String>,
    on_create: Option<ConnCallback>,
    on_open: Option<ConnCallback>,
    query_logger: Option<QueryLogger>,
    _spec: std::marker::PhantomData<T>,
}

impl<T: DatabaseSpec> Default for DatabaseBuilder<T> {
    /// 기본값 — 커넥션 수는 CPU 코어 기반(최대 4), busy_timeout 5초
    fn default() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Self {
            path: None,
            in_memory: false,
            connections: cores.clamp(1, 4) + 1,
            busy_timeout: Duration::from_secs(5),
            queue_timeout: None,
            migrate: MigrationPolicy::Auto,
            migrations: Vec::new(),
            auto_migrate: false,
            destructive_fallback: false,
            #[cfg(feature = "cipher")]
            encryption_key: None,
            on_create: None,
            on_open: None,
            query_logger: None,
            _spec: std::marker::PhantomData,
        }
    }
}

impl<T: DatabaseSpec> DatabaseBuilder<T> {
    /// Sets the SQLite file path. The special `:memory:` path uses the same
    /// single regular connection setup as [`Self::in_memory`].
    pub fn sqlite(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if path == std::path::Path::new(":memory:") {
            self.path = None;
            self.in_memory = true;
        } else {
            self.path = Some(path);
            self.in_memory = false;
        }
        self
    }

    /// In-memory database for tests.
    ///
    /// In-memory databases use one regular read/write connection regardless
    /// of [`Self::connections`]. This serializes transactions and avoids
    /// SQLite shared-cache `SQLITE_LOCKED` failures. With the `live` feature,
    /// the notifier still owns its separate internal connection.
    pub fn in_memory(mut self) -> Self {
        self.in_memory = true;
        self.path = None;
        self
    }

    /// Sets the number of read/write connections in the unified pool.
    ///
    /// In-memory databases always use one regular connection.
    pub fn connections(mut self, n: usize) -> Self {
        self.connections = n.max(1);
        self
    }

    /// SQLITE_BUSY 대기 — 프로세스 내(통합 풀 동시 write)·외부 프로세스 write 경합 공용 (명세 §10)
    pub fn busy_timeout(mut self, d: Duration) -> Self {
        self.busy_timeout = d;
        self
    }

    /// 커넥션 풀 대기 타임아웃 — 초과 시 `Error::QueueTimeout`
    pub fn queue_timeout(mut self, d: Duration) -> Self {
        self.queue_timeout = Some(d);
        self
    }

    /// SQLCipher 암호화 키 (명세 §0 14, feature cipher) — 모든 커넥션 오픈 직후 PRAGMA key
    #[cfg(feature = "cipher")]
    pub fn encryption_key(mut self, key: impl Into<String>) -> Self {
        self.encryption_key = Some(key.into());
        self
    }

    /// 마이그레이션 정책
    pub fn migrate(mut self, policy: MigrationPolicy) -> Self {
        self.migrate = policy;
        self
    }

    /// 마이그레이션 스텝 등록 (명세 §8.2/§8.3) — 세 소스 공통 표현
    pub fn migration(mut self, m: crate::migration::Migration) -> Self {
        self.migrations.push(m);
        self
    }

    /// 마이그레이션 스텝 일괄 등록 — `migrations_dir!` 산출물 등
    pub fn migrations(mut self, ms: impl IntoIterator<Item = crate::migration::Migration>) -> Self {
        self.migrations.extend(ms);
        self
    }

    /// Opt-in automatic migration from embedded snapshots (spec §8.4,
    /// decision 21d, default off).
    ///
    /// When enabled, gaps in the registered migration chain are filled by
    /// diffing consecutive embedded snapshots
    /// ([`DatabaseSpec::EMBEDDED_SCHEMAS`]). Only **safe** operations are
    /// executed automatically (CREATE TABLE, nullable ADD COLUMN, valid
    /// RENAME COLUMN, CREATE INDEX). A gap whose diff contains destructive
    /// changes fails with a clear [`Error::Migration`] instead — register a
    /// manual step or use
    /// [`fallback_to_destructive_migration`](Self::fallback_to_destructive_migration).
    /// Registered steps always take precedence over synthesized ones.
    pub fn auto_migrate(mut self, on: bool) -> Self {
        self.auto_migrate = on;
        self
    }

    /// 파괴적 마이그레이션 폴백 (명세 §8, 기본 off) —
    /// 체인이 불충분하면 **모든 테이블을 삭제**하고 현재 스키마로 재생성한다.
    pub fn fallback_to_destructive_migration(mut self, enable: bool) -> Self {
        self.destructive_fallback = enable;
        self
    }

    /// 최초 생성 시 1회 콜백 (테이블 생성 직후).
    ///
    /// Runs **inside** the schema-creation transaction (L-5): if the callback
    /// fails, the schema DDL and `user_version` roll back together, so the
    /// next open retries creation from scratch. Do not manage transactions
    /// (`BEGIN`/`COMMIT`) inside the callback.
    pub fn on_create(
        mut self,
        f: impl Fn(&Connection) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.on_create = Some(Arc::new(f));
        self
    }

    /// 오픈 시마다 콜백
    pub fn on_open(
        mut self,
        f: impl Fn(&Connection) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.on_open = Some(Arc::new(f));
        self
    }

    /// 쿼리 로거 — (sql, 소요시간)
    pub fn query_logger(mut self, f: impl Fn(&str, Duration) + Send + Sync + 'static) -> Self {
        self.query_logger = Some(Box::new(f));
        self
    }

    /// DB 오픈 — PRAGMA 초기화 · 스냅샷 스테일 검증 · 마이그레이션 · 풀 구성 (명세 §5.4)
    pub fn build(mut self) -> Result<T> {
        let schema = T::schema();
        schema.validate_unique_tables()?;

        // shared-cache 인메모리의 동시 BEGIN IMMEDIATE는 busy_timeout이 처리하지 못하는
        // SQLITE_LOCKED를 반환하므로 일반 풀을 하나로 고정한다.
        if self.in_memory {
            self.connections = 1;
        }

        // 스냅샷 스테일 런타임 검증 (명세 §7.4b) —
        // 매크로가 임베드한 스냅샷 파일 해시 vs 엔티티 메타 재계산 해시
        if let Some(embedded) = T::SNAPSHOT_HASH {
            let runtime = schema.to_snapshot().hash();
            if embedded != runtime {
                return Err(Error::SnapshotStale(format!(
                    "스냅샷 해시 불일치 (파일={embedded:#x}, 엔티티={runtime:#x}) — \
                     엔티티 수정 후 스냅샷 재생성이 필요합니다"
                )));
            }
        }

        // 인메모리 공유 이름 — 커넥션 N개가 같은 DB를 보도록 named shared-cache URI 사용
        let mem_name = if self.in_memory {
            use std::sync::atomic::{AtomicU64, Ordering};
            static MEM_SEQ: AtomicU64 = AtomicU64::new(0);
            Some(format!(
                "file:roomrs_mem_{}?mode=memory&cache=shared",
                MEM_SEQ.fetch_add(1, Ordering::Relaxed)
            ))
        } else {
            None
        };

        // 통합 풀 커넥션 오픈 + 공통 PRAGMA (명세 §10)
        let first_conn = self.open_conn(mem_name.as_deref(), &schema, false)?;

        // 일반 작업 커넥션 — 모두 read/write 가능 (명세 §10)
        let mut connections = Vec::with_capacity(self.connections);
        connections.push(first_conn);
        for _ in 1..self.connections {
            let conn = self.open_conn(mem_name.as_deref(), &schema, false)?;
            connections.push(conn);
        }

        // 라이브 쿼리 기반 구축 (feature live, 명세 §9) —
        // 노티파이어 전용 커넥션 + preupdate_hook 행 변경 수집 설치
        #[cfg(feature = "live")]
        let hook_columns = Arc::new(
            schema
                .tables
                .iter()
                .map(|table| {
                    (
                        table.name.to_ascii_lowercase(),
                        table
                            .columns
                            .iter()
                            .map(|column| column.name.to_string())
                            .collect(),
                    )
                })
                .collect(),
        );
        #[cfg(feature = "live")]
        let (tracker, notifier_join) = {
            let notifier_conn = self.open_conn(mem_name.as_deref(), &schema, false)?;
            for conn in &connections {
                install_preupdate_hook(conn, Arc::clone(&hook_columns))?;
            }
            crate::live::Tracker::start(notifier_conn)?
        };

        let reconnect_settings = ConnectionSettings {
            path: self.path.clone(),
            mem_name: mem_name.clone(),
            busy_timeout: self.busy_timeout,
            on_open: self.on_open.clone(),
            #[cfg(feature = "cipher")]
            encryption_key: self.encryption_key.clone(),
        };
        let connection_settings = reconnect_settings;
        #[cfg(feature = "live")]
        let connection_hook_columns = Arc::clone(&hook_columns);
        let connection_reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync> =
            Arc::new(move || {
                let conn = connection_settings.open(false)?;
                // 사용자 callback이 같은 이름의 함수/hook을 교체할 수 있으므로
                // roomrs connection-local 상태는 initialize 뒤 마지막에 설치한다.
                connection_settings.initialize(&conn)?;
                #[cfg(feature = "live")]
                {
                    install_preupdate_hook(&conn, Arc::clone(&connection_hook_columns))?;
                }
                Ok(conn)
            });

        let inner = DatabaseInner {
            pool: Pool {
                connections: ConnectionPool::new_with_preservation(
                    connections,
                    self.in_memory,
                    self.in_memory,
                    self.queue_timeout,
                    connection_reopen,
                ),
            },
            query_logger: self.query_logger.take(),
            #[cfg(feature = "live")]
            tracker,
            #[cfg(feature = "live")]
            hook_columns: Arc::clone(&hook_columns),
            #[cfg(feature = "live")]
            notifier_join: Some(notifier_join),
        };
        let db = Database {
            inner: Arc::new(inner),
        };

        // 마이그레이션 — 풀 구성 후 Tx 기반으로 실행 (명세 §8)
        self.run_migration(&db, &schema)?;

        // 마이그레이션 완료 뒤 모든 풀 연결을 초기화한다.
        if let Some(cb) = &self.on_open {
            {
                db.inner
                    .pool
                    .connections
                    .for_each_idle(|conn| Self::apply_on_open(conn, cb, false, self.in_memory))?;
            }
            #[cfg(feature = "live")]
            db.inner
                .tracker
                .initialize(Arc::clone(cb), self.in_memory)?;
        }

        // on_open이 roomrs hook을 교체했더라도 일반 풀의
        // connection-local 상태가 최종 승자가 되도록 전부 재설치한다.
        db.inner.pool.connections.for_each_idle(|_conn| {
            #[cfg(feature = "live")]
            {
                install_preupdate_hook(_conn, Arc::clone(&hook_columns))?;
            }
            Ok::<(), Error>(())
        })?;

        // 오픈 완료 로그 — 경로(인메모리는 ":memory:")와 스키마 버전
        log::info!(
            "database opened: path={}, version={}",
            self.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| ":memory:".into()),
            schema.version
        );

        Ok(T::from_database(db))
    }

    /// 커넥션 1개 오픈 + PRAGMA 설정
    fn open_conn(
        &self,
        mem_name: Option<&str>,
        _schema: &SchemaDef,
        _read_only: bool,
    ) -> Result<Connection> {
        let conn = match (mem_name, &self.path) {
            (Some(uri), _) => {
                use rusqlite::OpenFlags;
                Connection::open_with_flags(
                    uri,
                    OpenFlags::SQLITE_OPEN_READ_WRITE
                        | OpenFlags::SQLITE_OPEN_CREATE
                        | OpenFlags::SQLITE_OPEN_URI
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?
            }
            (None, Some(path)) => Connection::open(path)?,
            (None, None) => {
                return Err(Error::Config(
                    "DB 경로가 설정되지 않았습니다 — .sqlite(path) 또는 .in_memory() 필요".into(),
                ));
            }
        };

        // 암호화 키 — 어떤 접근보다 먼저 적용해야 한다 (feature cipher)
        #[cfg(feature = "cipher")]
        if let Some(key) = &self.encryption_key {
            conn.pragma_update(None, "key", key)?;
        }

        // busy 핸들러를 다른 PRAGMA보다 먼저 — journal_mode 전환 등도 락 경합이
        // 있어 동시 오픈 시 SQLITE_BUSY로 실패할 수 있다 (M-4)
        conn.busy_timeout(self.busy_timeout)?;

        // 공통 PRAGMA (명세 §10) — 인메모리는 WAL 미지원이라 파일 DB에만 적용.
        // 신규 파일의 WAL 전환은 동시 오픈 시 busy 핸들러가 개입하지 못하는
        // 락 경합이 있어 짧은 재시도로 흡수한다 (M-4)
        if mem_name.is_none() {
            let deadline =
                std::time::Instant::now() + self.busy_timeout.max(Duration::from_millis(500));
            loop {
                match conn.pragma_update(None, "journal_mode", "WAL") {
                    Ok(()) => break,
                    Err(rusqlite::Error::SqliteFailure(fe, _))
                        if fe.code == rusqlite::ErrorCode::DatabaseBusy
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        conn.pragma_update(None, "query_only", "OFF")?;
        if mem_name.is_some() {
            conn.pragma_update(None, "read_uncommitted", "ON")?;
        }
        log::debug!("read/write pool connection opened");
        Ok(conn)
    }

    /// 사용자 연결 초기화 후 트랜잭션·풀 커넥션 불변식을 복구한다.
    fn apply_on_open(
        conn: &Connection,
        cb: &ConnCallback,
        read_only: bool,
        read_uncommitted: bool,
    ) -> Result<()> {
        let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(conn)))
            .map_err(|_| Error::Internal("on_open 콜백 panic".into()))
            .and_then(|result| result);
        if !conn.is_autocommit() {
            conn.execute_batch("ROLLBACK")?;
        }
        let _ = read_only;
        conn.pragma_update(None, "query_only", "OFF")?;
        if read_uncommitted {
            conn.pragma_update(None, "read_uncommitted", "ON")?;
        }
        callback_result
    }

    /// 마이그레이션 러너 (명세 §8) — user_version 기반.
    /// 0(신규) = DDL 생성 + on_create · 일치 = 통과 ·
    /// 불일치 = Auto: 스텝 체인 실행(갭이면 destructive 폴백 또는 에러), Validate: 에러.
    /// 각 트랜잭션(BEGIN IMMEDIATE) 획득 후 user_version을 재확인해
    /// 교차 프로세스 동시 마이그레이션 경합을 차단한다 (M-4)
    fn run_migration(&self, db: &Database, schema: &SchemaDef) -> Result<()> {
        let h = db.run_sync();
        let current: u32 =
            h.with_connection(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))?;
        let target = schema.version;

        // 신규 DB — 스키마 생성 (스텝 없이 최신 DDL로 직행)
        if current == 0 {
            let created = h.transaction(|tx| {
                // 락 확보 후 재확인 — 다른 프로세스가 먼저 생성했으면 스킵 (M-4)
                let cur: u32 = tx.query_scalar("PRAGMA user_version", [])?;
                if cur != 0 {
                    return Ok(false);
                }
                for ddl in &schema.ddl {
                    // 빌드 시점엔 구독자가 존재할 수 없다 — 무효화 수집을 생략해
                    // 스테일 전체 무효화가 첫 구독과 경합하지 않게 한다 (H-1 회귀 방지)
                    tx.raw_conn().execute_batch(ddl)?;
                }
                // on_create를 생성 트랜잭션 안에서 실행 — 실패 시 스키마와
                // user_version이 함께 롤백돼 다음 오픈이 생성을 재시도한다 (L-5)
                if let Some(cb) = &self.on_create {
                    cb(tx.raw_conn())?;
                }
                tx.execute_batch(&format!("PRAGMA user_version = {target}"))?;
                Ok(true)
            })?;
            if created {
                log::info!("schema created at version {target}");
                return Ok(());
            }
            // 다른 프로세스가 먼저 생성 — 버전 검증 경로로 재진입 (M-4)
            return self.run_migration(db, schema);
        }

        if current == target {
            return Ok(());
        }

        if self.migrate == MigrationPolicy::Validate {
            log::error!(
                "migration failed: schema version mismatch (db={current}, code={target}, policy=Validate)"
            );
            return Err(Error::Migration(format!(
                "스키마 버전 불일치: DB={current}, 코드={target} (Validate 정책)"
            )));
        }

        // 자동 마이그레이션(옵트인, 명세 §8.4) — 등록 스텝이 없는 구간을
        // 내장 스냅샷 연속 쌍 diff의 안전 연산으로 합성해 메운다
        let synthesized = if self.auto_migrate {
            synthesize_embedded_steps(T::EMBEDDED_SCHEMAS, &self.migrations, current)?
        } else {
            SynthesizedSteps::default()
        };
        let all_steps: Vec<&crate::migration::Migration> = self
            .migrations
            .iter()
            .chain(synthesized.steps.iter())
            .collect();

        // 스텝 체인 실행 — 스텝별 트랜잭션 + user_version 갱신.
        // 파괴적 구간 사전 검사를 먼저 — plan_chain의 일반 갭 에러보다 구체적 안내
        let plan_result = check_destructive_gap(&all_steps, &synthesized, current, target)
            .and_then(|()| crate::migration::plan_chain(&all_steps, current, target));
        match plan_result {
            Ok(plan) => {
                for step in plan {
                    log::info!(
                        "migration step: v{}->v{}",
                        step.from_version(),
                        step.to_version()
                    );
                    h.transaction(|tx| {
                        // 락 확보 후 재확인 — 다른 프로세스가 이미 적용했으면 스킵 (M-4)
                        let cur: u32 = tx.query_scalar("PRAGMA user_version", [])?;
                        if cur >= step.to_version() {
                            return Ok(());
                        }
                        // 체인 구성이 다른 프로세스의 개입 감지 — 스텝 시작 버전이
                        // 실제 버전과 다르면 잘못된 SQL 적용을 차단한다 (M-5)
                        if cur != step.from_version() {
                            return Err(Error::Migration(format!(
                                "동시 마이그레이션 감지: 예상 v{}, 실제 v{cur} — 체인 구성 상이",
                                step.from_version()
                            )));
                        }
                        step.run_up(tx)?;
                        tx.execute_batch(&format!("PRAGMA user_version = {}", step.to_version()))
                    })?;
                }
                Ok(())
            }
            Err(e) if self.destructive_fallback => {
                // 파괴적 폴백 (옵트인, 명세 §8) — 전부 삭제 후 최신 스키마로 재생성
                log::warn!("migration chain insufficient — falling back to destructive migration");
                let _ = e;
                self.run_destructive(&h, schema)
            }
            Err(e) => {
                log::error!("migration failed: {e}");
                Err(e)
            }
        }
    }

    /// 파괴적 재생성 — 사용자 객체 전부 drop 후 DDL 재실행
    fn run_destructive(&self, h: &SyncHandle<'_>, schema: &SchemaDef) -> Result<()> {
        // FK 토글과 DDL은 같은 커넥션에서 실행해야 한다.
        h.with_connection(|c| {
            c.pragma_update(None, "foreign_keys", "OFF")?;
            let result: Result<()> = (|| {
                c.execute_batch("BEGIN IMMEDIATE")?;
                let migration: Result<()> = (|| {
                    // 사용자 객체 수집 (sqlite_* 내부 객체 제외)
                    let mut statement = c.prepare(
                        "SELECT type, name FROM sqlite_master \
                         WHERE name NOT LIKE 'sqlite_%' \
                         AND type IN ('trigger','view','index','table')",
                    )?;
                    let objs = statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<std::result::Result<Vec<(String, String)>, _>>()?;
                    drop(statement);
                    // 의존 역순: trigger → view → index → table.
                    // sqlite_master의 이름은 임의 문자열 — 식별자 이스케이프 필수 (M-9).
                    for kind in ["trigger", "view", "index", "table"] {
                        for (t, name) in objs.iter().filter(|(t, _)| t == kind) {
                            c.execute_batch(&format!(
                                "DROP {} {}",
                                t.to_uppercase(),
                                escape_ident(name)
                            ))?;
                        }
                    }
                    for ddl in &schema.ddl {
                        c.execute_batch(ddl)?;
                    }
                    c.execute_batch(&format!("PRAGMA user_version = {}", schema.version))?;
                    Ok(())
                })();
                match migration {
                    Ok(()) => c.execute_batch("COMMIT")?,
                    Err(error) => {
                        let _ = c.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                }
                Ok(())
            })();
            let restore = c.pragma_update(None, "foreign_keys", "ON");
            result.and(restore.map_err(Into::into))
        })
    }
}

/// 자동 마이그레이션 합성 결과 — 합성 스텝 + 파괴적으로 거부된 구간 기록
#[derive(Default)]
struct SynthesizedSteps {
    /// 안전 연산만으로 합성된 스텝들
    steps: Vec<crate::migration::Migration>,
    /// from 버전 → (to 버전, 파괴적 항목 요약) — 합성 거부 구간
    refused: std::collections::HashMap<u32, (u32, String)>,
}

/// 내장 스냅샷의 인접 가용 버전 쌍을 diff해 갭 메움 스텝을 합성한다 (명세 §8.4).
/// 등록 스텝이 있는 from 버전은 건너뛴다(등록 스텝 우선). 파괴적 변경이 포함된
/// 쌍은 합성하지 않고 기록만 남긴다 — 체인이 그 갭에 닿으면 명확한 에러.
/// `current` 미만에서 출발하는 쌍은 계획에 쓰일 수 없으므로 건너뛴다 —
/// 무관한 옛 스냅샷의 압축 해제(파손 시 실패 포함)를 피한다 (L-1)
fn synthesize_embedded_steps(
    embedded: &[EmbeddedSchema],
    registered: &[crate::migration::Migration],
    current: u32,
) -> Result<SynthesizedSteps> {
    let mut out = SynthesizedSteps::default();
    if embedded.len() < 2 {
        return Ok(out);
    }
    // 방어적 정렬 — 매크로는 오름차순 방출하지만 수동 impl 대비
    let mut sorted: Vec<&EmbeddedSchema> = embedded.iter().collect();
    sorted.sort_by_key(|e| e.version);

    for pair in sorted.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.version == b.version {
            continue;
        }
        // 현재 DB 버전 미만 구간 — 사용 불가, 옛 스냅샷 접근 생략 (L-1)
        if a.version < current {
            continue;
        }
        // 등록 스텝 우선 — 같은 from에서 출발하는 스텝이 있으면 합성 생략
        if registered.iter().any(|m| m.from_version() == a.version) {
            continue;
        }
        let old = a.snapshot()?;
        let new = b.snapshot()?;
        let plan = roomrs_migrate::diff_plan(&old, &new);
        if plan.destructive.is_empty() {
            log::info!(
                "auto-migrate synthesized step: v{}->v{}",
                a.version,
                b.version
            );
            out.steps.push(crate::migration::Migration::sql(
                a.version,
                b.version,
                plan.safe.join(";\n"),
            ));
        } else {
            out.refused
                .insert(a.version, (b.version, plan.destructive.join("; ")));
        }
    }
    Ok(out)
}

/// 체인을 사전 답사해 파괴적 합성 거부 구간에 닿는지 검사 — 닿으면 실행 전에
/// 구체적 에러를 반환한다(일반 갭·형식 오류는 plan_chain이 보고).
fn check_destructive_gap(
    steps: &[&crate::migration::Migration],
    synthesized: &SynthesizedSteps,
    current: u32,
    target: u32,
) -> Result<()> {
    if synthesized.refused.is_empty() {
        return Ok(());
    }
    let mut v = current;
    while v < target {
        match steps.iter().find(|s| s.from_version() == v) {
            Some(s) if s.to_version() > v && s.to_version() <= target => v = s.to_version(),
            // 역행/오버슈트 스텝 = plan_chain이 보고
            Some(_) => return Ok(()),
            None => {
                if let Some((to, items)) = synthesized.refused.get(&v) {
                    return Err(Error::Migration(format!(
                        "v{v}->v{to} 자동 마이그레이션 불가 — 파괴적 변경 포함: {items}; \
                         수동 스텝을 등록하거나 fallback_to_destructive_migration 사용"
                    )));
                }
                // 일반 갭 = plan_chain이 보고
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use roomrs_migrate::{ColumnSnapshot, SchemaSnapshot, TableSnapshot};

    /// 인메모리 DB는 builder 호출 순서와 무관하게 일반 커넥션 하나만 만든다.
    #[test]
    fn in_memory_uses_one_regular_connection() {
        for builder in [
            DatabaseBuilder::<DestructiveFkDb>::default()
                .in_memory()
                .connections(3),
            DatabaseBuilder::<DestructiveFkDb>::default()
                .connections(3)
                .in_memory(),
            DatabaseBuilder::<DestructiveFkDb>::default()
                .connections(3)
                .sqlite(":memory:"),
        ] {
            let db = builder.build().unwrap().0;
            let mut idle = 0;
            db.inner
                .pool
                .connections
                .for_each_idle(|_| {
                    idle += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(idle, 1);
        }
    }

    /// 인메모리 동시 트랜잭션은 단일 일반 커넥션에서 직렬 실행된다.
    #[test]
    fn in_memory_serializes_concurrent_transactions() {
        let db = Arc::new(
            DatabaseBuilder::<DestructiveFkDb>::default()
                .in_memory()
                .connections(4)
                .build()
                .unwrap()
                .0,
        );
        let mut workers = Vec::new();
        for worker in 0..4 {
            let db = Arc::clone(&db);
            workers.push(std::thread::spawn(move || {
                for item in 0..25 {
                    db.run_sync().transaction(|tx| {
                        tx.execute(
                            "INSERT INTO parents(id) VALUES (?1)",
                            [worker * 25 + item + 1],
                        )?;
                        Ok(())
                    })?;
                }
                Result::<()>::Ok(())
            }));
        }
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        let count: i64 = db
            .run_sync()
            .query_scalar("SELECT COUNT(*) FROM parents", [])
            .unwrap();
        assert_eq!(count, 100);
    }

    struct DestructiveFkDb(Database);

    impl DatabaseSpec for DestructiveFkDb {
        const VERSION: u32 = 1;
        const DB_NAME: &'static str = "destructive_fk_db";

        fn schema() -> SchemaDef {
            SchemaDef {
                version: 1,
                ddl: vec![
                    "CREATE TABLE parents(id INTEGER PRIMARY KEY)",
                    "CREATE TABLE children(id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parents(id))",
                ],
                tables: Vec::new(),
            }
        }

        fn from_database(db: Database) -> Self {
            Self(db)
        }
    }

    /// 파괴적 재생성은 FK 참조 데이터가 있어도 성공하고 모든 풀 커넥션의 FK를 복구한다.
    #[test]
    fn destructive_migration_uses_one_connection_and_restores_foreign_keys() {
        static FILE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let file = std::env::temp_dir().join(format!(
            "roomrs-destructive-fk-{}-{}.db",
            std::process::id(),
            FILE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let db = DatabaseBuilder::<DestructiveFkDb>::default()
            .sqlite(&file)
            .connections(3)
            .build()
            .unwrap()
            .0;
        db.run_sync()
            .execute("INSERT INTO parents(id) VALUES (1)", [])
            .unwrap();
        db.run_sync()
            .execute("INSERT INTO children(id, parent_id) VALUES (1, 1)", [])
            .unwrap();

        let target = SchemaDef {
            version: 2,
            ddl: vec!["CREATE TABLE replacements(id INTEGER PRIMARY KEY)"],
            tables: Vec::new(),
        };
        DatabaseBuilder::<DestructiveFkDb>::default()
            .run_destructive(&db.run_sync(), &target)
            .unwrap();

        db.inner
            .pool
            .connections
            .for_each_idle(|conn| {
                let enabled: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
                assert_eq!(enabled, 1);
                Ok(())
            })
            .unwrap();

        let invalid_target = SchemaDef {
            version: 3,
            ddl: vec!["CREATE TABLE invalid("],
            tables: Vec::new(),
        };
        assert!(
            DatabaseBuilder::<DestructiveFkDb>::default()
                .run_destructive(&db.run_sync(), &invalid_target)
                .is_err()
        );
        db.inner
            .pool
            .connections
            .for_each_idle(|conn| {
                let enabled: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
                assert_eq!(enabled, 1);
                Ok(())
            })
            .unwrap();
        drop(db);
        std::fs::remove_file(file).unwrap();
    }

    /// 다른 Rust path가 같은 SQLite TABLE을 가리키면 schema 단계에서 거부한다.
    #[test]
    fn duplicate_entity_table_names_are_rejected() {
        let schema = SchemaDef {
            version: 1,
            ddl: Vec::new(),
            tables: vec![
                TableMeta {
                    name: "items",
                    columns: &[],
                    ddl: &[],
                },
                TableMeta {
                    name: "ITEMS",
                    columns: &[],
                    ddl: &[],
                },
            ],
        };
        assert!(matches!(
            schema.validate_unique_tables(),
            Err(Error::Config(_))
        ));
    }

    /// 단일 테이블 스냅샷 생성 헬퍼
    fn snap(version: u32, cols: Vec<(&str, &str, bool)>) -> SchemaSnapshot {
        SchemaSnapshot {
            version,
            tables: vec![TableSnapshot {
                name: "t".into(),
                columns: cols
                    .into_iter()
                    .map(|(name, ty, not_null)| ColumnSnapshot {
                        name: name.into(),
                        sql_type: ty.into(),
                        not_null,
                        pk: name == "id",
                        renamed_from: None,
                    })
                    .collect(),
                ddl: vec![],
            }],
        }
    }

    /// 런타임 스냅샷 → 내장 스냅샷 (테스트 전용 leak)
    fn embed(snap: &SchemaSnapshot) -> EmbeddedSchema {
        let comp = roomrs_migrate::compress_snapshot(snap.to_json().unwrap().as_bytes());
        EmbeddedSchema {
            version: snap.version,
            compressed: Box::leak(comp.into_boxed_slice()),
        }
    }

    /// 안전 diff 쌍 = 스텝 합성, 구멍([1,3]) 건너 diff
    #[test]
    fn synthesize_spans_version_holes() {
        let v1 = snap(1, vec![("id", "INTEGER", true)]);
        let v3 = snap(3, vec![("id", "INTEGER", true), ("a", "TEXT", false)]);
        let v4 = snap(
            4,
            vec![
                ("id", "INTEGER", true),
                ("a", "TEXT", false),
                ("b", "TEXT", false),
            ],
        );
        let embedded = [embed(&v1), embed(&v3), embed(&v4)];
        let s = synthesize_embedded_steps(&embedded, &[], 1).unwrap();
        assert!(s.refused.is_empty());
        let spans: Vec<(u32, u32)> = s
            .steps
            .iter()
            .map(|m| (m.from_version(), m.to_version()))
            .collect();
        assert_eq!(spans, vec![(1, 3), (3, 4)], "인접 가용 쌍으로 합성");
    }

    /// 등록 스텝이 있는 from 구간은 합성하지 않는다 (등록 스텝 우선)
    #[test]
    fn synthesize_skips_registered_from() {
        let v1 = snap(1, vec![("id", "INTEGER", true)]);
        let v2 = snap(2, vec![("id", "INTEGER", true), ("a", "TEXT", false)]);
        let registered = [crate::migration::Migration::sql(1, 2, "SELECT 1")];
        let s = synthesize_embedded_steps(&[embed(&v1), embed(&v2)], &registered, 1).unwrap();
        assert!(s.steps.is_empty(), "등록 스텝 우선 — 합성 없음");
        assert!(s.refused.is_empty());
    }

    /// 파괴적 diff 쌍 = 합성 거부 + 체인 사전 검사에서 구체적 에러
    #[test]
    fn synthesize_refuses_destructive_pair() {
        let v1 = snap(1, vec![("id", "INTEGER", true), ("c", "TEXT", true)]);
        let v2 = snap(2, vec![("id", "INTEGER", true), ("c", "INTEGER", true)]);
        let s = synthesize_embedded_steps(&[embed(&v1), embed(&v2)], &[], 1).unwrap();
        assert!(s.steps.is_empty());
        assert!(s.refused.contains_key(&1), "{:?}", s.refused);

        // 체인이 갭에 닿으면 파괴적 안내 에러
        match check_destructive_gap(&[], &s, 1, 2) {
            Err(Error::Migration(msg)) => {
                assert!(msg.contains("v1->v2 자동 마이그레이션 불가"), "{msg}");
                assert!(msg.contains("파괴적 변경 포함"), "{msg}");
                assert!(msg.contains("fallback_to_destructive_migration"), "{msg}");
            }
            other => panic!("Migration 에러 기대, 결과: {other:?}"),
        }

        // 등록 스텝이 그 구간을 이으면 통과 (plan_chain으로 위임)
        let manual = crate::migration::Migration::sql(1, 2, "SELECT 1");
        assert!(check_destructive_gap(&[&manual], &s, 1, 2).is_ok());
    }

    /// 내장 스냅샷 파손 = Migration 에러 (한국어 메시지)
    #[test]
    fn synthesize_corrupt_embedded_errors() {
        let v1 = snap(1, vec![("id", "INTEGER", true)]);
        let bad = EmbeddedSchema {
            version: 2,
            compressed: b"\xff\x00\x12corrupt",
        };
        match synthesize_embedded_steps(&[embed(&v1), bad], &[], 1) {
            Err(Error::Migration(msg)) => assert!(msg.contains("내장 스냅샷"), "{msg}"),
            Err(other) => panic!("Migration 에러 기대, 결과: {other}"),
            Ok(_) => panic!("파손 스냅샷이 통과되면 안 된다"),
        }
    }

    /// 현재 버전 미만 구간은 건너뛴다 — 옛 스냅샷이 파손돼도 접근하지 않는다 (L-1)
    #[test]
    fn synthesize_skips_pairs_below_current() {
        let bad_old = EmbeddedSchema {
            version: 1,
            compressed: b"\xff\x00\x12corrupt",
        };
        let v2 = snap(2, vec![("id", "INTEGER", true)]);
        let v3 = snap(3, vec![("id", "INTEGER", true), ("a", "TEXT", false)]);
        let s = synthesize_embedded_steps(&[bad_old, embed(&v2), embed(&v3)], &[], 2).unwrap();
        let spans: Vec<(u32, u32)> = s
            .steps
            .iter()
            .map(|m| (m.from_version(), m.to_version()))
            .collect();
        assert_eq!(spans, vec![(2, 3)], "v1 파손 스냅샷은 압축 해제하지 않음");
        assert!(s.refused.is_empty());
    }
}
