// [명세 §5.1] #[entity]는 named 필드 구조체 전용 — 튜플 구조체 = 컴파일 에러
use roomrs::entity;

#[entity(table = "tuples")]
struct Tup(i64, String);

fn main() {}
