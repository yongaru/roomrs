//! # roomrs
//!
//! Room-style local SQLite persistence library for Rust.
//!
//! This is the facade crate: it re-exports the public API from the internal
//! crates. See the repository README for the full design document.
#![deny(unsafe_code)]

// 코어 표면
pub use roomrs_core::{Col, Execute, Expr, IntoDbValue, Order, Query, SelectBuilder, col};
pub use roomrs_core::{
    ColumnMeta, ColumnSnapshot, Database, DatabaseBuilder, DatabaseSpec, DiffPlan, EmbeddedSchema,
    Entity, Error, FromRow, Insertable, Migration, MigrationPolicy, MigrationStep, Params, Result,
    SCHEMA_DIR_RELATIVE, SchemaDef, SchemaSnapshot, SqlContext, SyncHandle, TableMeta,
    TableSnapshot, ToSql, ToSqlOutput, Tx, check_schema_snapshot, compress_snapshot,
    decompress_snapshot, diff_plan, diff_sql, export_schema_for_test, list_snapshot_versions,
    outputs_to_values, params, params_from_iter, resolve_schema_dir, rusqlite, snapshot_file_name,
    snapshot_path, to_owned_value, write_schema_snapshot,
};
pub use roomrs_core::{RelationView, in_placeholders, load_children, load_junction};

// 라이브 쿼리 (명세 §5.6, §9)
#[cfg(feature = "live")]
pub use roomrs_core::{
    InvalidationFilter, InvalidationFilterBuilder, InvalidationGroupBuilder, LiveQuery,
    SubscriptionGuard, WatchContext,
};

// 매크로
pub use roomrs_macros::{Relation, dao, database, entity, migrations_dir};

// 매크로 생성 코드 전용 — 직접 사용 금지
#[doc(hidden)]
pub use roomrs_core::__private;

// 비동기 파사드 (명세 §2.4)
#[cfg(feature = "async")]
pub use roomrs_async::{AsyncHandle, BuildAsyncExt};

/// 매크로 생성 async 코드 스위치 — roomrs가 `async` feature로 빌드됐을 때만 코드 방출.
/// 매크로 생성 코드 전용 — 직접 사용 금지.
#[cfg(feature = "async")]
#[doc(hidden)]
#[macro_export]
macro_rules! __if_async {
    ($($t:tt)*) => { $($t)* };
}

/// async 비활성 빌드 — 생성 async 코드를 통째로 제거 (순수 동기, 명세 §2.4)
#[cfg(not(feature = "async"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __if_async {
    ($($t:tt)*) => {};
}
