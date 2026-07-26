// [결정 40] #[pk(autoincrement)] + 다른 #[pk] = 컴파일 에러
use roomrs::entity;

#[entity(table = "pairs")]
struct Pair {
    #[pk(autoincrement)]
    a: i64,
    #[pk]
    b: i64,
}

fn main() {}
