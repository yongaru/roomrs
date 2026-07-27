use roomrs::entity;

#[entity(trigger = "migrations/triggers/t_payment_audit.sql")]
struct Item {
    #[pk]
    id: i64,
}

fn main() {}
