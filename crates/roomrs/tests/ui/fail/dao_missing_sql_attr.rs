// [명세 §5.2] SQL 속성 없는 DAO 메서드 = 컴파일 에러
use roomrs::dao;

#[dao]
trait TodoDao {
    fn orphan(&self) -> roomrs::Result<i64>;
}

fn main() {}
