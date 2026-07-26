use roomrs::{Entity, entity};

#[entity(
    table = "t_pass_adv",
    strict,
    without_rowid,
    index(name = "idx_n", columns(name collate nocase asc)),
)]
struct PassAdv {
    #[pk]
    id: String,
    #[column(collate = "RTRIM")]
    name: String,
    a: i64,
    b: i64,
    #[column(generated = "a + b")]
    sum: i64,
}

fn main() {
    let _ = PassAdv::TABLE;
    let _ = PassAdv::STRICT;
    let _ = PassAdv::WITHOUT_ROWID;
    let _ = PassAdv::DDL;
}
