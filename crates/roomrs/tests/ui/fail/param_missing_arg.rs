// [명세 §5.2] SQL 파라미터 :id 에 대응하는 인자가 없으면 컴파일 에러
use roomrs::{dao, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
}

#[dao]
trait TodoDao {
    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self) -> roomrs::Result<Option<Todo>>;
}

fn main() {}
