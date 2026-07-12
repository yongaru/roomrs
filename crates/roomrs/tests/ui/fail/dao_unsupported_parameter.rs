use roomrs::dao;

#[dao]
trait Dao {
    #[query("SELECT @id, $name, ?1")]
    fn find(&self, id: i64, name: String) -> roomrs::Result<i64>;
}

fn main() {}
