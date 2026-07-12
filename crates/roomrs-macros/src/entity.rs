//! `#[entity]` 전개 (명세 §5.1, §12b/§12c)
//!
//! 생성물: 보조 속성이 제거된 구조체 + `Entity`/`Insertable`/`FromRow` impl.
//! 생성 코드는 `::roomrs` 파사드 경로를 참조한다 — roomrs-macros 단독 사용 불가.

use crate::util::validate_sql_identifier;
use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{Attribute, Fields, ItemStruct, LitStr, Type};

/// 컬럼 메타 — 필드 파싱 결과
struct Column {
    ident: syn::Ident,
    name: String,
    ty: Type,
    sql_type: &'static str,
    not_null: bool,
    pk: bool,
    autoincrement: bool,
    unique: bool,
    index: bool,
    /// 렌더 완료된 SQL DEFAULT 절 조각 (M-16 — parse_field 에서 확정)
    default: Option<String>,
    json: bool,
    renamed_from: Option<String>,
}

/// 엔티티 수준 속성 인자
struct EntityArgs {
    table: Option<String>,
    multi_instance: bool,
}

/// `#[entity(...)]` 인자 파싱
fn parse_args(args: TokenStream) -> syn::Result<EntityArgs> {
    let mut out = EntityArgs {
        table: None,
        multi_instance: false,
    };
    if args.is_empty() {
        return Ok(out);
    }
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("table") {
            let lit: LitStr = meta.value()?.parse()?;
            validate_sql_identifier(&lit.value(), lit.span())?;
            out.table = Some(lit.value());
            Ok(())
        } else if meta.path.is_ident("multi_instance") {
            out.multi_instance = true;
            Ok(())
        } else {
            Err(meta.error("알 수 없는 entity 인자 — table / multi_instance 만 지원"))
        }
    });
    parser.parse2(args)?;
    Ok(out)
}

/// Rust 타입 → SQLite 타입·NULL 여부.
/// Option<T>는 내부 타입으로 재귀, 미지 타입은 typeless(BLOB affinity)로 선언.
fn map_type(ty: &Type) -> (&'static str, bool) {
    // Option<T> 판별
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            if seg.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = ab.args.first() {
                        let (sql, _) = map_type(inner);
                        return (sql, false); // nullable
                    }
                }
            }
        }
    }

    let name = match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    };

    let sql = match name.as_str() {
        // u64 도 INTEGER — SQLite INTEGER 는 i64 이므로 i64::MAX 초과 값은
        // 런타임 ToSql 에서 실패한다 (usize 와 동일 정책, L-12)
        "bool" | "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64"
        | "usize" => "INTEGER",
        "f32" | "f64" => "REAL",
        "String" => "TEXT",
        "OffsetDateTime" | "PrimitiveDateTime" | "Date" | "Time" => "TEXT",
        "Uuid" => "BLOB",
        "Vec" => {
            // Vec<u8> 만 BLOB — 그 외 Vec은 미지 타입
            match ty {
                Type::Path(tp) => tp
                    .path
                    .segments
                    .last()
                    .and_then(|seg| match &seg.arguments {
                        syn::PathArguments::AngleBracketed(ab) => ab.args.first(),
                        _ => None,
                    })
                    .and_then(|arg| match arg {
                        syn::GenericArgument::Type(Type::Path(inner)) => Some(inner),
                        _ => None,
                    })
                    .filter(|inner| inner.path.is_ident("u8"))
                    .map_or("", |_| "BLOB"),
                _ => "",
            }
        }
        _ => "", // 미지 타입 — typeless 컬럼(BLOB affinity), 사용자 ToSql/FromSql 위임
    };
    (sql, true) // 기본 NOT NULL (Option이 아니므로)
}

/// `Option<T>` 필드에서 내부 타입을 추출한다.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    match arguments.args.first() {
        Some(syn::GenericArgument::Type(inner)) => Some(inner),
        _ => None,
    }
}

/// 필드 하나 → Column 파싱. ignore 필드는 None.
fn parse_field(field: &syn::Field) -> syn::Result<Option<Column>> {
    let ident = field
        .ident
        .clone()
        .expect("named struct만 허용 — 상위에서 검증");
    let mut col = Column {
        name: ident.to_string(),
        ident,
        ty: field.ty.clone(),
        sql_type: "",
        not_null: true,
        pk: false,
        autoincrement: false,
        unique: false,
        index: false,
        default: None,
        json: false,
        renamed_from: None,
    };
    let mut ignored = false;

    for attr in &field.attrs {
        if attr.path().is_ident("pk") {
            col.pk = true;
            // #[pk] 단독 또는 #[pk(autoincrement)]
            if !matches!(attr.meta, syn::Meta::Path(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("autoincrement") {
                        col.autoincrement = true;
                        Ok(())
                    } else {
                        Err(meta.error("알 수 없는 pk 인자 — autoincrement 만 지원"))
                    }
                })?;
            }
        } else if attr.path().is_ident("json") {
            col.json = true;
        } else if attr.path().is_ident("column") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("ignore") {
                    ignored = true;
                    Ok(())
                } else if meta.path.is_ident("name") {
                    let lit: LitStr = meta.value()?.parse()?;
                    validate_sql_identifier(&lit.value(), lit.span())?;
                    col.name = lit.value();
                    Ok(())
                } else if meta.path.is_ident("unique") {
                    col.unique = true;
                    Ok(())
                } else if meta.path.is_ident("index") {
                    col.index = true;
                    Ok(())
                } else if meta.path.is_ident("renamed_from") {
                    let lit: LitStr = meta.value()?.parse()?;
                    validate_sql_identifier(&lit.value(), lit.span())?;
                    col.renamed_from = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("default") {
                    let lit: LitStr = meta.value()?.parse()?;
                    // 파스 시점 렌더 — 에러 span을 리터럴에 맞춘다 (M-16)
                    col.default = Some(render_default(&lit)?);
                    Ok(())
                } else {
                    Err(meta
                        .error("알 수 없는 column 인자 — name/unique/index/default/ignore/renamed_from 만 지원"))
                }
            })?;
        }
    }

    if ignored {
        return Ok(None);
    }

    let (sql_type, not_null) = map_type(&col.ty);
    col.sql_type = if col.json { "TEXT" } else { sql_type };
    col.not_null = not_null;
    Ok(Some(col))
}

/// `#[column(default = "…")]` 값 → SQL DEFAULT 절 렌더 (M-16).
/// - `now` / `CURRENT_TIMESTAMP|DATE|TIME`(전부 대소문자 무관) = 시각 키워드 원문
/// - `true`/`false` = 1/0 (bool FromSql 호환)
/// - `(`로 시작 = SQL 식 원문 (예: `(datetime('now'))`) — 괄호 균형 검증 (L-12)
/// - 유한 숫자 = 원문, nan/inf = 컴파일 에러 (SQLite DEFAULT 로 표현 불가)
/// - 그 외 = 작은따옴표 리터럴 (`'` 이스케이프)
fn render_default(lit: &LitStr) -> syn::Result<String> {
    let v = lit.value();
    // 정책 통일 (L-12): 시각 키워드는 전부 대소문자 무관. 종전엔 `now` 만
    // 정확 일치라 `NOW` 가 문자열 리터럴 'NOW' 로 조용히 강등되는 비일관이
    // 있었다. `now` → CURRENT_TIMESTAMP 매핑 자체는 명세 §5.1 예제 유지.
    if v.eq_ignore_ascii_case("now") || v.eq_ignore_ascii_case("current_timestamp") {
        return Ok("CURRENT_TIMESTAMP".to_string());
    }
    if v.eq_ignore_ascii_case("current_date") || v.eq_ignore_ascii_case("current_time") {
        return Ok(v.to_ascii_uppercase());
    }
    if v == "true" {
        return Ok("1".to_string());
    }
    if v == "false" {
        return Ok("0".to_string());
    }
    if v.starts_with('(') {
        // SQL 식 — 괄호 불균형이면 DDL 전체가 깨져 첫 CREATE TABLE에서야
        // 런타임 에러가 난다. 전개 시점에 잡는다 (L-12)
        if !parens_balanced(&v) {
            return Err(syn::Error::new(
                lit.span(),
                format!("default SQL 식 \"{v}\" 의 괄호가 불균형합니다"),
            ));
        }
        return Ok(v);
    }
    if let Ok(n) = v.parse::<f64>() {
        if n.is_finite() {
            return Ok(v);
        }
        return Err(syn::Error::new(
            lit.span(),
            format!(
                "default 값 \"{v}\" 은 SQLite DEFAULT 로 표현할 수 없습니다 — 유한 숫자만 지원"
            ),
        ));
    }
    Ok(format!("'{}'", v.replace('\'', "''")))
}

/// SQL 식의 괄호 균형 검사 — '…' 문자열 리터럴('' 이스케이프) 안의 괄호는
/// 제외한다 (L-12)
fn parens_balanced(s: &str) -> bool {
    let mut depth: i64 = 0;
    let mut in_str = false;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                // 문자열 안 '' = 이스케이프(문자열 계속), 아니면 여닫이 토글
                if in_str && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 1;
                } else {
                    in_str = !in_str;
                }
            }
            b'(' if !in_str => depth += 1,
            b')' if !in_str => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    depth == 0 && !in_str
}

/// 보조 속성 제거 — 남기면 "unknown attribute" 에러
fn strip_helper_attrs(attrs: &mut Vec<Attribute>) {
    attrs.retain(|a| {
        !(a.path().is_ident("pk") || a.path().is_ident("json") || a.path().is_ident("column"))
    });
}

/// DDL 렌더 — CREATE TABLE + 인덱스들
fn render_ddl(table: &str, cols: &[Column]) -> Vec<String> {
    let mut defs: Vec<String> = Vec::new();
    for c in cols {
        let mut d = format!("\"{}\"", c.name);
        if !c.sql_type.is_empty() {
            d.push(' ');
            d.push_str(c.sql_type);
        }
        if c.pk {
            d.push_str(" PRIMARY KEY");
            if c.sql_type != "INTEGER" {
                d.push_str(" NOT NULL");
            }
            if c.autoincrement {
                d.push_str(" AUTOINCREMENT");
            }
        } else {
            if c.not_null {
                d.push_str(" NOT NULL");
            }
            if c.unique {
                d.push_str(" UNIQUE");
            }
        }
        // default 는 parse_field 에서 이미 SQL 절로 렌더됨 (M-16)
        if let Some(def) = &c.default {
            d.push_str(&format!(" DEFAULT {def}"));
        }
        defs.push(d);
    }

    let mut out = vec![format!(
        "CREATE TABLE IF NOT EXISTS \"{table}\" ({})",
        defs.join(", ")
    )];
    for c in cols.iter().filter(|c| c.index) {
        out.push(format!(
            "CREATE INDEX IF NOT EXISTS \"idx_{table}_{name}\" ON \"{table}\"(\"{name}\")",
            name = c.name
        ));
    }
    out
}

/// 바인딩 값 추출 식 생성 — json 필드는 직렬화, 일반 필드는 ToSql 위임
fn param_expr(c: &Column) -> TokenStream {
    let ident = &c.ident;
    if c.json {
        if option_inner(&c.ty).is_some() {
            quote! {
                match &self.#ident {
                    Some(value) => ::roomrs::ToSqlOutput::Owned(
                        ::roomrs::rusqlite::types::Value::Text(
                            ::roomrs::__private::serde_json::to_string(value)?,
                        ),
                    ),
                    None => ::roomrs::ToSqlOutput::Owned(
                        ::roomrs::rusqlite::types::Value::Null,
                    ),
                }
            }
        } else {
            quote! {
                ::roomrs::ToSqlOutput::Owned(::roomrs::rusqlite::types::Value::Text(
                    ::roomrs::__private::serde_json::to_string(&self.#ident)?,
                ))
            }
        }
    } else {
        quote! { ::roomrs::ToSql::to_sql(&self.#ident)? }
    }
}

/// FromRow 필드 읽기 식 — 컬럼명 기반(SELECT 순서 무관)
fn from_row_expr(c: &Column) -> TokenStream {
    let name = &c.name;
    let ty = &c.ty;
    if c.json {
        if let Some(inner) = option_inner(ty) {
            quote! {{
                let raw: Option<String> = row.get(#name)?;
                match raw {
                    // 구버전은 Option::None을 SQL NULL이 아닌 JSON text `null`로 저장했다.
                    Some(raw) if raw == "null" => None,
                    Some(raw) => Some(::roomrs::__private::serde_json::from_str::<#inner>(&raw).map_err(|e| {
                        ::roomrs::rusqlite::Error::FromSqlConversionFailure(
                            0,
                            ::roomrs::rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?),
                    None => None,
                }
            }}
        } else {
            quote! {{
                let raw: String = row.get(#name)?;
                ::roomrs::__private::serde_json::from_str::<#ty>(&raw).map_err(|e| {
                    ::roomrs::rusqlite::Error::FromSqlConversionFailure(
                        0,
                        ::roomrs::rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?
            }}
        }
    } else {
        quote! { row.get::<_, #ty>(#name)? }
    }
}

/// `#[entity]` 본체
pub fn expand(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_args(args)?;
    let mut item: ItemStruct = syn::parse2(input)?;

    let Fields::Named(_) = &item.fields else {
        return Err(syn::Error::new(
            item.span(),
            "#[entity]는 named 필드 구조체에만 사용할 수 있습니다",
        ));
    };

    let struct_ident = item.ident.clone();
    let table = args.table.unwrap_or_else(|| struct_ident.to_string());
    let multi_instance = args.multi_instance;

    // 필드 파싱 (ignore 제외 컬럼 목록) — ignore 필드 ident는 FromRow Default용으로 수집
    let mut cols: Vec<Column> = Vec::new();
    let mut ignored_idents: Vec<syn::Ident> = Vec::new();
    for field in item.fields.iter() {
        match parse_field(field)? {
            Some(c) => cols.push(c),
            None => ignored_idents.push(field.ident.clone().expect("named 필드 보장")),
        }
    }

    // 컬럼명 중복 검증 (L-13) — #[column(name)] 충돌을 전개 시점에 잡는다.
    // SQLite 식별자는 대소문자 무시
    for (i, c) in cols.iter().enumerate() {
        if let Some(prev) = cols[..i]
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&c.name))
        {
            return Err(syn::Error::new(
                c.ident.span(),
                format!(
                    "컬럼명 중복: \"{}\" — 필드 {} 와 충돌합니다 (#[column(name)] 확인)",
                    c.name, prev.ident
                ),
            ));
        }
    }

    // PK 검증 — 0~1개, autoincrement는 INTEGER 계열만
    let pk_count = cols.iter().filter(|c| c.pk).count();
    if pk_count > 1 {
        return Err(syn::Error::new(
            struct_ident.span(),
            "#[pk] 필드는 최대 1개만 허용됩니다 (복합 PK는 후속 지원)",
        ));
    }
    if let Some(c) = cols
        .iter()
        .find(|c| c.autoincrement && c.sql_type != "INTEGER")
    {
        return Err(syn::Error::new(
            c.ident.span(),
            "#[pk(autoincrement)]는 정수 타입 필드에만 사용할 수 있습니다",
        ));
    }

    // WITHOUT ROWID 미지원(명세 §5.1) — v1에서 옵션 자체가 없으므로 검증 불필요

    let ddl = render_ddl(&table, &cols);
    let columns_joined = cols
        .iter()
        .map(|c| format!("\"{}\"", c.name))
        .collect::<Vec<_>>()
        .join(", ");

    // INSERT 메타 — autoincrement PK는 항상 생략(명세 §12c)
    let ins_cols: Vec<&Column> = cols.iter().filter(|c| !c.autoincrement).collect();
    let ins_columns = ins_cols
        .iter()
        .map(|c| format!("\"{}\"", c.name))
        .collect::<Vec<_>>()
        .join(", ");
    let ins_placeholders = (1..=ins_cols.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let keep_cols = cols
        .iter()
        .map(|c| format!("\"{}\"", c.name))
        .collect::<Vec<_>>()
        .join(", ");
    let keep_placeholders = (1..=cols.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");

    let ins_params: Vec<TokenStream> = ins_cols.iter().map(|c| param_expr(c)).collect();
    let keep_params: Vec<TokenStream> = cols.iter().map(param_expr).collect();

    // FromRow 본문
    let field_reads: Vec<TokenStream> = cols
        .iter()
        .map(|c| {
            let ident = &c.ident;
            let expr = from_row_expr(c);
            quote! { #ident: #expr }
        })
        .collect();
    let ignored_reads: Vec<TokenStream> = ignored_idents
        .iter()
        .map(|ident| quote! { #ident: ::core::default::Default::default() })
        .collect();

    // 보조 속성 제거 후 구조체 재방출
    for field in item.fields.iter_mut() {
        strip_helper_attrs(&mut field.attrs);
    }

    let ddl_lits: Vec<LitStr> = ddl
        .iter()
        .map(|s| LitStr::new(s, struct_ident.span()))
        .collect();

    // 컬럼 메타 — 스냅샷 생성·해시 대조용 (명세 §7)
    let column_metas: Vec<TokenStream> = cols
        .iter()
        .map(|c| {
            let name = &c.name;
            let sql_type = c.sql_type;
            let not_null = c.not_null;
            let pk = c.pk;
            let renamed = match &c.renamed_from {
                Some(s) => quote! { Some(#s) },
                None => quote! { None },
            };
            quote! {
                ::roomrs::ColumnMeta {
                    name: #name,
                    sql_type: #sql_type,
                    not_null: #not_null,
                    pk: #pk,
                    renamed_from: #renamed,
                }
            }
        })
        .collect();

    Ok(quote! {
        #item

        impl ::roomrs::FromRow for #struct_ident {
            /// 컬럼명 기반 행 매핑 — #[entity] 생성
            fn from_row(row: &::roomrs::__private::Row<'_>) -> ::roomrs::rusqlite::Result<Self> {
                Ok(Self {
                    #(#field_reads,)*
                    #(#ignored_reads,)*
                })
            }
        }

        impl ::roomrs::Entity for #struct_ident {
            const TABLE: &'static str = #table;
            const DDL: &'static [&'static str] = &[#(#ddl_lits),*];
            const COLUMNS: &'static str = #columns_joined;
            const COLUMNS_META: &'static [::roomrs::ColumnMeta] = &[#(#column_metas),*];
            const MULTI_INSTANCE: bool = #multi_instance;
        }

        impl ::roomrs::Insertable for #struct_ident {
            const INSERT_COLUMNS: &'static str = #ins_columns;
            const INSERT_PLACEHOLDERS: &'static str = #ins_placeholders;
            const INSERT_COLUMNS_KEEP_PK: &'static str = #keep_cols;
            const INSERT_PLACEHOLDERS_KEEP_PK: &'static str = #keep_placeholders;

            /// PK 생략 바인딩 값 (명세 §12c)
            fn insert_params(&self) -> ::roomrs::Result<Vec<::roomrs::ToSqlOutput<'_>>> {
                Ok(vec![#(#ins_params),*])
            }

            /// PK 포함 바인딩 값 — #[insert(keep_pk)] 용
            fn insert_params_keep_pk(&self) -> ::roomrs::Result<Vec<::roomrs::ToSqlOutput<'_>>> {
                Ok(vec![#(#keep_params),*])
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    /// render_default 헬퍼 — 리터럴 생성 후 렌더
    fn rd(v: &str) -> syn::Result<String> {
        render_default(&LitStr::new(v, Span::call_site()))
    }

    /// 시각 키워드 대소문자 무관 통일 (L-12)
    #[test]
    fn default_time_keywords_case_insensitive() {
        assert_eq!(rd("now").unwrap(), "CURRENT_TIMESTAMP");
        assert_eq!(rd("NOW").unwrap(), "CURRENT_TIMESTAMP");
        assert_eq!(rd("Now").unwrap(), "CURRENT_TIMESTAMP");
        assert_eq!(rd("current_timestamp").unwrap(), "CURRENT_TIMESTAMP");
        assert_eq!(rd("Current_Date").unwrap(), "CURRENT_DATE");
        assert_eq!(rd("CURRENT_TIME").unwrap(), "CURRENT_TIME");
    }

    /// SQL 식 괄호 균형 — 균형 = 원문, 불균형 = 컴파일 에러 (L-12)
    #[test]
    fn default_expr_paren_balance() {
        assert_eq!(rd("(datetime('now'))").unwrap(), "(datetime('now'))");
        // 문자열 리터럴 안 괄호·이스케이프는 균형 계산에서 제외
        assert_eq!(rd("(concat('(', ''''))").unwrap(), "(concat('(', ''''))");
        assert!(rd("(datetime('now')").is_err(), "여는 괄호 초과");
        assert!(rd("(a))(").is_err(), "음수 깊이");
        assert!(rd("('미종결").is_err(), "닫히지 않은 문자열");
    }

    /// 일반 값 렌더 — bool/숫자/문자열 이스케이프 (M-16 기존 정책 유지)
    #[test]
    fn default_plain_values() {
        assert_eq!(rd("true").unwrap(), "1");
        assert_eq!(rd("false").unwrap(), "0");
        assert_eq!(rd("3.5").unwrap(), "3.5");
        assert_eq!(rd("abc").unwrap(), "'abc'");
        assert_eq!(rd("o'clock").unwrap(), "'o''clock'");
        assert!(rd("nan").is_err());
    }
}
