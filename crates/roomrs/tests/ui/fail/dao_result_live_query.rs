use roomrs::dao;

#[dao]
trait ItemDao {
    #[query("SELECT id FROM items")]
    fn watch(&self) -> roomrs::Result<roomrs::LiveQuery<Vec<i64>>>;
}

fn main() {}
