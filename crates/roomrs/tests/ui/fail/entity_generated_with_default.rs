use roomrs::entity;

#[entity(table = "t")]
struct Bad {
    #[pk]
    id: i64,
    #[column(generated = "1", default = "0")]
    v: i64,
}

fn main() {}
