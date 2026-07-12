//! 동적 쿼리빌더 사용 케이스 (명세 §5.3)
//! - 런타임 조건 조합(검색 폼 시나리오) · LIKE · IN · 정렬 · 페이지
//! - 스키마 인지: 미지 컬럼 = SQLite 도달 전 에러
//! - 핸들 대칭: 같은 쿼리를 동기/비동기 양쪽 실행
//!
//! 실행: cargo run --example query_builder

use roomrs::{Order, Query, col, dao, database, entity};

#[entity(table = "products")]
#[derive(Debug, Clone, PartialEq)]
struct Product {
    #[pk(autoincrement)]
    id: i64,
    name: String,
    price: i64,
    category: String,
}

#[dao]
trait ProductDao {
    #[insert]
    fn add(&self, p: &Product) -> roomrs::Result<i64>;
}

#[database(entities(Product), daos(ProductDao), version = 1)]
struct Db;

/// 검색 폼 입력 시뮬레이션 — 값이 있는 조건만 동적으로 조립
struct SearchForm {
    keyword: Option<String>,
    max_price: Option<i64>,
    categories: Vec<String>,
}

fn main() -> roomrs::Result<()> {
    let db = Db::builder().in_memory().build()?;
    let h = db.run_sync();
    for (n, p, c) in [
        ("기계식 키보드", 120_000, "입력장치"),
        ("무선 마우스", 45_000, "입력장치"),
        ("모니터 27인치", 300_000, "디스플레이"),
        ("키보드 루프", 9_000, "액세서리"),
    ] {
        h.product_dao().add(&Product {
            id: 0,
            name: n.into(),
            price: p,
            category: c.into(),
        })?;
    }

    // 1) 동적 조건 조립 — Room의 SupportSQLiteQuery 대응
    let form = SearchForm {
        keyword: Some("키보드".into()),
        max_price: Some(150_000),
        categories: vec!["입력장치".into(), "액세서리".into()],
    };
    let mut q = Query::select::<Product>();
    if let Some(kw) = &form.keyword {
        q = q.and_where(col("name").like(format!("%{kw}%")));
    }
    if let Some(max) = form.max_price {
        q = q.and_where(col("price").le(max));
    }
    if !form.categories.is_empty() {
        q = q.and_where(col("category").in_list(form.categories.clone()));
    }
    let q = q.order_by("price", Order::Asc).limit(10);

    for p in q.clone().fetch_all(h)? {
        println!("{} — {}원 [{}]", p.name, p.price, p.category);
    }

    // 2) 핸들 대칭 — 같은 쿼리를 비동기로 (feature async 기본 on)
    #[cfg(feature = "async")]
    {
        let async_rows: Vec<Product> = smol::block_on(async { q.fetch_all(db.run_async()).await })?;
        println!("비동기 동일 결과: {}건", async_rows.len());
    }

    // 3) 스키마 인지 — 오타 컬럼은 실행 전에 잡힌다
    let bad: roomrs::Result<Vec<Product>> = Query::select::<Product>()
        .and_where(col("prise").gt(0i64))
        .fetch_all(db.run_sync());
    println!("오타 컬럼: {}", bad.unwrap_err());
    Ok(())
}
