// [M-15] 단일 autoincrement PK 엔티티 = INSERT 컬럼 0개 → DEFAULT VALUES 왕복.
// 이전엔 `INSERT INTO "solos" () VALUES ()` (SQL 문법 오류) 가 생성됐다.
use roomrs::{Insertable, dao, database, entity};

#[entity(table = "solos")]
struct Solo {
    #[pk(autoincrement)]
    id: i64,
}

#[dao]
trait SoloDao {
    #[insert]
    fn add(&self, s: &Solo) -> roomrs::Result<i64>;
}

#[database(entities(Solo), daos(SoloDao), version = 1)]
struct SoloDb;

fn main() {
    // 컬럼 0개 메타 확인 (autoincrement PK 는 INSERT 에서 항상 생략)
    assert!(<Solo as Insertable>::INSERT_COLUMNS.is_empty(), "컬럼 0개");

    let db = SoloDb::builder().in_memory().build().expect("빌드");
    let h = db.run_sync();
    let dao = h.solo_dao();
    let id1 = dao.add(&Solo { id: 0 }).expect("DEFAULT VALUES insert 1");
    let id2 = dao.add(&Solo { id: 0 }).expect("DEFAULT VALUES insert 2");
    assert_eq!((id1, id2), (1, 2), "rowid 증가");

    let n: i64 = h
        .query_scalar("SELECT COUNT(*) FROM solos", [])
        .expect("행 수");
    assert_eq!(n, 2, "행 2개");
}
