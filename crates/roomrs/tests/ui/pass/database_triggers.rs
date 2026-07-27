use roomrs::{database, entity};

#[entity(table = "items")]
struct Item {
    #[pk]
    id: i64,
}

#[database(
    entities(Item),
    version = 1,
    trigger(
        name = "trg_inline",
        sql = "CREATE TRIGGER trg_inline AFTER INSERT ON items BEGIN SELECT 1; END"
    )
)]
struct Db;

fn main() {}
