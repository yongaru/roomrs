use roomrs::dao;

#[dao]
trait ItemDao {
    #[update(
        unchecked,
        "UPDATE items SET value = :value WHERE id = :id RETURNING value"
    )]
    fn update(&self, id: i64, value: i64) -> roomrs::Result<i64>;

    #[delete(unchecked, "DELETE FROM items WHERE id = :id RETURNING id")]
    fn delete(&self, id: i64) -> roomrs::Result<Option<i64>>;
}

fn main() {}
