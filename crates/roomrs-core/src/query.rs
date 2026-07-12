//! 동적 쿼리빌더 (명세 §5.3, 결정 로그 3) — 스키마 인지 + 핸들 대칭 실행[C-6]
//!
//! 명세의 `col(User::id)` 표기는 Rust에서 필드 토큰이 불가하므로
//! `col("id")` 문자열 + 실행 시 `COLUMNS_META` 대조로 대체한다.

use crate::entity::Entity;
use crate::error::{Error, Result};
use crate::handle::SyncHandle;
use crate::row::FromRow;
use rusqlite::types::Value;

/// 컬럼 참조 시작점
pub fn col(name: impl Into<String>) -> Col {
    Col { name: name.into() }
}

/// 컬럼 참조 — 비교 콤비네이터의 시작점
#[derive(Debug, Clone)]
pub struct Col {
    name: String,
}

/// 빌더 값 변환 — `&str` 포함 흔한 타입 열거 (코히어런스 안전)
pub trait IntoDbValue {
    fn into_db_value(self) -> Value;
}

macro_rules! impl_into_db_value {
    ($($t:ty => $v:expr),+ $(,)?) => {$(
        impl IntoDbValue for $t {
            fn into_db_value(self) -> Value {
                let f: fn($t) -> Value = $v;
                f(self)
            }
        }
    )+};
}

impl_into_db_value! {
    i8 => |v| Value::Integer(v as i64),
    i16 => |v| Value::Integer(v as i64),
    i32 => |v| Value::Integer(v as i64),
    i64 => Value::Integer,
    u8 => |v| Value::Integer(v as i64),
    u16 => |v| Value::Integer(v as i64),
    u32 => |v| Value::Integer(v as i64),
    f32 => |v| Value::Real(v as f64),
    f64 => Value::Real,
    bool => |v| Value::Integer(v as i64),
    String => Value::Text,
    &str => |v: &str| Value::Text(v.to_string()),
    Vec<u8> => Value::Blob,
    Value => |v| v,
}

/// WHERE 식 트리
#[derive(Debug, Clone)]
pub enum Expr {
    /// `col OP ?`
    Cmp {
        col: String,
        op: &'static str,
        value: Value,
    },
    /// `col LIKE pattern ESCAPE char` with both values bound as parameters.
    LikeEscaped {
        col: String,
        pattern: String,
        escape: char,
    },
    /// `col IN (?, …)`
    InList {
        col: String,
        values: Vec<Value>,
    },
    /// `col IS NULL` / `IS NOT NULL`
    Null {
        col: String,
        not: bool,
    },
    /// 논리 결합
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Col {
    fn cmp(self, op: &'static str, v: impl IntoDbValue) -> Expr {
        Expr::Cmp {
            col: self.name,
            op,
            value: v.into_db_value(),
        }
    }
    pub fn eq(self, v: impl IntoDbValue) -> Expr {
        let value = v.into_db_value();
        if value == Value::Null {
            self.is_null()
        } else {
            Expr::Cmp {
                col: self.name,
                op: "=",
                value,
            }
        }
    }
    pub fn ne(self, v: impl IntoDbValue) -> Expr {
        let value = v.into_db_value();
        if value == Value::Null {
            self.is_not_null()
        } else {
            Expr::Cmp {
                col: self.name,
                op: "<>",
                value,
            }
        }
    }
    pub fn lt(self, v: impl IntoDbValue) -> Expr {
        self.cmp("<", v)
    }
    pub fn le(self, v: impl IntoDbValue) -> Expr {
        self.cmp("<=", v)
    }
    pub fn gt(self, v: impl IntoDbValue) -> Expr {
        self.cmp(">", v)
    }
    pub fn ge(self, v: impl IntoDbValue) -> Expr {
        self.cmp(">=", v)
    }
    /// LIKE 검색 (명세 §5.8 — 패턴은 호출자가 구성)
    pub fn like(self, pattern: impl Into<String>) -> Expr {
        self.cmp("LIKE", Value::Text(pattern.into()))
    }
    /// Builds a `LIKE ... ESCAPE ...` expression.
    ///
    /// Both the pattern and the single Unicode escape character are bound as
    /// parameters. Existing [`like`](Self::like) behavior is unchanged.
    pub fn like_escaped(self, pattern: impl Into<String>, escape: char) -> Expr {
        Expr::LikeEscaped {
            col: self.name,
            pattern: pattern.into(),
            escape,
        }
    }
    pub fn in_list<V: IntoDbValue>(self, vs: impl IntoIterator<Item = V>) -> Expr {
        Expr::InList {
            col: self.name,
            values: vs.into_iter().map(IntoDbValue::into_db_value).collect(),
        }
    }
    pub fn is_null(self) -> Expr {
        Expr::Null {
            col: self.name,
            not: false,
        }
    }
    pub fn is_not_null(self) -> Expr {
        Expr::Null {
            col: self.name,
            not: true,
        }
    }
}

impl Expr {
    /// 논리 AND
    pub fn and(self, other: Expr) -> Expr {
        Expr::And(Box::new(self), Box::new(other))
    }
    /// 논리 OR
    pub fn or(self, other: Expr) -> Expr {
        Expr::Or(Box::new(self), Box::new(other))
    }

    /// 참조 컬럼 수집 — 스키마 검증용
    fn collect_cols<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Expr::Cmp { col, .. }
            | Expr::LikeEscaped { col, .. }
            | Expr::InList { col, .. }
            | Expr::Null { col, .. } => {
                out.push(col);
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                a.collect_cols(out);
                b.collect_cols(out);
            }
        }
    }

    /// SQL 렌더 — 파라미터는 순서대로 누적
    fn render(&self, sql: &mut String, params: &mut Vec<Value>) {
        match self {
            Expr::Cmp { col, op, value } => {
                params.push(value.clone());
                sql.push_str(&format!("\"{col}\" {op} ?{}", params.len()));
            }
            Expr::LikeEscaped {
                col,
                pattern,
                escape,
            } => {
                params.push(Value::Text(pattern.clone()));
                let pattern_index = params.len();
                params.push(Value::Text(escape.to_string()));
                let escape_index = params.len();
                sql.push_str(&format!(
                    "\"{col}\" LIKE ?{pattern_index} ESCAPE ?{escape_index}"
                ));
            }
            Expr::InList { col, values } => {
                if values.is_empty() {
                    // 빈 IN = 항상 거짓 (SQLite 유효 표현)
                    sql.push_str("0 = 1");
                    return;
                }
                let start = params.len() + 1;
                params.extend(values.iter().cloned());
                let ph = (start..start + values.len())
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                sql.push_str(&format!("\"{col}\" IN ({ph})"));
            }
            Expr::Null { col, not } => {
                sql.push_str(&format!(
                    "\"{col}\" IS {}NULL",
                    if *not { "NOT " } else { "" }
                ));
            }
            Expr::And(a, b) => {
                sql.push('(');
                a.render(sql, params);
                sql.push_str(" AND ");
                b.render(sql, params);
                sql.push(')');
            }
            Expr::Or(a, b) => {
                sql.push('(');
                a.render(sql, params);
                sql.push_str(" OR ");
                b.render(sql, params);
                sql.push(')');
            }
        }
    }
}

/// 정렬 방향
#[derive(Debug, Clone, Copy)]
pub enum Order {
    Asc,
    Desc,
}

/// 쿼리 빌더 진입점 (명세 §5.3)
pub struct Query;

impl Query {
    /// 엔티티 전체 컬럼 SELECT
    pub fn select<T: Entity>() -> SelectBuilder<T> {
        SelectBuilder::new(false)
    }

    /// COUNT(*) — `fetch_scalar`로 소비
    pub fn count<T: Entity>() -> SelectBuilder<T> {
        SelectBuilder::new(true)
    }
}

/// SELECT 빌더 — 조건·정렬·페이지 조합
pub struct SelectBuilder<T: Entity> {
    count: bool,
    wheres: Option<Expr>,
    order: Vec<(String, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
    _t: std::marker::PhantomData<T>,
}

impl<T: Entity> Clone for SelectBuilder<T> {
    /// 명세 §5.3 — `q.clone()` 후 양쪽 핸들 실행
    fn clone(&self) -> Self {
        Self {
            count: self.count,
            wheres: self.wheres.clone(),
            order: self.order.clone(),
            limit: self.limit,
            offset: self.offset,
            _t: std::marker::PhantomData,
        }
    }
}

impl<T: Entity> SelectBuilder<T> {
    fn new(count: bool) -> Self {
        Self {
            count,
            wheres: None,
            order: Vec::new(),
            limit: None,
            offset: None,
            _t: std::marker::PhantomData,
        }
    }

    /// AND 조건 추가
    pub fn and_where(mut self, e: Expr) -> Self {
        self.wheres = Some(match self.wheres {
            None => e,
            Some(w) => w.and(e),
        });
        self
    }

    /// OR 조건 추가
    pub fn or_where(mut self, e: Expr) -> Self {
        self.wheres = Some(match self.wheres {
            None => e,
            Some(w) => w.or(e),
        });
        self
    }

    /// 정렬 추가
    pub fn order_by(mut self, col: impl Into<String>, order: Order) -> Self {
        self.order.push((col.into(), order));
        self
    }

    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    /// SQL + 파라미터 렌더 — 스키마 인지 검증 포함 (명세 §5.3)
    pub fn build(&self) -> Result<(String, Vec<Value>)> {
        // 참조 컬럼 전수 검증 — SQLite 도달 전 명확한 에러
        let mut cols: Vec<&str> = Vec::new();
        if let Some(w) = &self.wheres {
            w.collect_cols(&mut cols);
        }
        for (c, _) in &self.order {
            cols.push(c);
        }
        for c in cols {
            if !T::COLUMNS_META.iter().any(|m| m.name == c) {
                return Err(Error::Config(format!(
                    "엔티티 \"{}\"에 없는 컬럼: \"{c}\" (쿼리빌더 스키마 검증)",
                    T::TABLE
                )));
            }
        }

        let mut sql = if self.count {
            format!("SELECT COUNT(*) FROM \"{}\"", T::TABLE)
        } else {
            format!("SELECT {} FROM \"{}\"", T::COLUMNS, T::TABLE)
        };
        let mut params: Vec<Value> = Vec::new();
        if let Some(w) = &self.wheres {
            sql.push_str(" WHERE ");
            w.render(&mut sql, &mut params);
        }
        if !self.order.is_empty() {
            let parts: Vec<String> = self
                .order
                .iter()
                .map(|(c, o)| {
                    format!(
                        "\"{c}\" {}",
                        match o {
                            Order::Asc => "ASC",
                            Order::Desc => "DESC",
                        }
                    )
                })
                .collect();
            sql.push_str(&format!(" ORDER BY {}", parts.join(", ")));
        }
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        if let Some(n) = self.offset {
            sql.push_str(&format!(" OFFSET {n}"));
        }
        Ok((sql, params))
    }

    /// N건 실행 — 핸들 대칭 (동기=Result, 비동기=Future)
    pub fn fetch_all<E: Execute>(self, ex: E) -> E::Out<Vec<T>>
    where
        T: FromRow + Send + 'static,
    {
        match self.build() {
            Ok((sql, params)) => ex.run_all(sql, params),
            Err(e) => E::fail(e),
        }
    }

    /// 0~1건 실행
    pub fn fetch_optional<E: Execute>(self, ex: E) -> E::Out<Option<T>>
    where
        T: FromRow + Send + 'static,
    {
        match self.build() {
            Ok((sql, params)) => ex.run_optional(sql, params),
            Err(e) => E::fail(e),
        }
    }

    /// 정확히 1건 (0건 = NotFound)
    pub fn fetch_one<E: Execute>(self, ex: E) -> E::Out<T>
    where
        T: FromRow + Send + 'static,
    {
        match self.build() {
            Ok((sql, params)) => ex.run_one(sql, params),
            Err(e) => E::fail(e),
        }
    }

    /// 스칼라 (COUNT 등)
    pub fn fetch_scalar<E: Execute>(self, ex: E) -> E::Out<i64>
    where
        T: Send + 'static,
    {
        match self.build() {
            Ok((sql, params)) => ex.run_scalar(sql, params),
            Err(e) => E::fail(e),
        }
    }
}

/// 핸들 대칭 실행자 (명세 §5.3 [C-6]) —
/// 동기 = `Result<R>`, 비동기 = Boxed `Future`. `#[database]` 생성 핸들 래퍼에도 구현된다.
pub trait Execute {
    /// 실행 결과 컨테이너
    type Out<R: Send + 'static>;

    fn run_all<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<Value>,
    ) -> Self::Out<Vec<T>>;
    fn run_optional<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<Value>,
    ) -> Self::Out<Option<T>>;
    fn run_one<T: FromRow + Send + 'static>(self, sql: String, params: Vec<Value>) -> Self::Out<T>;
    fn run_scalar(self, sql: String, params: Vec<Value>) -> Self::Out<i64>;
    /// 빌드 단계 에러를 결과 컨테이너로
    fn fail<R: Send + 'static>(e: Error) -> Self::Out<R>;
}

impl Execute for SyncHandle<'_> {
    type Out<R: Send + 'static> = Result<R>;

    fn run_all<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<Value>,
    ) -> Result<Vec<T>> {
        self.query_all(&sql, rusqlite::params_from_iter(params))
    }
    fn run_optional<T: FromRow + Send + 'static>(
        self,
        sql: String,
        params: Vec<Value>,
    ) -> Result<Option<T>> {
        self.query_optional(&sql, rusqlite::params_from_iter(params))
    }
    fn run_one<T: FromRow + Send + 'static>(self, sql: String, params: Vec<Value>) -> Result<T> {
        self.query_one(&sql, rusqlite::params_from_iter(params))
    }
    fn run_scalar(self, sql: String, params: Vec<Value>) -> Result<i64> {
        self.query_scalar(&sql, rusqlite::params_from_iter(params))
    }
    fn fail<R: Send + 'static>(e: Error) -> Result<R> {
        Err(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NULL 비교는 SQL의 3값 논리 대신 IS NULL 계열로 렌더한다.
    #[test]
    fn null_comparisons_render_is_null() {
        let mut sql = String::new();
        let mut params = Vec::new();
        col("value").eq(Value::Null).render(&mut sql, &mut params);
        assert_eq!(sql, "\"value\" IS NULL");
        assert!(params.is_empty());

        sql.clear();
        col("value").ne(Value::Null).render(&mut sql, &mut params);
        assert_eq!(sql, "\"value\" IS NOT NULL");
        assert!(params.is_empty());
    }

    /// LIKE ESCAPE는 패턴과 escape를 모두 바인딩하고 번호를 연속 배정한다.
    #[test]
    fn escaped_like_binds_pattern_and_escape() {
        let mut sql = String::new();
        let mut params = vec![Value::Integer(7)];
        col("name")
            .like_escaped("100!%", '!')
            .render(&mut sql, &mut params);
        assert_eq!(sql, "\"name\" LIKE ?2 ESCAPE ?3");
        assert_eq!(
            params,
            vec![
                Value::Integer(7),
                Value::Text("100!%".into()),
                Value::Text("!".into())
            ]
        );

        sql.clear();
        params.clear();
        col("name")
            .like_escaped("한界%", '界')
            .render(&mut sql, &mut params);
        assert_eq!(params[1], Value::Text("界".into()));
    }

    /// 렌더 단위 검증용 미니 엔티티 (매크로 없이 수동 구현)
    struct T0;
    impl crate::row::FromRow for T0 {
        fn from_row(_: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
            Ok(T0)
        }
    }
    impl Entity for T0 {
        const TABLE: &'static str = "t0";
        const DDL: &'static [&'static str] = &[];
        const COLUMNS: &'static str = "\"id\", \"name\"";
        const COLUMNS_META: &'static [crate::database::ColumnMeta] = &[
            crate::database::ColumnMeta {
                name: "id",
                sql_type: "INTEGER",
                not_null: true,
                pk: true,
                renamed_from: None,
            },
            crate::database::ColumnMeta {
                name: "name",
                sql_type: "TEXT",
                not_null: true,
                pk: false,
                renamed_from: None,
            },
        ];
    }

    /// SQL 렌더 — 조건 조합·IN·정렬·페이지
    #[test]
    fn renders_sql() {
        let (sql, params) = Query::select::<T0>()
            .and_where(col("id").in_list([1i64, 2]).and(col("name").like("k%")))
            .or_where(col("name").is_null())
            .order_by("id", Order::Desc)
            .limit(10)
            .offset(5)
            .build()
            .unwrap();
        assert_eq!(
            sql,
            "SELECT \"id\", \"name\" FROM \"t0\" WHERE ((\"id\" IN (?1, ?2) AND \"name\" LIKE ?3) OR \"name\" IS NULL) ORDER BY \"id\" DESC LIMIT 10 OFFSET 5"
        );
        assert_eq!(params.len(), 3);
    }

    /// 스키마 검증 — 미지 컬럼 = Config 에러
    #[test]
    fn unknown_column_rejected() {
        let r = Query::select::<T0>()
            .and_where(col("nope").eq(1i64))
            .build();
        assert!(matches!(r, Err(Error::Config(_))));
    }

    /// 빈 IN = 항상 거짓
    #[test]
    fn empty_in_list() {
        let (sql, _) = Query::select::<T0>()
            .and_where(col("id").in_list(Vec::<i64>::new()))
            .build()
            .unwrap();
        assert!(sql.contains("0 = 1"));
    }
}
