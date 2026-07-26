//! 간이 처리량 벤치 (명세 §15 M7 — 본격 criterion 벤치는 후속)
//!
//! 실행: cargo run --release --example bench

use roomrs::{dao, database, entity, params};
use std::time::Instant;
mod support;

#[entity(table = "rows")]
#[derive(Debug, Clone)]
struct Row {
    #[pk(autoincrement)]
    id: i64,
    payload: String,
    n: i64,
}

#[dao]
trait RowDao {
    #[insert]
    fn add(&self, r: &Row) -> roomrs::Result<i64>;
}

#[database(entities(Row), daos(RowDao), version = 1)]
struct Db;

/// 구간 측정 헬퍼
fn bench(name: &str, count: u64, f: impl FnOnce() -> roomrs::Result<()>) -> roomrs::Result<()> {
    let start = Instant::now();
    f()?;
    let el = start.elapsed();
    let per_sec = count as f64 / el.as_secs_f64();
    println!("{name:<28} {count:>8}건  {el:>10.2?}  {per_sec:>12.0}/s");
    Ok(())
}

fn main() -> roomrs::Result<()> {
    support::init_tracing();
    let dir = tempfile::tempdir().map_err(|e| roomrs::Error::Config(format!("tempdir 생성 실패: {e}")))?;
    let db = Db::builder().sqlite(dir.path().join("bench.db")).build()?;
    let h = db.run_sync();
    let dao = h.row_dao();

    const N: u64 = 10_000;

    // 개별 insert (자동 커밋 — fsync 포함이라 느린 게 정상)
    bench("insert (개별 커밋)", 1_000, || {
        for i in 0..1_000 {
            dao.add(&Row { id: 0, payload: format!("p{i}"), n: i })?;
        }
        Ok(())
    })?;

    // 트랜잭션 일괄 insert
    use DbTxDaos as _;
    bench("insert (단일 트랜잭션)", N, || {
        h.transaction(|tx| {
            for i in 0..N {
                tx.row_dao().add(&Row { id: 0, payload: format!("p{i}"), n: i as i64 })?;
            }
            Ok(())
        })
    })?;

    // 인덱스 없는 조회
    bench("query_scalar COUNT", 1_000, || {
        for _ in 0..1_000 {
            let _: i64 = h.query_scalar("SELECT COUNT(*) FROM rows", params![])?;
        }
        Ok(())
    })?;

    // 단건 SELECT
    bench("query_one by pk", 1_000, || {
        for i in 1..=1_000i64 {
            let _: (i64, String) = h.query_one("SELECT id, payload FROM rows WHERE id = ?1", params![i])?;
        }
        Ok(())
    })?;

    Ok(())
}
