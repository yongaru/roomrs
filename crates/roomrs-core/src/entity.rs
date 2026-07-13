//! 엔티티 메타 trait — `#[entity]` 매크로가 구현을 생성한다 (명세 §5.1, §12c)

use crate::error::{Error, Result};
use crate::row::FromRow;
use rusqlite::ToSql;
use rusqlite::types::{ToSqlOutput, Value};

/// ToSql 값을 소유 `Value`로 변환 — 비동기 경로의 'static 확보용 (명세 §2.4)
pub fn to_owned_value<T: ToSql + ?Sized>(t: &T) -> Result<Value> {
    match t.to_sql()? {
        ToSqlOutput::Borrowed(vr) => Ok(vr.into()),
        ToSqlOutput::Owned(v) => Ok(v),
        other => Err(Error::Internal(format!(
            "지원하지 않는 ToSqlOutput 변형: {other:?} — 비동기 경로에서 사용 불가"
        ))),
    }
}

/// ToSqlOutput 목록을 소유 Value 목록으로 변환
pub fn outputs_to_values(outs: Vec<ToSqlOutput<'_>>) -> Result<Vec<Value>> {
    outs.into_iter()
        .map(|o| match o {
            ToSqlOutput::Borrowed(vr) => Ok(vr.into()),
            ToSqlOutput::Owned(v) => Ok(v),
            other => Err(Error::Internal(format!(
                "지원하지 않는 ToSqlOutput 변형: {other:?} — 비동기 경로에서 사용 불가"
            ))),
        })
        .collect()
}

/// 테이블 매핑 메타. `#[entity]` 생성물.
pub trait Entity: FromRow {
    /// 테이블 이름
    const TABLE: &'static str;
    /// 테이블 + 인덱스 DDL (실행 순서대로)
    const DDL: &'static [&'static str];
    /// SELECT 컬럼 목록 (`col1, col2, …` — ignore 필드 제외)
    const COLUMNS: &'static str;
    /// 컬럼 메타 — 스냅샷 생성·해시 대조용 (명세 §7)
    const COLUMNS_META: &'static [crate::database::ColumnMeta];
}

/// INSERT 지원 메타. `#[entity]` 생성물 (명세 §12c — autoincrement PK 항상 생략).
pub trait Insertable: Entity {
    /// PK 생략 컬럼 목록 (`title, done`)
    const INSERT_COLUMNS: &'static str;
    /// PK 생략 플레이스홀더 (`?1, ?2`)
    const INSERT_PLACEHOLDERS: &'static str;
    /// PK 포함 컬럼 목록 — `#[insert(keep_pk)]` 용
    const INSERT_COLUMNS_KEEP_PK: &'static str;
    /// PK 포함 플레이스홀더
    const INSERT_PLACEHOLDERS_KEEP_PK: &'static str;

    /// PK 생략 바인딩 값 추출 (`#[json]` 필드는 여기서 직렬화)
    fn insert_params(&self) -> crate::Result<Vec<ToSqlOutput<'_>>>;
    /// PK 포함 바인딩 값 추출
    fn insert_params_keep_pk(&self) -> crate::Result<Vec<ToSqlOutput<'_>>>;
}
