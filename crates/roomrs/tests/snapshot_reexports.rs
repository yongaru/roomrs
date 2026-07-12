//! 스냅샷 모델은 내부 크레이트 의존 없이 파사드에서 구성 가능해야 한다.

use roomrs::{ColumnSnapshot, SchemaSnapshot, TableSnapshot};

/// 공개 스냅샷 타입 세 가지로 완전한 모델을 구성한다.
#[test]
fn snapshot_model_is_available_from_facade() {
    let snapshot = SchemaSnapshot {
        version: 1,
        tables: vec![TableSnapshot {
            name: "items".into(),
            columns: vec![ColumnSnapshot {
                name: "id".into(),
                sql_type: "INTEGER".into(),
                not_null: true,
                pk: true,
                renamed_from: None,
            }],
            ddl: vec!["CREATE TABLE items(id INTEGER PRIMARY KEY)".into()],
        }],
    };
    assert_eq!(snapshot.tables[0].columns[0].name, "id");
}
