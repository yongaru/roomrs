#![allow(unnameable_test_items)]

use roomrs::{Entity, database, entity};

#[entity(table = "text_keys")]
struct TextKey {
    #[pk]
    id: String,
}

/// 비-INTEGER 기본 키는 SQLite의 역사적 NULL 허용을 명시적으로 차단한다.
#[test]
fn text_primary_key_is_not_null() {
    assert!(
        TextKey::DDL[0].contains("\"id\" TEXT PRIMARY KEY NOT NULL"),
        "DDL: {}",
        TextKey::DDL[0]
    );
}

/// 같은 Entity를 다른 path로 나열해도 동일 SQLite TABLE 중복을 거부한다.
#[test]
fn aliased_duplicate_entity_table_is_rejected() {
    #[database(entities(TextKey, self::TextKey), version = 1)]
    struct AliasDb;

    let result = AliasDb::builder().in_memory().build();
    assert!(
        matches!(result, Err(roomrs::Error::Config(message)) if message.contains("테이블 이름 중복"))
    );
}
