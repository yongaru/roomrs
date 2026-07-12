// [명세 §5.4] version = 0 은 신규 DB 마커 — 사용자 지정 불가
use roomrs::{database, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
}

#[database(entities(Todo), version = 0)]
struct Db;

fn main() {}
