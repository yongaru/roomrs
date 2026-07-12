// [명세 §5.1] #[pk] 필드 2개 = 컴파일 에러 (복합 PK 미지원)
use roomrs::entity;

#[entity(table = "pairs")]
struct Pair {
    #[pk]
    a: i64,
    #[pk]
    b: i64,
}

fn main() {}
