//! 관계 매핑 사용 케이스 (명세 결정 로그 7)
//! - 1:N(Vec) · 1:1(Option) · N:M(junction)
//! - #[query(with_relations, …)] = 자동 트랜잭션 + IN 일괄 조회(N+1 회피)
//!
//! 실행: cargo run --example relations

use roomrs::{Relation, dao, database, entity, params};

#[entity(table = "authors")]
#[derive(Debug, Clone)]
struct Author {
    #[pk(autoincrement)]
    id: i64,
    name: String,
}

#[entity(table = "books")]
#[derive(Debug, Clone)]
struct Book {
    #[pk(autoincrement)]
    id: i64,
    author_id: i64,
    title: String,
}

#[entity(table = "profiles")]
#[derive(Debug, Clone)]
struct Profile {
    #[pk(autoincrement)]
    id: i64,
    author_id: i64,
    bio: String,
}

#[entity(table = "genres")]
#[derive(Debug, Clone)]
struct Genre {
    #[pk(autoincrement)]
    id: i64,
    label: String,
}

#[entity(table = "book_genres")]
#[derive(Debug, Clone)]
struct BookGenre {
    #[pk(autoincrement)]
    id: i64,
    book_id: i64,
    genre_id: i64,
}

/// 작가 + 저서(1:N) + 프로필(1:1)
#[derive(Relation, Debug)]
struct AuthorView {
    #[embedded]
    author: Author,
    #[relation(entity = Book, parent_key = "id", entity_key = "author_id")]
    books: Vec<Book>,
    #[relation(entity = Profile, parent_key = "id", entity_key = "author_id")]
    profile: Option<Profile>,
}

/// 책 + 장르(N:M — 정션 경유)
#[derive(Relation, Debug)]
struct BookView {
    #[embedded]
    book: Book,
    #[relation(
        entity = Genre,
        parent_key = "id",
        entity_key = "id",
        junction = "book_genres",
        junction_parent_key = "book_id",
        junction_entity_key = "genre_id"
    )]
    genres: Vec<Genre>,
}

#[dao]
trait LibraryDao {
    #[query(with_relations, "SELECT * FROM authors ORDER BY id")]
    fn authors(&self) -> roomrs::Result<Vec<AuthorView>>;

    #[query(with_relations, "SELECT * FROM books ORDER BY id")]
    fn books(&self) -> roomrs::Result<Vec<BookView>>;
}

#[database(
    entities(Author, Book, Profile, Genre, BookGenre),
    daos(LibraryDao),
    version = 1
)]
struct Db;

fn main() -> roomrs::Result<()> {
    let db = Db::builder().in_memory().build()?;
    let h = db.run_sync();
    h.execute(
        "INSERT INTO authors (name) VALUES ('김작가'), ('이작가')",
        params![],
    )?;
    h.execute(
        "INSERT INTO books (author_id, title) VALUES (1,'첫 책'), (1,'둘째 책'), (2,'남의 책')",
        params![],
    )?;
    h.execute(
        "INSERT INTO profiles (author_id, bio) VALUES (1, '수필가')",
        params![],
    )?;
    h.execute(
        "INSERT INTO genres (label) VALUES ('에세이'), ('소설')",
        params![],
    )?;
    h.execute(
        "INSERT INTO book_genres (book_id, genre_id) VALUES (1,1), (1,2), (2,2)",
        params![],
    )?;

    // 1:N + 1:1 — 부모 수와 무관하게 쿼리 3개(작가 + 책 IN + 프로필 IN)
    for v in h.library_dao().authors()? {
        println!(
            "{} — 저서 {}권, 프로필: {}",
            v.author.name,
            v.books.len(),
            v.profile.map(|p| p.bio).unwrap_or_else(|| "없음".into()),
        );
    }

    // N:M
    for v in h.library_dao().books()? {
        let genres: Vec<&str> = v.genres.iter().map(|g| g.label.as_str()).collect();
        println!("《{}》 장르: {genres:?}", v.book.title);
    }
    Ok(())
}
