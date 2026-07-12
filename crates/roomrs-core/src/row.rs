//! 행 → 값 매핑: `FromRow` (명세 §5.7 — 구조체/튜플/스칼라)

use rusqlite::Row;
use rusqlite::types::FromSql;

/// 쿼리 결과 한 행을 값으로 변환하는 trait.
/// 엔티티는 `#[entity]` 매크로가 구현을 생성하고,
/// 프리미티브·튜플은 아래 매크로로 일괄 구현한다.
pub trait FromRow: Sized {
    /// 행에서 값 구성 — 실패 시 rusqlite 에러(타입 불일치 등)
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
}

/// 단일 컬럼 스칼라 타입들에 FromRow 구현
macro_rules! impl_from_row_scalar {
    ($($t:ty),+ $(,)?) => {$(
        impl FromRow for $t {
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                row.get(0)
            }
        }
        impl FromRow for Option<$t> {
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                row.get(0)
            }
        }
    )+};
}

impl_from_row_scalar!(
    bool,
    i8,
    i16,
    i32,
    i64,
    u8,
    u16,
    u32,
    f32,
    f64,
    String,
    Vec<u8>
);

#[cfg(feature = "time")]
impl_from_row_scalar!(
    time::OffsetDateTime,
    time::PrimitiveDateTime,
    time::Date,
    time::Time
);

#[cfg(feature = "uuid")]
impl_from_row_scalar!(uuid::Uuid);

/// 튜플(컬럼별 FromSql)에 FromRow 구현 — 최대 12원소
macro_rules! impl_from_row_tuple {
    ($($idx:tt : $name:ident),+) => {
        impl<$($name: FromSql),+> FromRow for ($($name,)+) {
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                Ok(($(row.get::<_, $name>($idx)?,)+))
            }
        }
    };
}

impl_from_row_tuple!(0: A);
impl_from_row_tuple!(0: A, 1: B);
impl_from_row_tuple!(0: A, 1: B, 2: C);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K);
impl_from_row_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L);
