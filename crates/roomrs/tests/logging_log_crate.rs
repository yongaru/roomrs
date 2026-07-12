//! log crate 통합 검증 (지시서 logging-log-crate) —
//! roomrs 내부 log 방출을 캡처 로거로 수집해 영어 메시지 조각을 확인한다.
//!
//! 전역 로거는 프로세스당 1회만 설정 가능하므로 이 파일에는 테스트를 1개만 둔다
//! (통합 테스트 파일 = 독립 바이너리 = 독립 프로세스).

use roomrs::{Migration, database, entity};
use std::sync::{Mutex, OnceLock};

/// 캡처된 로그 레코드 버퍼 (레벨 + 메시지)
static RECORDS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// 버퍼 접근 헬퍼 — 최초 접근 시 초기화
fn records() -> &'static Mutex<Vec<String>> {
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// 테스트용 캡처 로거 — 레코드를 문자열로 축적
struct CaptureLogger;

static LOGGER: CaptureLogger = CaptureLogger;

impl log::Log for CaptureLogger {
    /// 모든 레벨 수집
    fn enabled(&self, _m: &log::Metadata<'_>) -> bool {
        true
    }

    /// 레벨 + 메시지를 버퍼에 축적
    fn log(&self, record: &log::Record<'_>) {
        records()
            .lock()
            .unwrap()
            .push(format!("{} {}", record.level(), record.args()));
    }

    /// 버퍼링 없음
    fn flush(&self) {}
}

// ───── v1 스키마 ─────
#[entity(table = "log_items")]
struct ItemV1 {
    #[pk(autoincrement)]
    id: i64,
    name: String,
}

#[database(entities(ItemV1), version = 1)]
struct LogDb1;

// ───── v2 스키마 (note 추가) ─────
#[entity(table = "log_items")]
struct ItemV2 {
    #[pk(autoincrement)]
    id: i64,
    name: String,
    note: String,
}

#[database(entities(ItemV2), version = 2)]
struct LogDb2;

/// open → write → 트랜잭션 → close → 마이그레이션 체인의 log 방출 검증.
/// 메시지는 영어(지시서 규칙), 파라미터 값은 로그에 노출되지 않아야 한다.
#[test]
fn emits_log_records_for_open_write_migration() {
    // 전역 로거 설치 — 프로세스당 1회
    log::set_logger(&LOGGER).expect("전역 로거 설정 실패");
    log::set_max_level(log::LevelFilter::Trace);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.db");

    // v1 오픈(신규 스키마 생성) + 단문 write + 트랜잭션 write
    {
        let db = LogDb1::builder().sqlite(&path).build().unwrap();
        let h = db.run_sync();
        h.execute(
            "INSERT INTO log_items (name) VALUES (?1)",
            roomrs::params!["secret-value-1"],
        )
        .unwrap();
        h.transaction(|tx| {
            tx.execute(
                "INSERT INTO log_items (name) VALUES (?1)",
                roomrs::params!["secret-value-2"],
            )
            .map(|_| ())
        })
        .unwrap();
    } // drop = close 로그

    // v2 마이그레이션 체인 (1→2)
    let db2 = LogDb2::builder()
        .sqlite(&path)
        .migration(Migration::sql(
            1,
            2,
            r#"ALTER TABLE "log_items" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#,
        ))
        .build()
        .unwrap();
    drop(db2);

    let all = records().lock().unwrap().join("\n");

    // 느슨한 조각 매칭 — 핵심 영어 조각만 확인 (문구 전체 고정 금지)
    assert!(
        all.contains("database opened"),
        "open info 로그 없음:\n{all}"
    );
    assert!(
        all.contains("schema created at version 1"),
        "신규 스키마 info 로그 없음:\n{all}"
    );
    assert!(
        all.contains("migration step: v1->v2"),
        "마이그레이션 스텝 info 로그 없음:\n{all}"
    );
    assert!(
        all.contains("transaction begin"),
        "tx begin debug 로그 없음:\n{all}"
    );
    assert!(
        all.contains("transaction commit"),
        "tx commit debug 로그 없음:\n{all}"
    );
    assert!(
        all.contains("SQL: INSERT INTO log_items"),
        "SQL trace 로그 없음:\n{all}"
    );
    assert!(
        all.contains("database closed"),
        "close info 로그 없음:\n{all}"
    );

    // 민감정보 금지 — 파라미터 값은 어떤 로그에도 나타나면 안 된다
    assert!(
        !all.contains("secret-value"),
        "파라미터 값이 로그에 노출됨:\n{all}"
    );
}
