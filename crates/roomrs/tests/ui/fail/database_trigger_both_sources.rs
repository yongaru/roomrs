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
        sql = "CREATE TRIGGER trg_item AFTER INSERT ON item BEGIN SELECT 1; END",
        file = "migrations/triggers/t_payment_audit.sql"
    )
)]
struct Db;

fn main() {}
