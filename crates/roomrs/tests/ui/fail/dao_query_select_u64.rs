// [M-12] #[query] SELECT + Result<u64> = 영향 행 수 오분류 → 컴파일 에러
// (런타임 ExecuteReturnedResults 방지 — i64 스칼라 사용 안내)
use roomrs::dao;

#[dao]
trait TodoDao {
    #[query("SELECT COUNT(*) FROM todos")]
    fn count(&self) -> roomrs::Result<u64>;
}

fn main() {}
