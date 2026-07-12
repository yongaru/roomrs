use roomrs::Relation;

struct Parent {
    id: i64,
}
struct Child {
    parent_id: i64,
}

#[derive(Relation)]
struct Bad {
    #[embedded]
    #[relation(entity = Child, parent_key = "id", entity_key = "parent_id")]
    parent: Option<Parent>,
}

fn main() {}
