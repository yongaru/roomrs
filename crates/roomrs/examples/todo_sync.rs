//! 동기 todo 예제 (명세 부록 A)

use roomrs::{MigrationPolicy, dao, database, entity};
mod support;

#[entity(table = "todos")]
#[derive(Debug, Clone)]
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

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

/// 실행: cargo run --example todo_sync
fn main() -> roomrs::Result<()> {
    support::init_tracing();
    let db = Db::builder().in_memory().migrate(MigrationPolicy::Auto).build()?;
    let h = db.run_sync();

    let id = h.todo_dao().add(&Todo { id: 0, title: "명세 읽기".into(), done: false })?;
    println!("새 id = {id}");
    for t in h.todo_dao().by_done(false)? {
        println!("- [{}] {}", t.id, t.title);
    }

    let todo: Todo = h.query_one::<Todo, _>("SELECT id, title, done FROM todos WHERE id = ?1", roomrs::params![id])?;
    println!("직접 SQL 한 건: {}", todo.title);

    let missing: Option<Todo> = h.query_optional::<Todo, _>("SELECT id, title, done FROM todos WHERE id = ?1", roomrs::params![-1_i64])?;
    println!("직접 SQL 선택 조회: {}", missing.is_none());

    let todos: Vec<Todo> = h.query_all::<Todo, _>("SELECT id, title, done FROM todos WHERE done = ?1 ORDER BY id", roomrs::params![false])?;
    println!("직접 SQL 여러 건: {}건", todos.len());

    let count: i64 = h.query_scalar::<i64, _>("SELECT COUNT(*) FROM todos", ())?;
    println!("직접 SQL 스칼라: {count}건");

    let updated: Todo = h.query_one::<Todo, _>("UPDATE todos SET title = ?1 WHERE id = ?2 RETURNING id, title, done", roomrs::params!["직접 SQL 수정", id])?;
    println!("직접 SQL RETURNING: {}", updated.title);

    let changed = h.execute("UPDATE todos SET done = ?1 WHERE id = ?2", roomrs::params![true, id])?;
    println!("직접 SQL 쓰기: {changed}건 변경");
    Ok(())
}
