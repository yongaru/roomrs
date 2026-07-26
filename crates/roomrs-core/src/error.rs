//! roomrs 단일 에러 타입 (명세 §12, 결정 로그 16)

/// roomrs 공용 Result 별칭
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Stable subsystem where an [`Error`] originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorPath {
    /// SQLite engine, connection, or SQL execution.
    Database,
    /// Query result shape or row lookup.
    Query,
    /// Connection-pool checkout, maintenance, or recovery.
    ConnectionPool,
    /// Versioned schema migration.
    Migration,
    /// Committed schema snapshot validation.
    SchemaSnapshot,
    /// Live-query lifecycle or dependency tracking.
    LiveQuery,
    /// Builder, macro, or caller-supplied configuration.
    Configuration,
    /// Data serialization or conversion.
    Serialization,
    /// roomrs invariant or isolated callback failure.
    Internal,
}

impl ErrorPath {
    /// Returns a stable machine-readable path identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Query => "query",
            Self::ConnectionPool => "connection_pool",
            Self::Migration => "migration",
            Self::SchemaSnapshot => "schema_snapshot",
            Self::LiveQuery => "live_query",
            Self::Configuration => "configuration",
            Self::Serialization => "serialization",
            Self::Internal => "internal",
        }
    }
}

/// Recommended next action for an [`Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorAdvice {
    /// Retry the operation after a short backoff.
    Retry,
    /// Check the query, parameters, and expected row count.
    CheckQuery,
    /// Check database permissions, connection state, and SQLite diagnostics.
    CheckDatabase,
    /// Review or add a forward migration before retrying.
    ReviewMigration,
    /// Generate and commit the current version schema snapshot.
    RegenerateSnapshot,
    /// Declare live-query dependencies with `watching` or `watching_all`.
    DeclareDependencies,
    /// Correct builder, macro, or runtime configuration.
    CheckConfiguration,
    /// Correct serialization format or custom SQL type conversion.
    CheckDataFormat,
    /// Inspect logs and report the invariant failure with reproduction details.
    InspectLogs,
    /// No retry is useful because the resource is closed.
    ReopenDatabase,
}

impl ErrorAdvice {
    /// Returns a stable machine-readable advice identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::CheckQuery => "check_query",
            Self::CheckDatabase => "check_database",
            Self::ReviewMigration => "review_migration",
            Self::RegenerateSnapshot => "regenerate_snapshot",
            Self::DeclareDependencies => "declare_dependencies",
            Self::CheckConfiguration => "check_configuration",
            Self::CheckDataFormat => "check_data_format",
            Self::InspectLogs => "inspect_logs",
            Self::ReopenDatabase => "reopen_database",
        }
    }
}

/// roomrs 단일 에러 타입 — 공개 API는 전부 이 타입으로 반환한다
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// SQLite 하부 에러 (rusqlite 위임 — readonly 계열은 `ReadOnly`로 매핑, L-2)
    #[error("SQLite 에러: {0}")]
    Sqlite(rusqlite::Error),

    /// 정확히 1건을 기대했으나 0건 (명세 §5.2 반환 타입 규칙)
    #[error("행을 찾을 수 없습니다 (정확히 1건 기대, 0건 반환)")]
    NotFound,

    /// SQLite가 읽기 전용 상태 또는 파일로 판단한 write.
    #[error("읽기 전용 커넥션에 쓰기를 시도했습니다: {0}")]
    ReadOnly(String),

    /// 통합 풀 checkout 대기 타임아웃 (명세 §10 큐 정책)
    #[error("커넥션 풀 대기가 타임아웃되었습니다 ({0:?})")]
    QueueTimeout(std::time::Duration),

    /// 마이그레이션 실패/불가
    #[error("마이그레이션 에러: {0}")]
    Migration(String),

    /// 스키마 스냅샷 스테일 (명세 §7.4 — M1c에서 사용)
    #[error("스키마 스냅샷이 오래되었습니다: {0}")]
    SnapshotStale(String),

    /// 라이브 쿼리 의존 테이블 추출 실패 (명세 §5.7 — M4에서 사용)
    #[error("쿼리의 의존 테이블을 알 수 없습니다: {0}")]
    UnknownDependencies(String),

    /// InvalidationFilter 테이블·컬럼이 스키마에 없음 (명세 §9.5 P1)
    #[error("무효화 필터 스키마 검증 실패: {0}")]
    InvalidationFilter(String),

    /// 빌더/설정 오류
    #[error("설정 에러: {0}")]
    Config(String),

    /// 내부 불변식 위반 — 워커 응답 유실 등 (버그 신고 대상)
    #[error("내부 에러: {0}")]
    Internal(String),

    /// 데이터베이스 종료 — 라이브 쿼리 채널 닫힘 (M-7)
    #[error("데이터베이스가 종료되어 라이브 쿼리 채널이 닫혔습니다")]
    Closed,

    /// JSON 직렬화/역직렬화 실패 (`#[json]` 필드)
    #[cfg(feature = "json")]
    #[error("JSON 변환 에러: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// Returns the stable subsystem that produced this error.
    pub const fn path(&self) -> ErrorPath {
        match self {
            Self::Sqlite(_) | Self::ReadOnly(_) => ErrorPath::Database,
            Self::NotFound => ErrorPath::Query,
            Self::QueueTimeout(_) => ErrorPath::ConnectionPool,
            Self::Migration(_) => ErrorPath::Migration,
            Self::SnapshotStale(_) => ErrorPath::SchemaSnapshot,
            Self::UnknownDependencies(_) | Self::InvalidationFilter(_) | Self::Closed => ErrorPath::LiveQuery,
            Self::Config(_) => ErrorPath::Configuration,
            Self::Internal(_) => ErrorPath::Internal,
            #[cfg(feature = "json")]
            Self::Json(_) => ErrorPath::Serialization,
        }
    }

    /// Returns the recommended next action for this error.
    pub const fn advice(&self) -> ErrorAdvice {
        match self {
            Self::Sqlite(_) | Self::ReadOnly(_) => ErrorAdvice::CheckDatabase,
            Self::NotFound => ErrorAdvice::CheckQuery,
            Self::QueueTimeout(_) => ErrorAdvice::Retry,
            Self::Migration(_) => ErrorAdvice::ReviewMigration,
            Self::SnapshotStale(_) => ErrorAdvice::RegenerateSnapshot,
            Self::UnknownDependencies(_) => ErrorAdvice::DeclareDependencies,
            Self::InvalidationFilter(_) => ErrorAdvice::CheckConfiguration,
            Self::Config(_) => ErrorAdvice::CheckConfiguration,
            Self::Internal(_) => ErrorAdvice::InspectLogs,
            Self::Closed => ErrorAdvice::ReopenDatabase,
            #[cfg(feature = "json")]
            Self::Json(_) => ErrorAdvice::CheckDataFormat,
        }
    }
}

/// rusqlite 에러 변환 — SQLITE_READONLY는 공개 호환용 `ReadOnly`로 승격한다.
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(fe, _) if fe.code == rusqlite::ErrorCode::ReadOnly => Error::ReadOnly(e.to_string()),
            _ => Error::Sqlite(e),
        }
    }
}
