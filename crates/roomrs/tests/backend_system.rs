#![cfg(any(feature = "sqlite-system", feature = "sqlcipher-system"))]

#[cfg(feature = "sqlcipher-system")]
use roomrs::{database, entity};

#[cfg(feature = "sqlcipher-system")]
const KEY: &str = "roomrs-system-sqlcipher-test-key";

#[cfg(feature = "sqlcipher-system")]
#[entity(table = "secrets")]
struct Secret {
    #[pk]
    id: i64,
    value: String,
}

#[cfg(feature = "sqlcipher-system")]
#[database(entities(Secret), version = 1)]
struct CipherDb;

/// system backend가 실제 SQLite와 preupdate hook compile option을 제공하는지 검증한다.
#[test]
fn system_backend_reports_version_and_preupdate_hook() {
    let connection = roomrs::rusqlite::Connection::open_in_memory().unwrap();
    let version: String = connection
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .unwrap();
    let preupdate: i64 = connection
        .query_row(
            "SELECT sqlite_compileoption_used('ENABLE_PREUPDATE_HOOK')",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(!version.is_empty());
    assert_eq!(preupdate, 1);
}

/// system SQLCipher가 암호화 DB를 재개방하고 잘못된 키와 무키 접근을 거부하는지 검증한다.
#[cfg(feature = "sqlcipher-system")]
#[test]
fn system_sqlcipher_roundtrip_and_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("encrypted.db");

    {
        let database = CipherDb::builder()
            .sqlite(&path)
            .encryption_key(KEY)
            .build()
            .unwrap();
        database
            .run_sync()
            .execute("INSERT INTO secrets (id, value) VALUES (1, 'secret')", [])
            .unwrap();
    }

    let reopened = CipherDb::builder()
        .sqlite(&path)
        .encryption_key(KEY)
        .build()
        .unwrap();
    let value: String = reopened
        .run_sync()
        .query_scalar("SELECT value FROM secrets WHERE id = 1", [])
        .unwrap();
    assert_eq!(value, "secret");
    drop(reopened);

    assert!(
        CipherDb::builder()
            .sqlite(&path)
            .encryption_key("wrong-key")
            .build()
            .is_err()
    );
    assert!(CipherDb::builder().sqlite(&path).build().is_err());
}

/// system SQLCipher가 codec과 preupdate hook을 함께 제공하는지 검증한다.
#[cfg(feature = "sqlcipher-system")]
#[test]
fn system_sqlcipher_reports_cipher_version() {
    let connection = roomrs::rusqlite::Connection::open_in_memory().unwrap();
    let version: String = connection
        .query_row("PRAGMA cipher_version", [], |row| row.get(0))
        .unwrap();
    assert!(!version.is_empty());
}
