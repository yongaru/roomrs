use roomrs::Relation;

struct Parent {
    id: i64,
}
struct Child {
    id: i64,
}

#[derive(Relation)]
struct Bad {
    #[embedded]
    parent: Parent,
    #[relation(entity = Child, parent_key = "id", entity_key = "id", junction = "bad\"table", junction_parent_key = "parent_id", junction_entity_key = "child_id")]
    children: Vec<Child>,
}

fn main() {}
