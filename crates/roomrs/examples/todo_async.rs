//! 비동기 todo 예제 — 런타임 무관(smol 사용, tokio/async-std 동일)
//! 필요 feature: async (기본 on)

use roomrs::{BuildAsyncExt, MigrationPolicy, dao, database, entity};

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

/// 실행: cargo run --example todo_async
fn main() -> roomrs::Result<()> {
    smol::block_on(async {
        let db = Db::builder()
            .in_memory()
            .migrate(MigrationPolicy::Auto)
            .build_async()
            .await?;
        let h = db.run_async();

        let id = h
            .todo_dao()
            .add(&Todo {
                id: 0,
                title: "비동기".into(),
                done: false,
            })
            .await?;
        println!("새 id = {id}");
        for t in h.todo_dao().by_done(false).await? {
            println!("- [{}] {}", t.id, t.title);
        }
        Ok(())
    })
}
