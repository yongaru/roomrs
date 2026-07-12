// [H-8] #[transaction] 본문의 매크로 토큰 안 self = 재작성 불가 → 컴파일 에러.
// 침묵 통과 시 풀-바운드 DAO로 컴파일되어 writer 자기 데드락/원자성 파괴.
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
    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Option<Todo>>;

    #[transaction]
    fn exists_in_tx(&self, id: i64) -> roomrs::Result<bool> {
        Ok(matches!(self.find(id)?, Some(_)))
    }
}

fn main() {}
