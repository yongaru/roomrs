use roomrs::entity;

#[entity(table = "t", without_rowid)]
struct Bad {
    name: String,
}

fn main() {}
