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
pub(crate) fn install_preupdate_hook(conn: &Connection, columns: Arc<std::collections::HashMap<String, Vec<String>>>) -> Result<()> {
    use rusqlite::hooks::PreUpdateCase;
    use rusqlite::types::Value;

    conn.preupdate_hook(Some(move |_action, _db: &str, table: &str, change: &PreUpdateCase| {
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
            PreUpdateCase::Update { old_value_accessor, new_value_accessor } => (read_old(old_value_accessor), read_new(new_value_accessor)),
            PreUpdateCase::Unknown => return,
        };
        if old.is_some() || new.is_some() {
            record_hook_change(crate::live::TableChange { table: table.to_string(), old, new });
        }
    }))?;
    Ok(())
}

/// pool 재오픈에 필요한 connection 로컬 설정 소유본.
#[derive(Clone)]
struct ConnectionSettings {
    path: Option<PathBuf>,
    mem_name: Option<String>,
    busy_timeout: Duration,
    on_open: Option<ConnCallback>,
    #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
    encryption_key: Option<String>,
}

impl ConnectionSettings {
    /// on_open과 반환 불변식을 적용한다.
    fn initialize(&self, conn: &Connection) -> Result<()> {
        if let Some(cb) = &self.on_open {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(conn))).map_err(|_| Error::Internal("on_open 콜백 panic".into())).and_then(|result| result);
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
                Connection::open_with_flags(uri, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX)?
            }
            (None, Some(path)) => Connection::open(path)?,
            (None, None) => return Err(Error::Config("DB 경로가 설정되지 않았습니다".into())),
        };
        #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
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

/// Generated column meta from `#[column(generated = "…")]` (decision 54).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratedColumnMeta {
    /// Expression body for `GENERATED ALWAYS AS (…)`.
    pub expr: &'static str,
    /// `true` = STORED, `false` = VIRTUAL.
    pub stored: bool,
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
    /// `#[column(default = "…")]` 렌더 결과 SQL DEFAULT 식 (결정 53)
    pub default_sql: Option<&'static str>,
    /// `#[column(collate = "…")]` (결정 54)
    pub collate: Option<&'static str>,
    /// `#[column(generated = "…")]` (결정 54)
    pub generated: Option<GeneratedColumnMeta>,
}

/// Trigger SQL file hook meta from `#[entity(trigger = "…")]` (decision 46).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerMeta {
    /// Path relative to the crate manifest.
    pub path: &'static str,
    /// FNV-1a 64 of file bytes at macro expansion time.
    pub content_hash: u64,
}

/// 테이블 메타 — `#[database]`가 엔티티에서 수집
pub struct TableMeta {
    pub name: &'static str,
    pub columns: &'static [ColumnMeta],
    /// 테이블·인덱스 DDL
    pub ddl: &'static [&'static str],
    /// Trigger file hooks (decision 46)
    pub triggers: &'static [TriggerMeta],
    /// `#[entity(strict)]` (decision 54)
    pub strict: bool,
    /// `#[entity(without_rowid)]` (decision 54)
    pub without_rowid: bool,
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
            if self.tables[..index].iter().any(|previous| previous.name.eq_ignore_ascii_case(table.name)) {
                return Err(Error::Config(format!("database entities에 SQLite 테이블 이름 중복: {}", table.name)));
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
                            default_sql: c.default_sql.map(str::to_string),
                            collate: c.collate.map(str::to_string),
                            generated: c.generated.map(|g| roomrs_migrate::GeneratedColumnSnapshot { expr: g.expr.to_string(), stored: g.stored }),
                        })
                        .collect(),
                    ddl: t.ddl.iter().map(|d| d.to_string()).collect(),
                    triggers: t.triggers.iter().map(|tr| roomrs_migrate::TriggerSnapshot { path: tr.path.to_string(), content_hash: tr.content_hash }).collect(),
                    strict: t.strict,
                    without_rowid: t.without_rowid,
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
        let raw = roomrs_migrate::decompress_snapshot(self.compressed).map_err(|e| Error::Migration(format!("내장 스냅샷(v{}) 압축 해제 실패: {e}", self.version)))?;
        roomrs_migrate::SchemaSnapshot::from_slice(&raw).map_err(|e| Error::Migration(format!("내장 스냅샷(v{}) 파스 실패: {e}", self.version)))
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
    /// explicit export keeps reporting stale state after the file exists,
    /// until a rebuild re-expands the macro and embeds the snapshot —
    /// closing the fail-open window where `SNAPSHOT_HASH` and the embedded
    /// chain silently stay stale.
    const SNAPSHOT_FILE_SEEN: bool = true;
    /// 엔티티들의 DDL 수집
    fn schema() -> SchemaDef;
    /// core Database를 감싸 사용자 DB 타입 생성
    fn from_database(db: Database) -> Self;
}

/// 스냅샷 파일 생성 — 개발 플로우용 (명세 §7.4a, 결정 39).
///
/// 경로: `resolve_schema_dir(manifest_dir)/{DB_NAME}.{VERSION}.json`.
/// 동일 version 파일이 이미 있고 해시가 다르면 **덮어쓰지 않고** 에러.
/// 해시가 같으면 경로만 반환한다.
pub fn write_schema_snapshot<T: DatabaseSpec>(manifest_dir: &str) -> Result<std::path::PathBuf> {
    export_schema_snapshot::<T>(manifest_dir)
}

/// Export the current-version schema snapshot (decision 39/47).
///
/// - Missing file: create it and return the path.
/// - Existing file with matching hash: return the path (no rewrite).
/// - Existing file with different hash or corrupt: **do not overwrite**;
///   return [`Error::SnapshotStale`] asking to bump `version = N`.
pub fn export_schema_snapshot<T: DatabaseSpec>(manifest_dir: &str) -> Result<std::path::PathBuf> {
    plan_export_snapshot(T::DB_NAME, T::VERSION, &T::schema(), manifest_dir)?.write()
}

/// Planned snapshot write produced by preflight (decision 47 — multi-DB atomic).
#[derive(Debug)]
pub struct PlannedSnapshotWrite {
    /// Target path under `migrations/schema/`.
    pub path: std::path::PathBuf,
    /// When `Some`, the pretty JSON body to write. `None` means no-op (hash match).
    body: Option<String>,
    /// Database name for diagnostics.
    pub db_name: &'static str,
    /// Target version.
    pub version: u32,
}

impl PlannedSnapshotWrite {
    /// `true` when preflight found an identical on-disk snapshot (no file mutation).
    pub fn is_noop(&self) -> bool {
        self.body.is_none()
    }

    /// Apply a planned write. No-op plans return the path without touching the filesystem.
    pub fn write(self) -> Result<std::path::PathBuf> {
        if let Some(body) = self.body {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::Config(format!("스냅샷 디렉터리 생성 실패: {e}")))?;
            }
            // 원자 write: 임시 파일 후 rename
            let tmp = self.path.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&tmp, body.as_bytes()).map_err(|e| Error::Config(format!("스냅샷 임시 저장 실패: {e}")))?;
            std::fs::rename(&tmp, &self.path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                Error::Config(format!("스냅샷 저장 실패: {e}"))
            })?;
            log::info!("schema snapshot written: db={}, version={}, path={}", self.db_name, self.version, self.path.display());
        } else {
            log::debug!("schema snapshot already current: db={}, version={}, path={}", self.db_name, self.version, self.path.display());
        }
        Ok(self.path)
    }
}

/// Preflight one manual-version snapshot export without writing (decision 47).
pub fn plan_export_snapshot(db_name: &'static str, version: u32, schema: &SchemaDef, manifest_dir: &str) -> Result<PlannedSnapshotWrite> {
    schema.validate_unique_tables()?;
    let code = schema.to_snapshot();
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let path = roomrs_migrate::snapshot_path(&dir, db_name, version);
    if path.exists() {
        match roomrs_migrate::SchemaSnapshot::read_from(&path) {
            Ok(file) if file.hash() == code.hash() => {
                return Ok(PlannedSnapshotWrite { path, body: None, db_name, version });
            }
            Ok(_) => {
                return Err(Error::SnapshotStale(format!("스냅샷이 스테일입니다 — 동일 version 파일을 덮어쓰지 않습니다. `#[database(version = N)]`의 version을 올리고 새 스냅샷을 생성하세요: db={db_name}, path={}", path.display())));
            }
            Err(e) => {
                return Err(Error::SnapshotStale(format!("스냅샷 파일이 파손되었습니다 — 덮어쓰지 않습니다. 파일을 복구하거나 version을 올리세요 (db={db_name}, {}): {e}", path.display())));
            }
        }
    }
    let body = code.to_json().map_err(|e| Error::Config(format!("스냅샷 직렬화 실패: {e}")))?;
    Ok(PlannedSnapshotWrite { path, body: Some(body), db_name, version })
}

/// Inventory entry registered by `#[database]` for `cargo roomrs schema export` (decision 47/48).
pub struct SchemaExportEntry {
    /// snake_case database file prefix.
    pub db_name: &'static str,
    /// Compile-time version (manual N, or max-on-disk for auto).
    pub version: u32,
    /// `true` when declared as `version = auto`.
    pub auto: bool,
    /// Builds planned actions for this database (no filesystem mutation).
    pub plan: fn(&str) -> Result<Vec<PlannedExportAction>>,
}

inventory::collect!(SchemaExportEntry);

/// One filesystem action produced by export preflight.
#[derive(Debug)]
pub enum PlannedExportAction {
    /// Snapshot JSON write (or no-op).
    Snapshot(PlannedSnapshotWrite),
    /// Forward migration SQL draft (`migrations/{from}_{to}_roomrs_auto.sql`).
    SqlDraft { path: std::path::PathBuf, body: String },
}

impl PlannedExportAction {
    /// Apply action — no-op snapshot returns path without rewrite.
    pub fn write(self) -> Result<std::path::PathBuf> {
        match self {
            Self::Snapshot(p) => p.write(),
            Self::SqlDraft { path, body } => {
                if path.exists() {
                    return Err(Error::Config(format!("migration SQL 초안이 이미 있습니다 — 덮어쓰지 않습니다: {}", path.display())));
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| Error::Config(format!("migrations 디렉터리 생성 실패: {e}")))?;
                }
                let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
                std::fs::write(&tmp, body.as_bytes()).map_err(|e| Error::Config(format!("SQL 초안 임시 저장 실패: {e}")))?;
                std::fs::rename(&tmp, &path).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    Error::Config(format!("SQL 초안 저장 실패: {e}"))
                })?;
                log::info!("migration SQL draft written: path={}", path.display());
                Ok(path)
            }
        }
    }
}

/// Manual or auto export preflight (decision 48).
pub fn plan_export_for_entry(db_name: &'static str, compile_version: u32, auto: bool, schema: &SchemaDef, manifest_dir: &str) -> Result<Vec<PlannedExportAction>> {
    if auto { plan_export_auto(db_name, schema, manifest_dir) } else { Ok(vec![PlannedExportAction::Snapshot(plan_export_snapshot(db_name, compile_version, schema, manifest_dir)?)]) }
}

/// `version = auto` export plan: no-op / create v1 / next revision + safe SQL draft.
pub fn plan_export_auto(db_name: &'static str, schema: &SchemaDef, manifest_dir: &str) -> Result<Vec<PlannedExportAction>> {
    schema.validate_unique_tables()?;
    let mut entity_snap = schema.to_snapshot();
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let files = roomrs_migrate::list_snapshot_versions(&dir, db_name).map_err(|e| Error::Config(format!("스냅샷 스캔 실패: {e}")))?;

    if files.is_empty() {
        entity_snap.version = 1;
        let path = roomrs_migrate::snapshot_path(&dir, db_name, 1);
        let body = entity_snap.to_json().map_err(|e| Error::Config(format!("스냅샷 직렬화 실패: {e}")))?;
        return Ok(vec![PlannedExportAction::Snapshot(PlannedSnapshotWrite { path, body: Some(body), db_name, version: 1 })]);
    }

    let (latest_ver, latest_path) = files.last().cloned().ok_or_else(|| Error::Config(format!("스냅샷 목록이 비어 있습니다: {db_name}")))?;
    let latest = roomrs_migrate::SchemaSnapshot::read_from(&latest_path).map_err(|e| Error::SnapshotStale(format!("최신 스냅샷 파손 (db={db_name}, {}): {e}", latest_path.display())))?;
    entity_snap.version = latest_ver;
    if latest.hash() == entity_snap.hash() {
        return Ok(vec![PlannedExportAction::Snapshot(PlannedSnapshotWrite { path: latest_path, body: None, db_name, version: latest_ver })]);
    }

    let next = latest_ver.checked_add(1).ok_or_else(|| Error::Config(format!("version overflow for db={db_name}")))?;
    entity_snap.version = next;
    let plan = roomrs_migrate::diff_plan(&latest, &entity_snap);
    if !plan.destructive.is_empty() {
        return Err(Error::Config(format!("version=auto export 거부(db={db_name}): 파괴적/수동 변경 감지 — 수동 migration 후 version 을 명시하거나 검토하세요. 예: {}", plan.destructive.first().map(String::as_str).unwrap_or(""))));
    }
    let sql_path = std::path::PathBuf::from(manifest_dir).join("migrations").join(format!("{latest_ver}_{next}_roomrs_auto.sql"));
    if sql_path.exists() {
        return Err(Error::Config(format!("migration SQL 초안이 이미 있습니다 — 덮어쓰지 않습니다: {}", sql_path.display())));
    }
    let mut sql = roomrs_migrate::diff_sql(&latest, &entity_snap);
    let header = format!("-- roomrs auto forward draft: {db_name} v{latest_ver} -> v{next}\n-- Generated by `cargo roomrs schema export` (version=auto).\n-- Review before use. Not auto-registered; add via migrations_dir! or Migration::sql.\n");
    sql = format!("{header}{sql}");
    let mut actions = vec![PlannedExportAction::SqlDraft { path: sql_path, body: sql }];
    let snap_path = roomrs_migrate::snapshot_path(&dir, db_name, next);
    let body = entity_snap.to_json().map_err(|e| Error::Config(format!("스냅샷 직렬화 실패: {e}")))?;
    actions.push(PlannedExportAction::Snapshot(PlannedSnapshotWrite { path: snap_path, body: Some(body), db_name, version: next }));
    Ok(actions)
}

/// Run every registered schema export entry with multi-DB preflight then atomic writes.
///
/// Manual and auto modes must not mix in one export run (decision 48).
pub fn run_registered_schema_export(manifest_dir: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut all_actions: Vec<PlannedExportAction> = Vec::new();
    let mut names: Vec<&'static str> = Vec::new();
    let mut saw_auto: Option<bool> = None;
    for entry in inventory::iter::<SchemaExportEntry> {
        if names.iter().any(|n| n.eq_ignore_ascii_case(entry.db_name)) {
            return Err(Error::Config(format!("schema export registry 에 DB 이름 중복: {}", entry.db_name)));
        }
        if let Some(prev) = saw_auto {
            if prev != entry.auto {
                return Err(Error::Config("manual version 과 version=auto 를 같은 export 실행에서 섞을 수 없습니다 — 패키지를 분리하거나 모드를 통일하세요".into()));
            }
        } else {
            saw_auto = Some(entry.auto);
        }
        names.push(entry.db_name);
        log::debug!("schema export preflight: db={}, version={}, auto={}", entry.db_name, entry.version, entry.auto);
        all_actions.extend((entry.plan)(manifest_dir)?);
    }
    if all_actions.is_empty() {
        return Err(Error::Config("등록된 #[database]가 없습니다 — cargo roomrs가 탐색하는 lib/bin target에 database 선언이 있는지 확인하세요".into()));
    }
    let mut paths = Vec::with_capacity(all_actions.len());
    for action in all_actions {
        paths.push(action.write()?);
    }
    Ok(paths)
}

/// 스냅샷 ↔ 엔티티 메타 일치 검사 — CI/check용 (명세 §7.4a)
pub fn check_schema_snapshot<T: DatabaseSpec>(manifest_dir: &str) -> Result<()> {
    let schema = T::schema();
    schema.validate_unique_tables()?;
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let path = roomrs_migrate::snapshot_path(&dir, T::DB_NAME, T::VERSION);
    let file = roomrs_migrate::SchemaSnapshot::read_from(&path).map_err(|e| Error::SnapshotStale(format!("스냅샷 파일을 읽을 수 없습니다: {e}")))?;
    let code = schema.to_snapshot();
    if file.hash() != code.hash() {
        return Err(Error::SnapshotStale(format!("스냅샷과 엔티티 정의가 다릅니다 — 동일 version 덮어쓰기 금지. `#[database(version = N)]` version을 올리세요 (파일: {})", path.display())));
    }
    Ok(())
}

/// Read-only check of one registry entry against on-disk snapshots (decision 48, no file writes).
pub fn check_export_entry(db_name: &str, version: u32, auto: bool, schema: &SchemaDef, manifest_dir: &str) -> Result<()> {
    schema.validate_unique_tables()?;
    let dir = roomrs_migrate::resolve_schema_dir(manifest_dir);
    let files = roomrs_migrate::list_snapshot_versions(&dir, db_name).map_err(|e| Error::Config(format!("스냅샷 스캔 실패: {e}")))?;
    if files.is_empty() {
        return Err(Error::SnapshotStale(format!("snapshot missing for db={db_name} — run `cargo roomrs schema export` then `cargo build`")));
    }
    let (latest_ver, latest_path) = files.last().cloned().ok_or_else(|| Error::Config("empty snapshot list".into()))?;
    if auto {
        // auto: latest snapshot must match entity hash at that version
        let file = roomrs_migrate::SchemaSnapshot::read_from(&latest_path).map_err(|e| Error::SnapshotStale(format!("read {}: {e}", latest_path.display())))?;
        let mut code = schema.to_snapshot();
        code.version = latest_ver;
        if file.hash() != code.hash() {
            return Err(Error::SnapshotStale(format!("stale auto snapshot db={db_name} v{latest_ver} path={} — run `cargo roomrs schema export`", latest_path.display())));
        }
    } else {
        if latest_ver != version {
            return Err(Error::SnapshotStale(format!("version mismatch db={db_name}: code={version}, disk_latest={latest_ver} path={} — export or bump version", latest_path.display())));
        }
        let path = roomrs_migrate::snapshot_path(&dir, db_name, version);
        let file = roomrs_migrate::SchemaSnapshot::read_from(&path).map_err(|e| Error::SnapshotStale(format!("read {}: {e}", path.display())))?;
        let code = schema.to_snapshot();
        if file.hash() != code.hash() {
            return Err(Error::SnapshotStale(format!("stale manual snapshot db={db_name} v{version} path={} — bump version and export", path.display())));
        }
    }
    Ok(())
}

/// Walk inventory and check every registered DB without writing files (decision 47/48).
pub fn run_registered_schema_check(manifest_dir: &str) -> Result<()> {
    let mut count = 0usize;
    let mut saw_auto: Option<bool> = None;
    for entry in inventory::iter::<SchemaExportEntry> {
        if let Some(prev) = saw_auto {
            if prev != entry.auto {
                return Err(Error::Config("manual/auto version mix in registry".into()));
            }
        } else {
            saw_auto = Some(entry.auto);
        }
        // plan 클로저 안에서 schema 를 다시 만들지 않고, entry.plan 이 아닌 별도 check 가 필요.
        // plan 은 write 용이라 check 용 스키마는 사용자가 링크한 타입에 묶여 있다.
        // entry 에 schema 접근이 없으므로 plan 의 preflight 와 동일 로직을 check-only 로 재사용:
        // plan 호출은 쓰기를 하지 않으므로 여기서 plan 결과의 is_noop / 존재만 본다.
        let actions = (entry.plan)(manifest_dir)?;
        for a in &actions {
            match a {
                PlannedExportAction::Snapshot(p) if p.is_noop() => {}
                PlannedExportAction::Snapshot(p) => {
                    return Err(Error::SnapshotStale(format!("snapshot missing or stale: db={}, version={}, path={} — run `cargo roomrs schema export`", p.db_name, p.version, p.path.display())));
                }
                PlannedExportAction::SqlDraft { path, .. } => {
                    return Err(Error::SnapshotStale(format!("pending auto migration draft needed: {} — run `cargo roomrs schema export`", path.display())));
                }
            }
        }
        count += 1;
    }
    if count == 0 {
        return Err(Error::Config("no #[database] registry entries — ensure a selected lib/bin target links database types".into()));
    }
    log::info!("schema check ok: {count} database(s)");
    Ok(())
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
    /// notifier + live worker join 핸들 — drop 시 join으로 스레드 잔류 방지 (M-5, 결정 51)
    #[cfg(feature = "live")]
    live_joins: Vec<std::thread::JoinHandle<()>>,
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
    /// notifier·live worker 종료 신호 + join — 스레드 잔류 방지 (M-5, 결정 51).
    /// 마지막 Arc가 notifier/worker 자신(구독 콜백 등)에서 drop되면 self-join이
    /// 교착이므로 그 경우엔 join을 생략하고 분리한다 (H-3)
    fn drop(&mut self) {
        log::info!("database closing — shutting down live-query runtime");
        // 트래커 종료 — 레지스트리 청산으로 대기 중 recv를 깨운다 (M-7)
        self.tracker.shutdown();
        let current = std::thread::current().id();
        for h in self.live_joins.drain(..) {
            if h.thread().id() == current {
                log::warn!("database dropped on live-query thread — detaching join handle");
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
        let changed_tables: std::collections::HashSet<String> = capture.changes.iter().map(|change| change.table.clone()).collect();
        if !capture.changes.is_empty() {
            self.tracker.invalidate_changes(capture.changes);
        }
        match crate::live::extract_write_tables(sql) {
            crate::live::WriteTables::ReadOnly => {
                let tables: std::collections::HashSet<String> = capture.tables.difference(&changed_tables).cloned().collect();
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

    /// LiveQuery 관측성 스냅샷 (명세 §9.5 P2).
    ///
    /// SQL 파라미터·행 데이터는 포함하지 않는다. DB drop 이후 핸들을 더 이상
    /// 쓸 수 없으므로 스냅샷은 수명 동안 안전하게 읽는다.
    #[cfg(feature = "live")]
    pub fn live_metrics(&self) -> crate::live::LiveMetrics {
        self.inner.tracker.metrics_snapshot()
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
    /// LiveQuery 재조회 worker 수. `None` = `min(2, connections)` (결정 51).
    #[cfg(feature = "live")]
    notifier_readers: Option<usize>,
    /// LiveQuery DB 전역 debounce 기본값 (결정 49). 미설정 시 [`crate::DEFAULT_DEBOUNCE`].
    #[cfg(feature = "live")]
    live_debounce: Duration,
    busy_timeout: Duration,
    queue_timeout: Option<Duration>,
    migrate: MigrationPolicy,
    migrations: Vec<crate::migration::Migration>,
    auto_migrate: bool,
    destructive_fallback: bool,
    #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
    encryption_key: Option<String>,
    on_create: Option<ConnCallback>,
    on_open: Option<ConnCallback>,
    query_logger: Option<QueryLogger>,
    _spec: std::marker::PhantomData<T>,
}

impl<T: DatabaseSpec> Default for DatabaseBuilder<T> {
    /// 기본값 — 커넥션 수는 CPU 코어 기반(최대 4), busy_timeout 5초
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        Self {
            path: None,
            in_memory: false,
            connections: cores.clamp(1, 4) + 1,
            #[cfg(feature = "live")]
            notifier_readers: None,
            #[cfg(feature = "live")]
            live_debounce: crate::live::DEFAULT_DEBOUNCE,
            busy_timeout: Duration::from_secs(5),
            queue_timeout: None,
            migrate: MigrationPolicy::Auto,
            migrations: Vec::new(),
            auto_migrate: false,
            destructive_fallback: false,
            #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
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
    /// SQLite shared-cache `SQLITE_LOCKED` failures. LiveQuery workers also
    /// checkout from this single unified pool connection (decision 51).
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

    /// Sets how many LiveQuery refresh worker threads to spawn (decision 51).
    ///
    /// Default is `min(2, connections)`. Workers checkout from the unified
    /// read/write pool for each refresh and return the connection immediately
    /// after. There is no dedicated read-only connection pool.
    #[cfg(feature = "live")]
    pub fn notifier_readers(mut self, n: usize) -> Self {
        self.notifier_readers = Some(n.max(1));
        self
    }

    /// Sets the DB-wide default LiveQuery debounce window (decision 49).
    ///
    /// Default is [`crate::DEFAULT_DEBOUNCE`] (250ms). New LiveQuery observers
    /// copy this value at registration. Call [`crate::LiveQuery::debounce`] on
    /// an individual query to override it. The window is fixed coalesce: the
    /// first invalidation starts the window and further invalidations inside
    /// it merge without extending the deadline. `Duration::ZERO` refreshes on
    /// the next notifier turn.
    #[cfg(feature = "live")]
    pub fn live_debounce(mut self, delay: Duration) -> Self {
        self.live_debounce = delay;
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

    /// Sets the SQLCipher key before any other access on every connection.
    ///
    /// Available with `sqlcipher-bundled`, `sqlcipher-system`, or the legacy
    /// `cipher` alias.
    #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
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
    /// executed automatically (CREATE TABLE, nullable ADD COLUMN, NOT NULL
    /// ADD COLUMN with DEFAULT, valid RENAME COLUMN, CREATE INDEX). A gap
    /// whose diff contains destructive changes fails with a clear
    /// [`Error::Migration`] instead — register a manual step or use
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
    pub fn on_create(mut self, f: impl Fn(&Connection) -> Result<()> + Send + Sync + 'static) -> Self {
        self.on_create = Some(Arc::new(f));
        self
    }

    /// 오픈 시마다 콜백
    pub fn on_open(mut self, f: impl Fn(&Connection) -> Result<()> + Send + Sync + 'static) -> Self {
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

        // 스냅샷 부재·스테일 런타임 검증 (명세 §7.4b, 결정 39) —
        // 매크로 전개 때 현재 버전 파일이 없었으면 시작 실패.
        // 라이브러리 자체 테스트 등 온보딩 예외만 ROOMRS_ALLOW_MISSING_SNAPSHOT=1.
        if !T::SNAPSHOT_FILE_SEEN {
            let allow_missing = std::env::var("ROOMRS_ALLOW_MISSING_SNAPSHOT").as_deref() == Ok("1");
            if !allow_missing {
                return Err(Error::SnapshotStale("현재 버전 스냅샷이 없습니다 — `cargo test` export 또는 `export_schema_snapshot` 실행 후 재빌드하세요".into()));
            }
            log::warn!("snapshot file missing at expand time — ROOMRS_ALLOW_MISSING_SNAPSHOT=1, continuing");
        }
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
            Some(format!("file:roomrs_mem_{}?mode=memory&cache=shared", MEM_SEQ.fetch_add(1, Ordering::Relaxed)))
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

        // 라이브 쿼리 — preupdate_hook 을 일반 풀 커넥션에 설치 (명세 §9).
        // 전용 read-only 연결 없음. worker 는 통합 풀 checkout (결정 51).
        #[cfg(feature = "live")]
        let hook_columns = Arc::new(schema.tables.iter().map(|table| (table.name.to_ascii_lowercase(), table.columns.iter().map(|column| column.name.to_string()).collect())).collect());
        #[cfg(feature = "live")]
        for conn in &connections {
            install_preupdate_hook(conn, Arc::clone(&hook_columns))?;
        }

        let reconnect_settings = ConnectionSettings {
            path: self.path.clone(),
            mem_name: mem_name.clone(),
            busy_timeout: self.busy_timeout,
            on_open: self.on_open.clone(),
            #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
            encryption_key: self.encryption_key.clone(),
        };
        let connection_settings = reconnect_settings;
        #[cfg(feature = "live")]
        let connection_hook_columns = Arc::clone(&hook_columns);
        let connection_reopen: Arc<dyn Fn() -> Result<Connection> + Send + Sync> = Arc::new(move || {
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

        let pool_connections = Arc::new(ConnectionPool::new_with_preservation(connections, self.in_memory, self.in_memory, self.queue_timeout, connection_reopen));

        #[cfg(feature = "live")]
        let (tracker, live_joins) = {
            let worker_count = self.notifier_readers.unwrap_or_else(|| 2.min(self.connections));
            crate::live::Tracker::start(Arc::clone(&pool_connections), worker_count, self.live_debounce)?
        };

        let inner = DatabaseInner {
            pool: Pool { connections: pool_connections },
            query_logger: self.query_logger.take(),
            #[cfg(feature = "live")]
            tracker,
            #[cfg(feature = "live")]
            hook_columns: Arc::clone(&hook_columns),
            #[cfg(feature = "live")]
            live_joins,
        };
        let db = Database { inner: Arc::new(inner) };

        // 마이그레이션 — 풀 구성 후 Tx 기반으로 실행 (명세 §8)
        self.run_migration(&db, &schema)?;

        // 마이그레이션 완료 뒤 모든 풀 연결을 초기화한다.
        if let Some(cb) = &self.on_open {
            db.inner.pool.connections.for_each_idle(|conn| Self::apply_on_open(conn, cb, false, self.in_memory))?;
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
        log::info!("database opened: path={}, version={}", self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| ":memory:".into()), schema.version);

        Ok(T::from_database(db))
    }

    /// 커넥션 1개 오픈 + PRAGMA 설정
    fn open_conn(&self, mem_name: Option<&str>, _schema: &SchemaDef, _read_only: bool) -> Result<Connection> {
        let conn = match (mem_name, &self.path) {
            (Some(uri), _) => {
                use rusqlite::OpenFlags;
                Connection::open_with_flags(uri, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX)?
            }
            (None, Some(path)) => Connection::open(path)?,
            (None, None) => {
                return Err(Error::Config("DB 경로가 설정되지 않았습니다 — .sqlite(path) 또는 .in_memory() 필요".into()));
            }
        };

        // 암호화 키 — 어떤 접근보다 먼저 적용해야 한다.
        #[cfg(any(feature = "sqlcipher-bundled", feature = "sqlcipher-system"))]
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
            let deadline = std::time::Instant::now() + self.busy_timeout.max(Duration::from_millis(500));
            loop {
                match conn.pragma_update(None, "journal_mode", "WAL") {
                    Ok(()) => break,
                    Err(rusqlite::Error::SqliteFailure(fe, _)) if fe.code == rusqlite::ErrorCode::DatabaseBusy && std::time::Instant::now() < deadline => {
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
    fn apply_on_open(conn: &Connection, cb: &ConnCallback, read_only: bool, read_uncommitted: bool) -> Result<()> {
        let callback_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(conn))).map_err(|_| Error::Internal("on_open 콜백 panic".into())).and_then(|result| result);
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
        let current: u32 = h.with_connection(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))?;
        let target = schema.version;
        log::debug!("migration planning started: current={current}, target={target}, manual_steps={}, auto_migrate={}, destructive_fallback={}", self.migrations.len(), self.auto_migrate, self.destructive_fallback);

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
                    tx.raw_conn()?.execute_batch(ddl)?;
                }
                // on_create를 생성 트랜잭션 안에서 실행 — 실패 시 스키마와
                // user_version이 함께 롤백돼 다음 오픈이 생성을 재시도한다 (L-5)
                if let Some(cb) = &self.on_create {
                    cb(tx.raw_conn()?)?;
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
            log::trace!("schema version already current: version={target}");
            return Ok(());
        }

        if self.migrate == MigrationPolicy::Validate {
            log::error!("migration failed: schema version mismatch (db={current}, code={target}, policy=Validate)");
            return Err(Error::Migration(format!("스키마 버전 불일치: DB={current}, 코드={target} (Validate 정책)")));
        }

        // 자동 마이그레이션(옵트인, 명세 §8.4) — 등록 스텝이 없는 구간을
        // 내장 스냅샷 연속 쌍 diff의 안전 연산으로 합성해 메운다
        let synthesized = if self.auto_migrate { synthesize_embedded_steps(T::EMBEDDED_SCHEMAS, &self.migrations, current)? } else { SynthesizedSteps::default() };
        let all_steps: Vec<&crate::migration::Migration> = self.migrations.iter().chain(synthesized.steps.iter()).collect();

        // 스텝 체인 실행 — 스텝별 트랜잭션 + user_version 갱신.
        // 파괴적 구간 사전 검사를 먼저 — plan_chain의 일반 갭 에러보다 구체적 안내
        let plan_result = check_destructive_gap(&all_steps, &synthesized, current, target).and_then(|()| crate::migration::plan_chain(&all_steps, current, target));
        match plan_result {
            Ok(plan) => {
                log::debug!("migration plan prepared: current={current}, target={target}, steps={}", plan.len());
                for step in plan {
                    let from = step.from_version();
                    let to = step.to_version();
                    log::info!("migration step: v{}->v{}", from, to);
                    let result = h.transaction(|tx| {
                        // 락 확보 후 재확인 — 다른 프로세스가 이미 적용했으면 스킵 (M-4)
                        let cur: u32 = tx.query_scalar("PRAGMA user_version", [])?;
                        if cur >= to {
                            log::debug!("migration step skipped after version recheck: current={cur}, target={to}");
                            return Ok(());
                        }
                        // 체인 구성이 다른 프로세스의 개입 감지 — 스텝 시작 버전이
                        // 실제 버전과 다르면 잘못된 SQL 적용을 차단한다 (M-5)
                        if cur != from {
                            return Err(Error::Migration(format!("동시 마이그레이션 감지: 예상 v{}, 실제 v{cur} — 체인 구성 상이", from)));
                        }
                        step.run_up(tx)?;
                        tx.execute_batch(&format!("PRAGMA user_version = {to}"))
                    });
                    if let Err(e) = result {
                        log::error!("migration step failed: v{from}->v{to}: {e}");
                        return Err(e);
                    }
                    log::debug!("migration step completed: v{from}->v{to}");
                }
                Ok(())
            }
            Err(e) if self.destructive_fallback => {
                // 파괴적 폴백 (옵트인, 명세 §8) — 전부 삭제 후 최신 스키마로 재생성
                log::warn!("migration chain insufficient — falling back to destructive migration");
                let _ = e;
                self.run_destructive(&h, schema)?;
                log::info!("destructive migration completed: target={target}");
                Ok(())
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
                    let objs = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<std::result::Result<Vec<(String, String)>, _>>()?;
                    drop(statement);
                    // 의존 역순: trigger → view → index → table.
                    // sqlite_master의 이름은 임의 문자열 — 식별자 이스케이프 필수 (M-9).
                    for kind in ["trigger", "view", "index", "table"] {
                        for (t, name) in objs.iter().filter(|(t, _)| t == kind) {
                            c.execute_batch(&format!("DROP {} {}", t.to_uppercase(), escape_ident(name)))?;
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
fn synthesize_embedded_steps(embedded: &[EmbeddedSchema], registered: &[crate::migration::Migration], current: u32) -> Result<SynthesizedSteps> {
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
            log::info!("auto-migrate synthesized step: v{}->v{}", a.version, b.version);
            out.steps.push(crate::migration::Migration::sql(a.version, b.version, plan.safe.join(";\n")));
        } else {
            out.refused.insert(a.version, (b.version, plan.destructive.join("; ")));
        }
    }
    Ok(out)
}

/// 체인을 사전 답사해 파괴적 합성 거부 구간에 닿는지 검사 — 닿으면 실행 전에
/// 구체적 에러를 반환한다(일반 갭·형식 오류는 plan_chain이 보고).
fn check_destructive_gap(steps: &[&crate::migration::Migration], synthesized: &SynthesizedSteps, current: u32, target: u32) -> Result<()> {
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
        for builder in [DatabaseBuilder::<DestructiveFkDb>::default().in_memory().connections(3), DatabaseBuilder::<DestructiveFkDb>::default().connections(3).in_memory(), DatabaseBuilder::<DestructiveFkDb>::default().connections(3).sqlite(":memory:")] {
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
        let db = Arc::new(DatabaseBuilder::<DestructiveFkDb>::default().in_memory().connections(4).build().unwrap().0);
        let mut workers = Vec::new();
        for worker in 0..4 {
            let db = Arc::clone(&db);
            workers.push(std::thread::spawn(move || {
                for item in 0..25 {
                    db.run_sync().transaction(|tx| {
                        tx.execute("INSERT INTO parents(id) VALUES (?1)", [worker * 25 + item + 1])?;
                        Ok(())
                    })?;
                }
                Result::<()>::Ok(())
            }));
        }
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        let count: i64 = db.run_sync().query_scalar("SELECT COUNT(*) FROM parents", []).unwrap();
        assert_eq!(count, 100);
    }

    struct DestructiveFkDb(Database);

    impl DatabaseSpec for DestructiveFkDb {
        const VERSION: u32 = 1;
        const DB_NAME: &'static str = "destructive_fk_db";

        fn schema() -> SchemaDef {
            SchemaDef {
                version: 1,
                ddl: vec!["CREATE TABLE parents(id INTEGER PRIMARY KEY)", "CREATE TABLE children(id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL REFERENCES parents(id))"],
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
        let file = std::env::temp_dir().join(format!("roomrs-destructive-fk-{}-{}.db", std::process::id(), FILE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let db = DatabaseBuilder::<DestructiveFkDb>::default().sqlite(&file).connections(3).build().unwrap().0;
        db.run_sync().execute("INSERT INTO parents(id) VALUES (1)", []).unwrap();
        db.run_sync().execute("INSERT INTO children(id, parent_id) VALUES (1, 1)", []).unwrap();

        let target = SchemaDef {
            version: 2,
            ddl: vec!["CREATE TABLE replacements(id INTEGER PRIMARY KEY)"],
            tables: Vec::new(),
        };
        DatabaseBuilder::<DestructiveFkDb>::default().run_destructive(&db.run_sync(), &target).unwrap();

        db.inner
            .pool
            .connections
            .for_each_idle(|conn| {
                let enabled: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
                assert_eq!(enabled, 1);
                Ok(())
            })
            .unwrap();

        let invalid_target = SchemaDef { version: 3, ddl: vec!["CREATE TABLE invalid("], tables: Vec::new() };
        assert!(DatabaseBuilder::<DestructiveFkDb>::default().run_destructive(&db.run_sync(), &invalid_target).is_err());
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
                    triggers: &[],
                    strict: false,
                    without_rowid: false,
                },
                TableMeta {
                    name: "ITEMS",
                    columns: &[],
                    ddl: &[],
                    triggers: &[],
                    strict: false,
                    without_rowid: false,
                },
            ],
        };
        assert!(matches!(schema.validate_unique_tables(), Err(Error::Config(_))));
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
                        default_sql: None,
                        collate: None,
                        generated: None,
                    })
                    .collect(),
                ddl: vec![],
                triggers: vec![],
                strict: false,
                without_rowid: false,
            }],
        }
    }

    /// 런타임 스냅샷 → 내장 스냅샷 (테스트 전용 leak)
    fn embed(snap: &SchemaSnapshot) -> EmbeddedSchema {
        let comp = roomrs_migrate::compress_snapshot(snap.to_json().unwrap().as_bytes());
        EmbeddedSchema { version: snap.version, compressed: Box::leak(comp.into_boxed_slice()) }
    }

    /// 안전 diff 쌍 = 스텝 합성, 구멍([1,3]) 건너 diff
    #[test]
    fn synthesize_spans_version_holes() {
        let v1 = snap(1, vec![("id", "INTEGER", true)]);
        let v3 = snap(3, vec![("id", "INTEGER", true), ("a", "TEXT", false)]);
        let v4 = snap(4, vec![("id", "INTEGER", true), ("a", "TEXT", false), ("b", "TEXT", false)]);
        let embedded = [embed(&v1), embed(&v3), embed(&v4)];
        let s = synthesize_embedded_steps(&embedded, &[], 1).unwrap();
        assert!(s.refused.is_empty());
        let spans: Vec<(u32, u32)> = s.steps.iter().map(|m| (m.from_version(), m.to_version())).collect();
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
        let bad = EmbeddedSchema { version: 2, compressed: b"\xff\x00\x12corrupt" };
        match synthesize_embedded_steps(&[embed(&v1), bad], &[], 1) {
            Err(Error::Migration(msg)) => assert!(msg.contains("내장 스냅샷"), "{msg}"),
            Err(other) => panic!("Migration 에러 기대, 결과: {other}"),
            Ok(_) => panic!("파손 스냅샷이 통과되면 안 된다"),
        }
    }

    /// 현재 버전 미만 구간은 건너뛴다 — 옛 스냅샷이 파손돼도 접근하지 않는다 (L-1)
    #[test]
    fn synthesize_skips_pairs_below_current() {
        let bad_old = EmbeddedSchema { version: 1, compressed: b"\xff\x00\x12corrupt" };
        let v2 = snap(2, vec![("id", "INTEGER", true)]);
        let v3 = snap(3, vec![("id", "INTEGER", true), ("a", "TEXT", false)]);
        let s = synthesize_embedded_steps(&[bad_old, embed(&v2), embed(&v3)], &[], 2).unwrap();
        let spans: Vec<(u32, u32)> = s.steps.iter().map(|m| (m.from_version(), m.to_version())).collect();
        assert_eq!(spans, vec![(2, 3)], "v1 파손 스냅샷은 압축 해제하지 않음");
        assert!(s.refused.is_empty());
    }
}
