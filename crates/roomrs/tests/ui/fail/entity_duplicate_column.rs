// [L-13] #[column(name)] 으로 생기는 컬럼명 중복 = 전개 시점 컴파일 에러
use roomrs::entity;

#[entity(table = "dups")]
struct Dup {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    #[column(name = "title")]
    subject: String,
}

fn main() {}
