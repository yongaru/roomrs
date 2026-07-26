//! 구조화 오류 API 통합 테스트.

use roomrs::{Error, ErrorAdvice, ErrorPath};

/// 오류 경로와 조치 권고는 파사드에서 안정 식별자를 제공한다.
#[test]
fn error_path_and_advice_are_structured() {
    let cases = [
        (Error::NotFound, ErrorPath::Query, ErrorAdvice::CheckQuery),
        (Error::QueueTimeout(std::time::Duration::from_secs(1)), ErrorPath::ConnectionPool, ErrorAdvice::Retry),
        (Error::Migration("step missing".into()), ErrorPath::Migration, ErrorAdvice::ReviewMigration),
        (Error::SnapshotStale("hash differs".into()), ErrorPath::SchemaSnapshot, ErrorAdvice::RegenerateSnapshot),
        (Error::UnknownDependencies("view".into()), ErrorPath::LiveQuery, ErrorAdvice::DeclareDependencies),
        (Error::InvalidationFilter("no such column".into()), ErrorPath::LiveQuery, ErrorAdvice::CheckConfiguration),
        (Error::Config("invalid".into()), ErrorPath::Configuration, ErrorAdvice::CheckConfiguration),
        (Error::Internal("invariant".into()), ErrorPath::Internal, ErrorAdvice::InspectLogs),
        (Error::Closed, ErrorPath::LiveQuery, ErrorAdvice::ReopenDatabase),
    ];

    for (error, path, advice) in cases {
        assert_eq!(error.path(), path);
        assert_eq!(error.advice(), advice);
        assert!(!error.path().as_str().is_empty());
        assert!(!error.advice().as_str().is_empty());
    }
}
