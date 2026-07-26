use roomrs::entity;

#[entity(table = "t")]
struct Bad {
    #[pk]
    id: i64,
    #[column(collate = "")]
    name: String,
}

fn main() {}
