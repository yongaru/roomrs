// [명세 §8.4/결정 21c] 내장 스냅샷·diff·파일 스캔 API의 파사드 재수출 통합 검증.
// 매크로 경로(compile-time 임베드 + 자동 마이그레이션 e2e)는
// tests/ui/pass/schema_embed*.rs 픽스처가, 갭 합성 로직 단위 검증은
// roomrs-core 유닛 테스트가 담당한다.

use roomrs::SchemaSnapshot;

/// v1 스냅샷 JSON — items(id)
fn json_v1() -> &'static str {
    r#"{
      "version": 1,
      "tables": [{
        "name": "items",
        "columns": [
          { "name": "id", "sql_type": "INTEGER", "not_null": true, "pk": true }
        ],
        "ddl": ["CREATE TABLE \"items\" (\"id\" INTEGER PRIMARY KEY)"]
      }]
    }"#
}

/// v2 스냅샷 JSON — items(id, name·nullable) + 인덱스
fn json_v2() -> &'static str {
    r#"{
      "version": 2,
      "tables": [{
        "name": "items",
        "columns": [
          { "name": "id", "sql_type": "INTEGER", "not_null": true, "pk": true },
          { "name": "name", "sql_type": "TEXT", "not_null": false, "pk": false }
        ],
        "ddl": [
          "CREATE TABLE \"items\" (\"id\" INTEGER PRIMARY KEY, \"name\" TEXT)",
          "CREATE INDEX \"idx_items_name\" ON \"items\"(\"name\")"
        ]
      }]
    }"#
}

/// compress/decompress 왕복 + EmbeddedSchema::snapshot 파스 (결정 21c)
#[test]
fn embedded_snapshot_roundtrip() {
    let snap = SchemaSnapshot::from_slice(json_v1().as_bytes()).unwrap();
    let json = snap.to_json().unwrap();
    let comp = roomrs::compress_snapshot(json.as_bytes());
    assert_eq!(
        roomrs::decompress_snapshot(&comp).unwrap(),
        json.as_bytes(),
        "압축 왕복 무손실"
    );

    // 런타임 구성 EmbeddedSchema (테스트 전용 leak) — snapshot() 파스 왕복
    let embedded = roomrs::EmbeddedSchema {
        version: 1,
        compressed: Box::leak(comp.into_boxed_slice()),
    };
    let back = embedded.snapshot().unwrap();
    assert_eq!(back, snap);
    assert_eq!(back.hash(), snap.hash());

    // 파손 blob = Migration 에러 (한국어)
    let bad = roomrs::EmbeddedSchema {
        version: 9,
        compressed: b"\xff\x00corrupt",
    };
    match bad.snapshot() {
        Err(roomrs::Error::Migration(msg)) => assert!(msg.contains("내장 스냅샷"), "{msg}"),
        other => panic!("Migration 에러 기대, 결과: {other:?}"),
    }
}

/// diff_plan/diff_sql 파사드 재수출 — 안전/파괴적 분류 확인
#[test]
fn diff_plan_via_facade() {
    let v1 = SchemaSnapshot::from_slice(json_v1().as_bytes()).unwrap();
    let v2 = SchemaSnapshot::from_slice(json_v2().as_bytes()).unwrap();

    let plan: roomrs::DiffPlan = roomrs::diff_plan(&v1, &v2);
    assert_eq!(
        plan.safe,
        vec![
            r#"ALTER TABLE "items" ADD COLUMN "name" TEXT"#.to_string(),
            r#"CREATE INDEX "idx_items_name" ON "items"("name")"#.to_string(),
        ],
        "안전 연산: nullable ADD COLUMN + CREATE INDEX"
    );
    assert!(plan.destructive.is_empty(), "{plan:?}");

    // 역방향 = 파괴적 (DROP COLUMN + DROP INDEX)
    let back = roomrs::diff_plan(&v2, &v1);
    assert!(back.safe.is_empty(), "{back:?}");
    assert_eq!(back.destructive.len(), 2, "{back:?}");

    // diff_sql 초안 렌더 — 안전=실행문, 마무리 user_version
    let sql = roomrs::diff_sql(&v1, &v2);
    assert!(
        sql.contains(r#"ALTER TABLE "items" ADD COLUMN "name" TEXT;"#),
        "{sql}"
    );
    assert!(sql.contains("PRAGMA user_version = 2;"), "{sql}");
}

/// 버전 파일 스캔·경로 규칙 파사드 재수출 (결정 21)
#[test]
fn snapshot_dir_scan_via_facade() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = SchemaSnapshot::from_slice(json_v1().as_bytes()).unwrap();
    let v2 = SchemaSnapshot::from_slice(json_v2().as_bytes()).unwrap();

    assert_eq!(roomrs::snapshot_file_name("app_db", 2), "app_db.2.json");
    v1.write_to(&roomrs::snapshot_path(dir.path(), "app_db", 1))
        .unwrap();
    v2.write_to(&roomrs::snapshot_path(dir.path(), "app_db", 2))
        .unwrap();
    // 무관 파일 — 무시
    v1.write_to(&roomrs::snapshot_path(dir.path(), "other_db", 1))
        .unwrap();
    std::fs::write(dir.path().join("app_db.x.json"), "무시").unwrap();

    let files = roomrs::list_snapshot_versions(dir.path(), "app_db").unwrap();
    let versions: Vec<u32> = files.iter().map(|(v, _)| *v).collect();
    assert_eq!(versions, vec![1, 2], "오름차순 + 엄격 파스");

    // 디렉토리 부재 = 빈 목록 (온보딩 스킵 경로)
    assert!(
        roomrs::list_snapshot_versions(&dir.path().join("없음"), "app_db")
            .unwrap()
            .is_empty()
    );

    // resolve_schema_dir — env 미설정 시 manifest 표준 경로
    // (env 설정 케이스는 schema_snapshot.rs가 검증 — env는 프로세스 전역이라 여기서 만지지 않는다)
    if std::env::var("ROOMRS_SCHEMA_DIR").is_err() {
        assert_eq!(
            roomrs::resolve_schema_dir("/proj"),
            std::path::Path::new("/proj").join(roomrs::SCHEMA_DIR_RELATIVE)
        );
    }
}
