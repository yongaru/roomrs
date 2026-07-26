use roomrs::entity;

#[entity(table = "t")]
struct Bad {
    #[pk]
    #[column(generated = "1")]
    id: i64,
}

fn main() {}
