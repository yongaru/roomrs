use roomrs::{dao, entity};

#[entity(table = "items")]
struct Item {
    #[pk(autoincrement)]
    id: i64,
}

#[dao]
trait ItemDao {
    #[insert(on_conflict = "ignore")]
    fn add(&self, item: &Item) -> roomrs::Result<i64>;
}

fn main() {}
