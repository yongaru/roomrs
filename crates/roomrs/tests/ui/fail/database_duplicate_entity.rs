use roomrs::{database, entity};

#[entity]
struct Item {
    #[pk]
    id: i64,
}

#[database(entities(Item, Item), version = 1)]
struct Db;

fn main() {}
