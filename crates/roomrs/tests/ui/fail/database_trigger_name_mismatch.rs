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
        name = "trg_declared",
        sql = "CREATE TRIGGER trg_actual AFTER INSERT ON item BEGIN SELECT 1; END"
    )
)]
struct Db;

fn main() {}
