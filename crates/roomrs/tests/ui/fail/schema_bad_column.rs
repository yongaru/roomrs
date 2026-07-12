// [명세 §7.2] 테이블에 없는 컬럼 참조 = 컴파일 에러 (단일 테이블·별칭 없음)
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
    #[query("SELECT id, nonexistent FROM todos")]
    fn broken(&self) -> roomrs::Result<Vec<(i64, String)>>;
}

fn main() {}
