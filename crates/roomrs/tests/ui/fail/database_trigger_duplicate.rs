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
        name = "trg_item",
        sql = "CREATE TRIGGER trg_item AFTER INSERT ON item BEGIN SELECT 1; END"
    ),
    trigger(
        name = "TRG_ITEM",
        sql = "CREATE TRIGGER TRG_ITEM AFTER DELETE ON item BEGIN SELECT 1; END"
    )
)]
struct Db;

fn main() {}
