use roomrs::{database, entity};

#[entity]
struct Item {
    #[pk]
    id: i64,
}

#[database(
    entities(Item),
    version = 1,
    trigger(
        name = "item_trigger",
        sql = "CREATE TEMP TRIGGER item_trigger AFTER INSERT ON item BEGIN SELECT 1; END"
    )
)]
struct Db;

fn main() {}
