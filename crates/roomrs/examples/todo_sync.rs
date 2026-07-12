//! 동기 todo 예제 (명세 부록 A)

use roomrs::{MigrationPolicy, dao, database, entity};

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
    let db = Db::builder()
        .in_memory()
        .migrate(MigrationPolicy::Auto)
        .build()?;
    let h = db.run_sync();

    let id = h.todo_dao().add(&Todo {
        id: 0,
        title: "명세 읽기".into(),
        done: false,
    })?;
    println!("새 id = {id}");
    for t in h.todo_dao().by_done(false)? {
        println!("- [{}] {}", t.id, t.title);
    }
    Ok(())
}
