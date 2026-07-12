// [명세 §5.4] entities(...) 비움 = 컴파일 에러
use roomrs::database;

#[database(version = 1)]
struct Db;

fn main() {}
