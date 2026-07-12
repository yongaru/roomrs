// M5 검증 통합 테스트 (명세 §15 M5) —
// 1:N · 1:1 · N:M(junction) · 자동 tx 래핑 · N+1 회피(쿼리 수 고정)

use roomrs::{Relation, dao, database, entity, params};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[entity(table = "users")]
#[derive(Debug, Clone)]
struct User {
    #[pk(autoincrement)]
    id: i64,
    name: String,
}

#[entity(table = "posts")]
#[derive(Debug, Clone)]
struct Post {
    #[pk(autoincrement)]
    id: i64,
    user_id: i64,
    title: String,
}

#[entity(table = "profiles")]
#[derive(Debug, Clone)]
struct Profile {
    #[pk(autoincrement)]
    id: i64,
    user_id: i64,
    bio: String,
}

#[entity(table = "tags")]
#[derive(Debug, Clone)]
struct Tag {
    #[pk(autoincrement)]
    id: i64,
    label: String,
}

#[entity(table = "post_tags")]
#[derive(Debug, Clone)]
struct PostTag {
    #[pk(autoincrement)]
    id: i64,
    post_id: i64,
    tag_id: i64,
}

/// 1:N + 1:1 관계 뷰
#[derive(Relation, Debug)]
struct UserWithPosts {
    #[embedded]
    user: User,
    #[relation(entity = Post, parent_key = "id", entity_key = "user_id")]
    posts: Vec<Post>,
    #[relation(entity = Profile, parent_key = "id", entity_key = "user_id")]
    profile: Option<Profile>,
}

/// N:M 관계 뷰
#[derive(Relation, Debug)]
#[allow(dead_code)]
struct PostWithTags {
    #[embedded]
    post: Post,
    #[relation(
        entity = Tag,
        parent_key = "id",
        entity_key = "id",
        junction = "post_tags",
        junction_parent_key = "post_id",
        junction_entity_key = "tag_id"
    )]
    tags: Vec<Tag>,
}

#[dao]
trait UserDao {
    #[query(with_relations, "SELECT * FROM users ORDER BY id")]
    fn users_with_posts(&self) -> roomrs::Result<Vec<UserWithPosts>>;

    #[query(with_relations, "SELECT * FROM users WHERE id = :id")]
    fn one_with_posts(&self, id: i64) -> roomrs::Result<Option<UserWithPosts>>;

    #[query(with_relations, "SELECT * FROM posts ORDER BY id")]
    fn posts_with_tags(&self) -> roomrs::Result<Vec<PostWithTags>>;
}

#[database(
    entities(User, Post, Profile, Tag, PostTag),
    daos(UserDao),
    version = 1
)]
struct Db;

/// 시드: 사용자 2, 글 3(u1:2 + u2:1), 프로필 1(u1), 태그 2, 정션(글1=태그2개, 글2=태그1개)
fn seed(logger_counter: Option<Arc<AtomicU64>>) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let mut b = Db::builder().sqlite(dir.path().join("r.db"));
    if let Some(c) = logger_counter {
        b = b.query_logger(move |_sql, _d| {
            c.fetch_add(1, Ordering::SeqCst);
        });
    }
    let db = b.build().unwrap();
    let h = db.run_sync();
    h.execute("INSERT INTO users (name) VALUES ('u1'), ('u2')", params![])
        .unwrap();
    h.execute(
        "INSERT INTO posts (user_id, title) VALUES (1,'p1'), (1,'p2'), (2,'p3')",
        params![],
    )
    .unwrap();
    h.execute(
        "INSERT INTO profiles (user_id, bio) VALUES (1, 'bio1')",
        params![],
    )
    .unwrap();
    h.execute("INSERT INTO tags (label) VALUES ('t1'), ('t2')", params![])
        .unwrap();
    h.execute(
        "INSERT INTO post_tags (post_id, tag_id) VALUES (1,1), (1,2), (2,1)",
        params![],
    )
    .unwrap();
    (dir, db)
}

/// 1:N + 1:1 조립
#[test]
fn one_to_many_and_one_to_one() {
    let (_d, db) = seed(None);
    let views = db.run_sync().user_dao().users_with_posts().unwrap();
    assert_eq!(views.len(), 2);

    let u1 = &views[0];
    assert_eq!(u1.user.name, "u1");
    assert_eq!(u1.posts.len(), 2, "1:N");
    assert_eq!(u1.profile.as_ref().unwrap().bio, "bio1", "1:1 존재");

    let u2 = &views[1];
    assert_eq!(u2.posts.len(), 1);
    assert!(u2.profile.is_none(), "1:1 부재 = None");
}

/// Optional 형태 + 파라미터
#[test]
fn optional_shape() {
    let (_d, db) = seed(None);
    let v = db
        .run_sync()
        .user_dao()
        .one_with_posts(1)
        .unwrap()
        .expect("존재");
    assert_eq!(v.posts.len(), 2);
    assert!(
        db.run_sync()
            .user_dao()
            .one_with_posts(999)
            .unwrap()
            .is_none()
    );
}

/// N:M — junction 경유 조립
#[test]
fn many_to_many() {
    let (_d, db) = seed(None);
    let views = db.run_sync().user_dao().posts_with_tags().unwrap();
    assert_eq!(views.len(), 3);
    let mut l1: Vec<&str> = views[0].tags.iter().map(|t| t.label.as_str()).collect();
    l1.sort();
    assert_eq!(l1, vec!["t1", "t2"], "글1 = 태그 2개");
    assert_eq!(views[1].tags.len(), 1, "글2 = 태그 1개");
    assert!(views[2].tags.is_empty(), "글3 = 태그 없음");
}

/// N+1 회피 — 부모 수와 무관하게 쿼리 수 고정 (query_logger로 계수)
#[test]
fn no_n_plus_one() {
    let counter = Arc::new(AtomicU64::new(0));
    let (_d, db) = seed(Some(counter.clone()));

    counter.store(0, Ordering::SeqCst);
    let views = db.run_sync().user_dao().users_with_posts().unwrap();
    assert_eq!(views.len(), 2);
    let queries = counter.load(Ordering::SeqCst);
    // 부모 1 + posts IN 1 + profiles IN 1 = 3 (부모 수 2와 무관)
    assert_eq!(queries, 3, "쿼리 수 고정 = N+1 회피 (실측 {queries})");
}

/// 999개를 넘는 1:N/1:1 부모 키는 여러 IN 쿼리로 나눈다.
#[test]
fn relation_keys_are_chunked() {
    let counter = Arc::new(AtomicU64::new(0));
    let (_d, db) = seed(Some(counter.clone()));
    let h = db.run_sync();
    h.execute(
        "WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<1001) INSERT INTO users(id,name) SELECT x, 'bulk' FROM n",
        params![],
    )
    .unwrap();
    counter.store(0, Ordering::SeqCst);
    let views = h.user_dao().users_with_posts().unwrap();
    assert_eq!(views.len(), 1001);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        5,
        "부모 1 + 관계별 청크 2개"
    );
}

/// 999개를 넘는 N:M 부모 키와 자식 키를 각각 청킹한다.
#[test]
fn junction_keys_are_chunked() {
    let counter = Arc::new(AtomicU64::new(0));
    let (_d, db) = seed(Some(counter.clone()));
    let h = db.run_sync();
    h.execute(
        "WITH RECURSIVE n(x) AS (VALUES(4) UNION ALL SELECT x+1 FROM n WHERE x<1001) INSERT INTO posts(id,user_id,title) SELECT x, 1, 'bulk' FROM n",
        params![],
    ).unwrap();
    h.execute(
        "WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<1001) INSERT INTO tags(id,label) SELECT x, 'bulk' FROM n",
        params![],
    ).unwrap();
    h.execute(
        "WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<1001) INSERT INTO post_tags(post_id,tag_id) SELECT x, x FROM n",
        params![],
    ).unwrap();
    counter.store(0, Ordering::SeqCst);
    let views = h.user_dao().posts_with_tags().unwrap();
    assert_eq!(views.len(), 1001);
    assert_eq!(
        counter.load(Ordering::SeqCst),
        5,
        "부모 1 + 정션 2 + 자식 2"
    );
}

/// 비동기 with_relations — 워커에서 자동 tx 래핑
#[cfg(all(feature = "async", not(feature = "tokio")))]
#[test]
fn async_relations() {
    let (_d, db) = seed(None);
    smol::block_on(async {
        let views = db.run_async().user_dao().users_with_posts().await.unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].posts.len(), 2);
    });
}
