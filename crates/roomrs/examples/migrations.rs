//! 마이그레이션 사용 케이스 (명세 §8)
//! - v1 DB 생성 → v3 코드로 열면서 스텝 체인(1→2→3) 실행
//! - Migration::sql / Migration::code 혼합
//! - diff_sql 초안 생성 출력
//!
//! 실행: cargo run --example migrations

use roomrs::{DatabaseSpec, Migration, dao, database, entity, params};

// ── v1 스키마 ──
#[entity(table = "docs")]
struct DocV1 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
}
#[database(entities(DocV1), version = 1)]
struct DbV1;

// ── v3 스키마 (note, done 추가) ──
#[entity(table = "docs")]
#[derive(Debug)]
struct DocV3 {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    note: String,
    done: bool,
}

#[dao]
trait DocDao {
    #[query("SELECT * FROM docs ORDER BY id")]
    fn all(&self) -> roomrs::Result<Vec<DocV3>>;
}

#[database(entities(DocV3), daos(DocDao), version = 3)]
struct DbV3;

fn main() -> roomrs::Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("m.db");

    // 1) v1 DB 생성 + 데이터
    {
        let db = DbV1::builder().sqlite(&path).build()?;
        db.run_sync()
            .execute("INSERT INTO docs (title) VALUES ('구버전 행')", params![])?;
    }

    // 2) v3 코드로 열기 — 스텝 체인 자동 실행
    let db = DbV3::builder()
        .sqlite(&path)
        .migration(Migration::sql(
            1,
            2,
            r#"ALTER TABLE "docs" ADD COLUMN "note" TEXT NOT NULL DEFAULT ''"#,
        ))
        .migration(Migration::code(2, 3, |tx| {
            // 코드 스텝 — 임의 로직 가능
            tx.execute_batch(r#"ALTER TABLE "docs" ADD COLUMN "done" INTEGER NOT NULL DEFAULT 0"#)
        }))
        .build()?;

    let v: i64 = db
        .run_sync()
        .query_scalar("PRAGMA user_version", params![])?;
    println!("마이그레이션 완료: user_version = {v}");
    for d in db.run_sync().doc_dao().all()? {
        println!("  {d:?}  (기존 데이터 보존)");
    }

    // 3) diff 초안 — v1 스냅샷 vs v3 스냅샷 (자동 실행은 안 함, 검토용)
    let old = <DbV1 as DatabaseSpec>::schema().to_snapshot();
    let new = <DbV3 as DatabaseSpec>::schema().to_snapshot();
    println!(
        "\n--- diff 초안 (roomrs migrate diff 동일) ---\n{}",
        roomrs::diff_sql(&old, &new)
    );
    Ok(())
}
