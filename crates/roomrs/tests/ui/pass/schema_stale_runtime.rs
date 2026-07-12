// [명세 §7.4b] 엔티티가 스냅샷과 다르면(스테일) build()가 SnapshotStale 반환.
// 스냅샷: todos(id, title, done) — 이 엔티티는 extra 컬럼이 추가된 상태.
use roomrs::{dao, database, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
    extra: Option<String>, // 스냅샷 재생성을 잊은 신규 컬럼
}

#[dao]
trait TodoDao {
    // 공통 컬럼만 참조 — 정적 검증은 통과해야 스테일 검증까지 도달한다
    #[query("SELECT id FROM todos")]
    fn ids(&self) -> roomrs::Result<Vec<i64>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

// 컴파일은 성공, 런타임 build()에서 스테일 감지
fn main() {
    match Db::builder().in_memory().build() {
        Err(roomrs::Error::SnapshotStale(_)) => {}
        Err(other) => panic!("SnapshotStale을 기대했으나: {other}"),
        Ok(_) => panic!("스테일 스냅샷이 통과되면 안 된다"),
    }
}
