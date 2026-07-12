//! roomrs 단일 에러 타입 (명세 §12, 결정 로그 16)

/// roomrs 공용 Result 별칭
pub type Result<T, E = Error> = std::result::Result<T, E>;

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

/// rusqlite 에러 변환 — SQLITE_READONLY는 공개 호환용 `ReadOnly`로 승격한다.
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(fe, _) if fe.code == rusqlite::ErrorCode::ReadOnly => {
                Error::ReadOnly(e.to_string())
            }
            _ => Error::Sqlite(e),
        }
    }
}
