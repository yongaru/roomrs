// InvalidationFilter 공개 계약 검증 — 행 조건과 일치하는 변경만 LiveQuery를 재조회한다.
#![cfg(feature = "live")]

use roomrs::{InvalidationFilter, LiveQuery, database, entity, params};
use std::time::Duration;

#[entity(table = "store_prefs")]
#[derive(Debug, Clone)]
struct StorePref {
    #[pk(autoincrement)]
    id: i64,
    data_path: Option<String>,
    profile_id: Option<i64>,
    value: String,
}

#[database(entities(StorePref), version = 1)]
struct Db;

/// emit 대기 헬퍼 — 최대 2초
fn next<T: Clone + Send + 'static>(query: &LiveQuery<T>) -> T {
    query
        .recv_timeout(Duration::from_secs(2))
        .expect("수신 에러")
        .expect("emit 타임아웃")
}

/// 행 필터는 AND/OR·NULL predicate와 INSERT/UPDATE/DELETE의 OLD/NEW 매칭을 지킨다.
#[test]
fn filtered_watch_matches_only_affected_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("invalidation-filter.db"))
        .build()
        .unwrap();
    let handle = db.run_sync();

    let filter = InvalidationFilter::table("store_prefs")
        .where_group(|group| group.eq("data_path", "aa.bb.dd").neq("profile_id", 0))
        .or_where_group(|group| group.is_null("data_path").is_not_null("profile_id"))
        .build()
        .unwrap();
    let query: LiveQuery<i64> = handle.watch_scalar_filtered(
        "SELECT COUNT(*) FROM store_prefs \
         WHERE (data_path = 'aa.bb.dd' AND profile_id != 0) \
            OR (data_path IS NULL AND profile_id IS NOT NULL)",
        params![],
        filter,
    );
    assert_eq!(next(&query), 0, "구독 즉시 현재 값 emit");

    // filter 밖 INSERT = 재조회 없음
    handle
        .execute(
            "INSERT INTO store_prefs(data_path, profile_id, value) VALUES ('other', 1, 'x')",
            params![],
        )
        .unwrap();
    assert!(
        query
            .recv_timeout(Duration::from_millis(300))
            .unwrap()
            .is_none(),
        "filter 밖 INSERT = emit 없음"
    );

    // UPDATE NEW가 filter 진입 = emit
    handle
        .execute(
            "UPDATE store_prefs SET data_path = 'aa.bb.dd' WHERE data_path = 'other'",
            params![],
        )
        .unwrap();
    assert_eq!(next(&query), 1, "UPDATE NEW 매칭 = emit");

    // UPDATE OLD가 filter 이탈 = emit
    handle
        .execute(
            "UPDATE store_prefs SET profile_id = 0 WHERE data_path = 'aa.bb.dd'",
            params![],
        )
        .unwrap();
    assert_eq!(next(&query), 0, "UPDATE OLD 매칭 = emit");

    // OR NULL 그룹 INSERT = emit
    handle
        .execute(
            "INSERT INTO store_prefs(data_path, profile_id, value) VALUES (NULL, 7, 'null')",
            params![],
        )
        .unwrap();
    assert_eq!(next(&query), 1, "OR NULL 그룹 INSERT = emit");

    // DELETE OLD 매칭 = emit
    handle
        .execute(
            "DELETE FROM store_prefs WHERE data_path IS NULL AND profile_id = 7",
            params![],
        )
        .unwrap();
    assert_eq!(next(&query), 0, "DELETE OLD 매칭 = emit");
}

/// 커밋한 변경만 필터된 구독에 전달하고 롤백 변경은 버린다.
#[test]
fn filtered_watch_waits_for_commit_and_discards_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("invalidation-filter-tx.db"))
        .build()
        .unwrap();
    let handle = db.run_sync();
    let filter = InvalidationFilter::table("store_prefs")
        .where_group(|group| group.eq("data_path", "aa.bb.dd"))
        .build()
        .unwrap();
    let query: LiveQuery<i64> = handle.watch_scalar_filtered(
        "SELECT count(*) FROM store_prefs WHERE data_path = 'aa.bb.dd'",
        params![],
        filter,
    );
    assert_eq!(next(&query), 0);

    let tx = handle.begin().unwrap();
    tx.execute(
        "INSERT INTO store_prefs(data_path, profile_id, value) VALUES ('aa.bb.dd', 1, 'commit')",
        params![],
    )
    .unwrap();
    assert!(
        query
            .recv_timeout(Duration::from_millis(100))
            .unwrap()
            .is_none()
    );
    tx.commit().unwrap();
    assert_eq!(next(&query), 1);

    let tx = handle.begin().unwrap();
    tx.execute(
        "INSERT INTO store_prefs(data_path, profile_id, value) VALUES ('aa.bb.dd', 1, 'rollback')",
        params![],
    )
    .unwrap();
    tx.rollback().unwrap();
    assert!(
        query
            .recv_timeout(Duration::from_millis(300))
            .unwrap()
            .is_none()
    );
}

/// UPSERT의 conflict UPDATE도 OLD/NEW 행으로 필터를 판정한다.
#[test]
fn filtered_watch_matches_upsert_conflict_update() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::builder()
        .sqlite(dir.path().join("invalidation-filter-upsert.db"))
        .build()
        .unwrap();
    let handle = db.run_sync();
    handle
        .execute(
            "INSERT INTO store_prefs(id, data_path, profile_id, value) VALUES (10, 'other', 1, 'old')",
            params![],
        )
        .unwrap();
    let filter = InvalidationFilter::table("store_prefs")
        .where_group(|group| group.eq("data_path", "aa.bb.dd"))
        .build()
        .unwrap();
    let query: LiveQuery<i64> = handle.watch_scalar_filtered(
        "SELECT count(*) FROM store_prefs WHERE data_path = 'aa.bb.dd'",
        params![],
        filter,
    );
    assert_eq!(next(&query), 0);

    handle
        .execute(
            "INSERT INTO store_prefs(id, data_path, profile_id, value) VALUES (10, 'aa.bb.dd', 1, 'new') \
             ON CONFLICT(id) DO UPDATE SET data_path = excluded.data_path, value = excluded.value",
            params![],
        )
        .unwrap();
    assert_eq!(next(&query), 1);
}
