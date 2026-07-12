use roomrs::dao;

#[dao]
trait ItemDao {
    #[update("UPDATE items SET value = 1")]
    fn update(&self) -> roomrs::Result<()>;
}

fn main() {}
