// [명세 §5.2] SQL에서 사용되지 않는 메서드 인자는 컴파일 에러
use roomrs::{dao, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
}

#[dao]
trait TodoDao {
    #[query("SELECT * FROM todos")]
    fn all(&self, done: bool) -> roomrs::Result<Vec<Todo>>;
}

fn main() {}
