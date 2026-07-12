// [명세 §7.2] 스냅샷에 없는 테이블 참조 = 컴파일 에러
use roomrs::{dao, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait BadDao {
    #[query("SELECT * FROM missing_table")]
    fn broken(&self) -> roomrs::Result<Vec<Todo>>;
}

fn main() {}
