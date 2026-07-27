use roomrs::{database, entity};

#[entity]
struct Item {
    #[pk]
    id: i64,
}

#[database(
    entities(Item),
    version = 1,
    trigger(name = "item_trigger")
)]
struct Db;

fn main() {}
