use roomrs::dao;

#[dao]
trait ItemDao {
    #[query("SELECT :id")]
    fn find(id: i64) -> roomrs::Result<i64>;
}

fn main() {}
