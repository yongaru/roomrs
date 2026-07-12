use roomrs::dao;

#[dao]
trait ItemDao {
    #[delete("DELETE FROM items WHERE id = :id")]
    fn delete(&self, id: i64) -> roomrs::Result<Option<i64>>;
}

fn main() {}
