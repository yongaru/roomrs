// [명세 §5.2] DAO 메서드는 Result<...> 반환 필수 — 맨 타입 = 컴파일 에러
use roomrs::dao;

#[dao]
trait TodoDao {
    #[query("SELECT COUNT(*) FROM todos")]
    fn count(&self) -> i64;
}

fn main() {}
