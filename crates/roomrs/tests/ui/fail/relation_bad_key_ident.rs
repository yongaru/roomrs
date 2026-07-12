// [M-13] parent_key/entity_key 는 러스트 필드명 — 무효 식별자 = 컴파일 에러
// (format_ident! panic 대신 원인 span 에러)
use roomrs::{Relation, entity};

#[entity(table = "users")]
struct User {
    #[pk(autoincrement)]
    id: i64,
    name: String,
}

#[entity(table = "posts")]
struct Post {
    #[pk(autoincrement)]
    id: i64,
    user_id: i64,
}

#[derive(Relation)]
struct UserWithPosts {
    #[embedded]
    user: User,
    #[relation(entity = Post, parent_key = "user id", entity_key = "user_id")]
    posts: Vec<Post>,
}

fn main() {}
