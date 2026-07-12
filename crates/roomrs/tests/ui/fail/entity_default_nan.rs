// [M-16] #[column(default = "nan")] — 비유한 숫자는 SQLite DEFAULT로 표현 불가
use roomrs::entity;

#[entity(table = "measures")]
struct Measure {
    #[pk(autoincrement)]
    id: i64,
    #[column(default = "nan")]
    value: f64,
}

fn main() {}
