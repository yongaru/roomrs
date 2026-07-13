//! roomrs-core — core runtime: `Database`, connection pool, errors,
//! invalidation tracker, notifier, migration runner.
//!
//! Internal crate — use the `roomrs` facade instead.
#![deny(unsafe_code)]

// bundled와 cipher는 상호 배타 — libsqlite3-sys가 어차피 실패하지만 명확한 메시지 제공
#[cfg(all(feature = "bundled", feature = "cipher"))]
compile_error!(
    "feature 'bundled'와 'cipher'는 함께 켤 수 없습니다 — cipher 사용 시 default-features=false로 bundled를 끄세요"
);

mod database;
mod entity;
mod error;
mod handle;
#[cfg(feature = "live")]
mod live;
mod migration;
#[cfg(feature = "multi-instance")]
mod multi_instance;
mod pool;
mod query;
mod relation;
mod row;

pub use database::{
    ColumnMeta, Database, DatabaseBuilder, DatabaseInner, DatabaseSpec, EmbeddedSchema,
    MigrationPolicy, SchemaDef, TableMeta, check_schema_snapshot, export_schema_for_test,
    write_schema_snapshot,
};
pub use entity::{Entity, Insertable, outputs_to_values, to_owned_value};
pub use error::{Error, Result};
#[cfg(feature = "live")]
pub use handle::WatchContext;
pub use handle::{SqlContext, SyncHandle, Tx};
#[cfg(feature = "live")]
pub use live::{
    InvalidationFilter, InvalidationFilterBuilder, InvalidationGroupBuilder, LiveQuery,
    SubscriptionGuard,
};
pub use migration::{Migration, MigrationStep};
pub use query::{Col, Execute, Expr, IntoDbValue, Order, Query, SelectBuilder, col};
pub use relation::{RelationView, in_placeholders, load_children, load_junction};
pub use roomrs_migrate::{
    ColumnSnapshot, DiffPlan, SCHEMA_DIR_RELATIVE, SchemaSnapshot, TableSnapshot,
    compress_snapshot, decompress_snapshot, diff_plan, diff_sql, list_snapshot_versions,
    resolve_schema_dir, snapshot_file_name, snapshot_path,
};
pub use row::FromRow;

// 매크로 생성 코드·사용자 코드가 쓰는 rusqlite 표면 재수출
pub use rusqlite;
pub use rusqlite::types::ToSqlOutput;
pub use rusqlite::{Params, ToSql, params, params_from_iter};

/// 매크로 생성 코드 전용 내부 재수출 — 직접 사용 금지
#[doc(hidden)]
pub mod __private {
    pub use rusqlite::Row;

    #[cfg(feature = "json")]
    pub use serde_json;
}
