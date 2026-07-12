use roomrs::dao;

#[dao]
trait ItemDao {
    #[update("UPDATE items SET value = :value WHERE id = :id")]
    fn update(&self, id: i64, value: i64) -> roomrs::Result<i64>;
}

fn main() {}
