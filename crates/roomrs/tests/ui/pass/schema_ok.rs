// [명세 §7.2/§7.3] 스냅샷 일치 SQL = 통과, unchecked = 검증 우회, 실제 빌드·CRUD 동작
use roomrs::{dao, database, entity, params};

#[entity(table = "todos")]
#[derive(Debug)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, t: &Todo) -> roomrs::Result<i64>;

    // 스냅샷 대조 통과 — 테이블·컬럼 전부 존재
    #[query("SELECT id, title FROM todos WHERE done = :done")]
    fn titles(&self, done: bool) -> roomrs::Result<Vec<(i64, String)>>;

    // unchecked 해치 — 스냅샷에 없는 컬럼이지만 검증 스킵 (명세 §7.3).
    // 런타임 prepare에서 잡히는 게 정상이므로 호출은 하지 않는다.
    #[query(unchecked, "SELECT ghost_column FROM todos")]
    fn ghost(&self) -> roomrs::Result<Vec<String>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

// 스냅샷과 엔티티가 일치 — build()의 해시 검증 통과 + CRUD 왕복
fn main() {
    let db = Db::builder().in_memory().build().expect("스냅샷 일치 = 빌드 성공");
    let h = db.run_sync();
    let id = h
        .todo_dao()
        .add(&Todo { id: 0, title: "ok".into(), done: false })
        .expect("insert");
    assert!(id > 0);
    let rows = h.todo_dao().titles(false).expect("select");
    assert_eq!(rows.len(), 1);
    let _ = params![]; // params 재수출 사용 확인
}
