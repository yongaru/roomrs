// [명세 §12c] #[insert] 메서드는 엔티티 참조 인자 1개 필수
use roomrs::{dao, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self) -> roomrs::Result<i64>;
}

fn main() {}
