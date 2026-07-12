use roomrs::entity;

#[entity(table = "bad\"table")]
struct Bad {
    #[pk]
    id: String,
}

fn main() {}
