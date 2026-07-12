// [M-17] 메서드에 SQL 속성 2개 = 마지막 승자 침묵 대신 컴파일 에러
use roomrs::dao;

#[dao]
trait TodoDao {
    #[query("SELECT COUNT(*) FROM todos")]
    #[delete("DELETE FROM todos")]
    fn confused(&self) -> roomrs::Result<i64>;
}

fn main() {}
