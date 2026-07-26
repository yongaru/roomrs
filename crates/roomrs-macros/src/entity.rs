//! `#[entity]` 전개 (명세 §5.1, §12b/§12c, 결정 40–46)
//!
//! 생성물: 보조 속성이 제거된 구조체 + `Entity`/`Insertable`/`FromRow` impl.
//! 생성 코드는 `::roomrs` 파사드 경로를 참조한다 — roomrs-macros 단독 사용 불가.

use crate::util::validate_sql_identifier;
use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Fields, Ident, ItemStruct, LitStr, Token, Type};

/// 컬럼 메타 — 필드 파싱 결과
struct Column {
    ident: syn::Ident,
    name: String,
    ty: Type,
    /// SQLite type name (map_type 또는 `sql_type` 오버라이드)
    sql_type: String,
    not_null: bool,
    pk: bool,
    autoincrement: bool,
    unique: bool,
    index: bool,
    /// 렌더 완료된 SQL DEFAULT 절 조각 (M-16 — parse_field 에서 확정)
    default: Option<String>,
    json: bool,
    renamed_from: Option<String>,
    /// `#[column(collate = "…")]` (결정 54)
    collate: Option<String>,
    /// `#[column(generated = "…")]` 식 (결정 54)
    generated: Option<String>,
    /// generated 가 STORED 인지 (기본 VIRTUAL)
    generated_stored: bool,
}

/// 인덱스 컬럼 — 이름 + DESC 여부 + optional COLLATE
struct IndexColumn {
    name: String,
    desc: bool,
    /// `columns(name collate nocase)` (결정 54)
    collate: Option<String>,
}

/// 엔티티 수준 복합 인덱스
struct IndexDef {
    name: String,
    columns: Vec<IndexColumn>,
    where_clause: Option<String>,
}

/// 복합 foreign key
struct ForeignKeyDef {
    columns: Vec<String>,
    references: String,
    on_delete: Option<String>,
    on_update: Option<String>,
}

/// 엔티티 수준 속성 인자
struct EntityArgs {
    table: Option<String>,
    primary_key: Option<Vec<String>>,
    uniques: Vec<Vec<String>>,
    indexes: Vec<IndexDef>,
    foreign_keys: Vec<ForeignKeyDef>,
    checks: Vec<String>,
    triggers: Vec<String>,
    /// `#[entity(strict)]` (결정 54)
    strict: bool,
    /// `#[entity(without_rowid)]` (결정 54)
    without_rowid: bool,
}

impl Default for EntityArgs {
    /// 빈 인자 기본값
    fn default() -> Self {
        Self {
            table: None,
            primary_key: None,
            uniques: Vec::new(),
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            checks: Vec::new(),
            triggers: Vec::new(),
            strict: false,
            without_rowid: false,
        }
    }
}

/// SQLite collating sequence 이름 검증 (BINARY/NOCASE/RTRIM 또는 식별자)
fn validate_collate_name(name: &str, span: proc_macro2::Span) -> syn::Result<()> {
    let t = name.trim();
    if t.is_empty() {
        return Err(syn::Error::new(span, "collate 이름이 비어 있습니다"));
    }
    // 내장 3종 또는 사용자 정의 식별자(알파벳 시작)
    if t.eq_ignore_ascii_case("BINARY") || t.eq_ignore_ascii_case("NOCASE") || t.eq_ignore_ascii_case("RTRIM") {
        return Ok(());
    }
    validate_sql_identifier(t, span)
}

/// `columns(a, b desc, name collate nocase)` 목록 파서
fn parse_column_list(input: ParseStream) -> syn::Result<Vec<IndexColumn>> {
    let content;
    syn::parenthesized!(content in input);
    let mut cols = Vec::new();
    while !content.is_empty() {
        let ident: Ident = content.parse()?;
        validate_sql_identifier(&ident.to_string(), ident.span())?;
        let mut desc = false;
        let mut collate: Option<String> = None;
        // 선택 토큰: asc|desc, collate <name> — 순서 자유, 중복 금지
        while content.peek(Ident) {
            let kw: Ident = content.parse()?;
            if kw == "desc" {
                if desc {
                    return Err(syn::Error::new(kw.span(), "desc 가 중복되었습니다"));
                }
                desc = true;
            } else if kw == "asc" {
                desc = false;
            } else if kw == "collate" {
                if collate.is_some() {
                    return Err(syn::Error::new(kw.span(), "collate 가 중복되었습니다"));
                }
                let name: Ident = content.parse().map_err(|_| syn::Error::new(kw.span(), "collate 뒤에 시퀀스 이름이 필요합니다"))?;
                validate_collate_name(&name.to_string(), name.span())?;
                collate = Some(name.to_string().to_ascii_uppercase());
            } else {
                return Err(syn::Error::new(kw.span(), "인덱스 컬럼 수식어는 asc/desc/collate 만 지원"));
            }
        }
        cols.push(IndexColumn { name: ident.to_string(), desc, collate });
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        } else if !content.is_empty() {
            return Err(content.error("컬럼 목록은 쉼표로 구분합니다"));
        }
    }
    if cols.is_empty() {
        return Err(input.error("컬럼 목록이 비어 있습니다"));
    }
    Ok(cols)
}

/// bare ident 목록 `a, b` (unique / foreign_key columns)
fn parse_ident_list(input: ParseStream) -> syn::Result<Vec<String>> {
    let content;
    syn::parenthesized!(content in input);
    let idents: Punctuated<Ident, Token![,]> = content.parse_terminated(Ident::parse, Token![,])?;
    if idents.is_empty() {
        return Err(input.error("컬럼 목록이 비어 있습니다"));
    }
    let mut out = Vec::new();
    for id in idents {
        validate_sql_identifier(&id.to_string(), id.span())?;
        out.push(id.to_string());
    }
    Ok(out)
}

/// `index(...)` 인자 파싱 — `where` 는 Rust 키워드라 Token![where] 로 받는다
fn parse_index_def(input: ParseStream) -> syn::Result<IndexDef> {
    let content;
    syn::parenthesized!(content in input);
    let mut name: Option<String> = None;
    let mut columns: Option<Vec<IndexColumn>> = None;
    let mut where_clause: Option<String> = None;
    while !content.is_empty() {
        // `where` 키워드 전용 분기 (ROADMAP 문법 유지)
        if content.peek(Token![where]) {
            let _: Token![where] = content.parse()?;
            let _: Token![=] = content.parse()?;
            let lit: LitStr = content.parse()?;
            if lit.value().trim().is_empty() {
                return Err(syn::Error::new(lit.span(), "index where 절이 비어 있습니다"));
            }
            where_clause = Some(lit.value());
        } else {
            let key: Ident = content.parse()?;
            if key == "name" {
                let _: Token![=] = content.parse()?;
                let lit: LitStr = content.parse()?;
                validate_sql_identifier(&lit.value(), lit.span())?;
                name = Some(lit.value());
            } else if key == "columns" {
                columns = Some(parse_column_list(&content)?);
            } else {
                return Err(syn::Error::new(key.span(), "알 수 없는 index 인자 — name/columns/where 만 지원"));
            }
        }
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    let name = name.ok_or_else(|| content.error("index 에 name 이 필요합니다"))?;
    let columns = columns.ok_or_else(|| content.error("index 에 columns 가 필요합니다"))?;
    Ok(IndexDef { name, columns, where_clause })
}

/// `foreign_key(...)` 인자 파싱
fn parse_foreign_key_def(input: ParseStream) -> syn::Result<ForeignKeyDef> {
    let content;
    syn::parenthesized!(content in input);
    let mut columns: Option<Vec<String>> = None;
    let mut references: Option<String> = None;
    let mut on_delete: Option<String> = None;
    let mut on_update: Option<String> = None;
    while !content.is_empty() {
        let key: Ident = content.parse()?;
        if key == "columns" {
            columns = Some(parse_ident_list(&content)?);
        } else if key == "references" {
            let _: Token![=] = content.parse()?;
            let lit: LitStr = content.parse()?;
            if lit.value().trim().is_empty() {
                return Err(syn::Error::new(lit.span(), "foreign_key references 가 비어 있습니다"));
            }
            if lit.value().contains('"') {
                return Err(syn::Error::new(lit.span(), "references 에 큰따옴표를 사용할 수 없습니다"));
            }
            references = Some(lit.value());
        } else if key == "on_delete" {
            let _: Token![=] = content.parse()?;
            let lit: LitStr = content.parse()?;
            on_delete = Some(lit.value());
        } else if key == "on_update" {
            let _: Token![=] = content.parse()?;
            let lit: LitStr = content.parse()?;
            on_update = Some(lit.value());
        } else {
            return Err(syn::Error::new(key.span(), "알 수 없는 foreign_key 인자 — columns/references/on_delete/on_update 만 지원"));
        }
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    let columns = columns.ok_or_else(|| content.error("foreign_key 에 columns 가 필요합니다"))?;
    let references = references.ok_or_else(|| content.error("foreign_key 에 references 가 필요합니다"))?;
    Ok(ForeignKeyDef { columns, references, on_delete, on_update })
}

/// `#[entity(...)]` 인자 파싱
fn parse_args(args: TokenStream) -> syn::Result<EntityArgs> {
    let mut out = EntityArgs::default();
    if args.is_empty() {
        return Ok(out);
    }
    // 쉼표 구분 Meta 목록
    let parser = |input: ParseStream| -> syn::Result<Punctuated<syn::Meta, Token![,]>> { Punctuated::parse_terminated(input) };
    let metas: Punctuated<syn::Meta, Token![,]> = parser.parse2(args)?;
    for meta in metas {
        match meta {
            syn::Meta::NameValue(nv) if nv.path.is_ident("table") => {
                let lit = match nv.value {
                    syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => s,
                    other => return Err(syn::Error::new(other.span(), "table 은 문자열 리터럴이어야 합니다")),
                };
                validate_sql_identifier(&lit.value(), lit.span())?;
                out.table = Some(lit.value());
            }
            syn::Meta::NameValue(nv) if nv.path.is_ident("check") => {
                let lit = match nv.value {
                    syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => s,
                    other => return Err(syn::Error::new(other.span(), "check 는 문자열 리터럴이어야 합니다")),
                };
                if lit.value().trim().is_empty() {
                    return Err(syn::Error::new(lit.span(), "check 식이 비어 있습니다"));
                }
                out.checks.push(lit.value());
            }
            syn::Meta::NameValue(nv) if nv.path.is_ident("trigger") => {
                let lit = match nv.value {
                    syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => s,
                    other => return Err(syn::Error::new(other.span(), "trigger 는 경로 문자열이어야 합니다")),
                };
                if lit.value().trim().is_empty() {
                    return Err(syn::Error::new(lit.span(), "trigger 경로가 비어 있습니다"));
                }
                out.triggers.push(lit.value());
            }
            syn::Meta::List(list) if list.path.is_ident("unique") => {
                // Meta::List 토큰은 괄호 안 내용만 — bare `a, b` 파스
                let idents: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated.parse2(list.tokens.clone()).map_err(|e| syn::Error::new(list.span(), format!("unique 컬럼 목록 파스 실패: {e}")))?;
                if idents.is_empty() {
                    return Err(syn::Error::new(list.span(), "unique 컬럼 목록이 비어 있습니다"));
                }
                let mut names = Vec::new();
                for id in idents {
                    validate_sql_identifier(&id.to_string(), id.span())?;
                    names.push(id.to_string());
                }
                out.uniques.push(names);
            }
            syn::Meta::List(list) if list.path.is_ident("primary_key") => {
                if out.primary_key.is_some() {
                    return Err(syn::Error::new(list.span(), "primary_key는 한 번만 선언할 수 있습니다"));
                }
                let idents: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated.parse2(list.tokens.clone()).map_err(|e| syn::Error::new(list.span(), format!("primary_key 필드 목록 파스 실패: {e}")))?;
                if idents.is_empty() {
                    return Err(syn::Error::new(list.span(), "primary_key 필드 목록이 비어 있습니다"));
                }
                let mut names = Vec::with_capacity(idents.len());
                for id in idents {
                    let name = id.to_string();
                    if names.iter().any(|existing| existing == &name) {
                        return Err(syn::Error::new(id.span(), format!("primary_key 필드 중복: {name}")));
                    }
                    names.push(name);
                }
                out.primary_key = Some(names);
            }
            syn::Meta::List(list) if list.path.is_ident("index") => {
                // wrap tokens as parenthesized for parse_index_def
                let wrapped: TokenStream = {
                    let t = list.tokens.clone();
                    quote! { ( #t ) }
                };
                out.indexes.push(parse_index_def.parse2(wrapped)?);
            }
            syn::Meta::List(list) if list.path.is_ident("foreign_key") => {
                let wrapped: TokenStream = {
                    let t = list.tokens.clone();
                    quote! { ( #t ) }
                };
                out.foreign_keys.push(parse_foreign_key_def.parse2(wrapped)?);
            }
            syn::Meta::Path(p) if p.is_ident("strict") => {
                out.strict = true;
            }
            syn::Meta::Path(p) if p.is_ident("without_rowid") => {
                out.without_rowid = true;
            }
            other => {
                return Err(syn::Error::new(other.span(), "알 수 없는 entity 인자 — table/primary_key/unique/index/foreign_key/check/trigger/strict/without_rowid 만 지원"));
            }
        }
    }
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
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default(),
        _ => String::new(),
    };

    let sql = match name.as_str() {
        // u64 도 INTEGER — SQLite INTEGER 는 i64 이므로 i64::MAX 초과 값은
        // 런타임 ToSql 에서 실패한다 (usize 와 동일 정책, L-12)
        "bool" | "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize" => "INTEGER",
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
    let ident = field.ident.clone().ok_or_else(|| syn::Error::new(field.span(), "named struct 필드가 필요합니다"))?;
    let mut col = Column {
        name: ident.to_string(),
        ident,
        ty: field.ty.clone(),
        sql_type: String::new(),
        not_null: true,
        pk: false,
        autoincrement: false,
        unique: false,
        index: false,
        default: None,
        json: false,
        renamed_from: None,
        collate: None,
        generated: None,
        generated_stored: false,
    };
    let mut ignored = false;
    let mut sql_type_override: Option<String> = None;

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
                } else if meta.path.is_ident("sql_type") {
                    let lit: LitStr = meta.value()?.parse()?;
                    if lit.value().trim().is_empty() {
                        return Err(meta.error("sql_type 이 비어 있습니다"));
                    }
                    if lit.value().contains('"') {
                        return Err(meta.error("sql_type 에 큰따옴표를 사용할 수 없습니다"));
                    }
                    sql_type_override = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("default") {
                    let lit: LitStr = meta.value()?.parse()?;
                    // 파스 시점 렌더 — 에러 span을 리터럴에 맞춘다 (M-16)
                    col.default = Some(render_default(&lit)?);
                    Ok(())
                } else if meta.path.is_ident("collate") {
                    let lit: LitStr = meta.value()?.parse()?;
                    validate_collate_name(&lit.value(), lit.span())?;
                    col.collate = Some(lit.value().trim().to_ascii_uppercase());
                    Ok(())
                } else if meta.path.is_ident("generated") {
                    let lit: LitStr = meta.value()?.parse()?;
                    let expr = lit.value();
                    if expr.trim().is_empty() {
                        return Err(meta.error("generated 식이 비어 있습니다"));
                    }
                    if !parens_balanced(&expr) {
                        return Err(syn::Error::new(lit.span(), format!("generated 식 \"{expr}\" 의 괄호가 불균형합니다")));
                    }
                    col.generated = Some(expr);
                    Ok(())
                } else if meta.path.is_ident("stored") {
                    // bare `stored` flag for generated column
                    col.generated_stored = true;
                    Ok(())
                } else {
                    Err(meta.error("알 수 없는 column 인자 — name/unique/index/default/ignore/renamed_from/sql_type/collate/generated/stored 만 지원"))
                }
            })?;
        }
    }

    if ignored {
        return Ok(None);
    }

    // generated 비호환 조합 (결정 54)
    if col.generated.is_some() {
        if col.default.is_some() {
            return Err(syn::Error::new(col.ident.span(), "generated 컬럼에 default 를 함께 쓸 수 없습니다"));
        }
        if col.pk {
            return Err(syn::Error::new(col.ident.span(), "generated 컬럼은 PK 가 될 수 없습니다"));
        }
        if col.autoincrement {
            return Err(syn::Error::new(col.ident.span(), "generated 컬럼에 autoincrement 를 쓸 수 없습니다"));
        }
    } else if col.generated_stored {
        return Err(syn::Error::new(col.ident.span(), "stored 는 generated 와 함께만 사용할 수 있습니다"));
    }

    let (sql_type, not_null) = map_type(&col.ty);
    col.sql_type = if col.json {
        "TEXT".into()
    } else if let Some(over) = sql_type_override {
        over
    } else {
        sql_type.into()
    };
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
            return Err(syn::Error::new(lit.span(), format!("default SQL 식 \"{v}\" 의 괄호가 불균형합니다")));
        }
        return Ok(v);
    }
    if let Ok(n) = v.parse::<f64>() {
        if n.is_finite() {
            return Ok(v);
        }
        return Err(syn::Error::new(lit.span(), format!("default 값 \"{v}\" 은 SQLite DEFAULT 로 표현할 수 없습니다 — 유한 숫자만 지원")));
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
    attrs.retain(|a| !(a.path().is_ident("pk") || a.path().is_ident("json") || a.path().is_ident("column")));
}

/// DDL 렌더 — CREATE TABLE + 단일/복합 인덱스 (결정 40–45)
fn render_ddl(table: &str, cols: &[Column], primary_key: &[String], args: &EntityArgs) -> syn::Result<Vec<String>> {
    let composite_pk = primary_key.len() > 1;

    let mut defs: Vec<String> = Vec::new();
    for c in cols {
        let mut d = format!("\"{}\"", c.name);
        if !c.sql_type.is_empty() {
            d.push(' ');
            d.push_str(&c.sql_type);
        }
        if primary_key.len() == 1 && primary_key.first().is_some_and(|name| name == &c.name) {
            // 단일 PK — 컬럼-level (INTEGER PRIMARY KEY affinity 유지)
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
        if let Some(collate) = &c.collate {
            d.push_str(&format!(" COLLATE {collate}"));
        }
        // generated (결정 54) — DEFAULT 와 상호 배타 (parse_field 검증)
        if let Some(expr) = &c.generated {
            let mode = if c.generated_stored { "STORED" } else { "VIRTUAL" };
            d.push_str(&format!(" GENERATED ALWAYS AS ({expr}) {mode}"));
        }
        defs.push(d);
    }

    // 복합 PRIMARY KEY (결정 40)
    if composite_pk {
        let parts: Vec<String> = primary_key.iter().map(|name| format!("\"{name}\"")).collect();
        defs.push(format!("PRIMARY KEY ({})", parts.join(", ")));
    }

    // table-level UNIQUE (결정 41)
    for u in &args.uniques {
        let parts: Vec<String> = u.iter().map(|n| format!("\"{n}\"")).collect();
        defs.push(format!("UNIQUE ({})", parts.join(", ")));
    }

    // CHECK (결정 44)
    for expr in &args.checks {
        defs.push(format!("CHECK ({expr})"));
    }

    // FOREIGN KEY (결정 43)
    for fk in &args.foreign_keys {
        let cols_sql: Vec<String> = fk.columns.iter().map(|n| format!("\"{n}\"")).collect();
        let mut clause = format!("FOREIGN KEY ({}) REFERENCES {}", cols_sql.join(", "), fk.references);
        if let Some(od) = &fk.on_delete {
            clause.push_str(&format!(" ON DELETE {od}"));
        }
        if let Some(ou) = &fk.on_update {
            clause.push_str(&format!(" ON UPDATE {ou}"));
        }
        defs.push(clause);
    }

    let mut create = format!("CREATE TABLE IF NOT EXISTS \"{table}\" ({})", defs.join(", "));
    // STRICT / WITHOUT ROWID 는 쉼표로 구분된 테이블 옵션 꼬리 (결정 54, SQLite 문법)
    let mut options: Vec<&str> = Vec::new();
    if args.strict {
        options.push("STRICT");
    }
    if args.without_rowid {
        options.push("WITHOUT ROWID");
    }
    if !options.is_empty() {
        create.push(' ');
        create.push_str(&options.join(", "));
    }
    let mut out = vec![create];

    // 단일 컬럼 index 속성
    for c in cols.iter().filter(|c| c.index) {
        out.push(format!("CREATE INDEX IF NOT EXISTS \"idx_{table}_{name}\" ON \"{table}\"(\"{name}\")", name = c.name));
    }

    // 엔티티 수준 복합/정렬/partial/collate index (결정 42/54)
    for idx in &args.indexes {
        let col_parts: Vec<String> = idx
            .columns
            .iter()
            .map(|c| {
                let mut part = format!("\"{}\"", c.name);
                if let Some(col) = &c.collate {
                    part.push_str(&format!(" COLLATE {col}"));
                }
                if c.desc {
                    part.push_str(" DESC");
                }
                part
            })
            .collect();
        let mut ddl = format!("CREATE INDEX IF NOT EXISTS \"{}\" ON \"{table}\"({})", idx.name, col_parts.join(", "));
        if let Some(w) = &idx.where_clause {
            ddl.push_str(&format!(" WHERE {w}"));
        }
        out.push(ddl);
    }

    Ok(out)
}

/// 제약/인덱스가 참조하는 컬럼이 엔티티에 있는지 검증
fn validate_refs(cols: &[Column], args: &EntityArgs, span: proc_macro2::Span) -> syn::Result<()> {
    let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
    let has = |n: &str| names.iter().any(|c| c.eq_ignore_ascii_case(n));

    for u in &args.uniques {
        for n in u {
            if !has(n) {
                return Err(syn::Error::new(span, format!("unique 컬럼 \"{n}\" 이 엔티티에 없습니다")));
            }
        }
    }
    for idx in &args.indexes {
        for c in &idx.columns {
            if !has(&c.name) {
                return Err(syn::Error::new(span, format!("index \"{}\" 컬럼 \"{}\" 이 엔티티에 없습니다", idx.name, c.name)));
            }
        }
    }
    for fk in &args.foreign_keys {
        for n in &fk.columns {
            if !has(n) {
                return Err(syn::Error::new(span, format!("foreign_key 컬럼 \"{n}\" 이 엔티티에 없습니다")));
            }
        }
    }
    Ok(())
}

/// trigger 파일 로드 결과 — 선언 경로·절대 경로(include 의존성)·내용 hash
struct LoadedTrigger {
    /// `#[entity(trigger = "…")]` 에 쓴 상대 경로
    rel_path: String,
    /// `include_bytes!` 용 절대 경로 (슬래시 정규화)
    abs_path: String,
    /// FNV-1a 64 of file bytes
    content_hash: u64,
}

/// trigger 파일 읽기. 경로 = CARGO_MANIFEST_DIR 기준.
/// 절대 경로는 `include_bytes!` 의존성 등록에 쓴다 — 파일 변경 = 매크로 재전개 (결정 46).
fn load_triggers(paths: &[String], span: proc_macro2::Span) -> syn::Result<Vec<LoadedTrigger>> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| syn::Error::new(span, "CARGO_MANIFEST_DIR 없음 — #[entity(trigger=…)] 는 cargo 빌드에서만 사용할 수 있습니다"))?;
    let mut out = Vec::new();
    for rel in paths {
        let full = std::path::Path::new(&manifest).join(rel);
        let bytes = std::fs::read(&full).map_err(|e| syn::Error::new(span, format!("trigger 파일을 읽을 수 없습니다 (\"{rel}\"): {e}")))?;
        // rustc include_bytes! 는 슬래시 경로를 안정적으로 처리한다 (database 스냅샷 의존성과 동일, M-8)
        let abs_path = full.canonicalize().unwrap_or(full).to_string_lossy().replace('\\', "/");
        out.push(LoadedTrigger { rel_path: rel.clone(), abs_path, content_hash: fnv1a64(&bytes) });
    }
    Ok(out)
}

/// FNV-1a 64 — roomrs-migrate 와 동일 상수 (결정 46 hash 일치)
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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
        return Err(syn::Error::new(item.span(), "#[entity]는 named 필드 구조체에만 사용할 수 있습니다"));
    };

    let struct_ident = item.ident.clone();
    let table = args.table.clone().unwrap_or_else(|| struct_ident.to_string());

    // 필드 파싱 (ignore 제외 컬럼 목록) — ignore 필드 ident는 FromRow Default용으로 수집
    let mut cols: Vec<Column> = Vec::new();
    let mut ignored_idents: Vec<syn::Ident> = Vec::new();
    for field in item.fields.iter() {
        match parse_field(field)? {
            Some(c) => cols.push(c),
            None => ignored_idents.push(field.ident.clone().ok_or_else(|| syn::Error::new(field.span(), "named struct 필드가 필요합니다"))?),
        }
    }

    // 컬럼명 중복 검증 (L-13) — #[column(name)] 충돌을 전개 시점에 잡는다.
    // SQLite 식별자는 대소문자 무시
    for (i, c) in cols.iter().enumerate() {
        if let Some(prev) = cols[..i].iter().find(|p| p.name.eq_ignore_ascii_case(&c.name)) {
            return Err(syn::Error::new(c.ident.span(), format!("컬럼명 중복: \"{}\" — 필드 {} 와 충돌합니다 (#[column(name)] 확인)", c.name, prev.ident)));
        }
    }

    // entity-level PK를 필드 메타에 반영하고 이중 표기 불일치를 export용 메타로 보존한다(결정 56).
    let field_primary_key: Vec<String> = cols.iter().filter(|c| c.pk).map(|c| c.ident.to_string()).collect();
    let primary_key_error = if let Some(entity_primary_key) = &args.primary_key {
        for name in entity_primary_key {
            if !cols.iter().any(|c| c.ident == name.as_str()) {
                return Err(syn::Error::new(struct_ident.span(), format!("primary_key 필드 \"{name}\"이 엔티티에 없거나 #[column(ignore)]입니다")));
            }
        }
        if field_primary_key.is_empty() {
            for col in &mut cols {
                col.pk = entity_primary_key.iter().any(|name| col.ident == name.as_str());
            }
            None
        } else if field_primary_key == *entity_primary_key {
            None
        } else {
            Some(format!("{} PRIMARY KEY 선언 불일치: 필드 #[pk] = ({}), #[entity(primary_key(...))] = ({}) — 한쪽을 제거하거나 목록과 순서를 같게 맞추세요", struct_ident, field_primary_key.join(", "), entity_primary_key.join(", ")))
        }
    } else {
        None
    };
    let primary_key_fields = if field_primary_key.is_empty() { args.primary_key.as_deref().unwrap_or_default() } else { &field_primary_key };
    let primary_key_columns: Vec<String> = primary_key_fields
        .iter()
        .map(|field_name| cols.iter().find(|col| col.ident == field_name.as_str()).map(|col| col.name.clone()).ok_or_else(|| syn::Error::new(struct_ident.span(), format!("PRIMARY KEY 필드 메타를 찾을 수 없습니다: {field_name}"))))
        .collect::<syn::Result<_>>()?;

    // PK 검증 — 복합 허용, autoincrement는 단독 INTEGER 만 (결정 40)
    let pk_count = cols.iter().filter(|c| c.pk).count();
    if let Some(c) = cols.iter().find(|c| c.autoincrement) {
        if pk_count > 1 {
            return Err(syn::Error::new(c.ident.span(), "#[pk(autoincrement)]는 다른 #[pk] 와 함께 사용할 수 없습니다 — SQLite 단일 INTEGER PRIMARY KEY 전용"));
        }
        if c.sql_type != "INTEGER" {
            return Err(syn::Error::new(c.ident.span(), "#[pk(autoincrement)]는 정수 타입 필드에만 사용할 수 있습니다"));
        }
    }

    validate_refs(&cols, &args, struct_ident.span())?;
    let triggers = load_triggers(&args.triggers, struct_ident.span())?;

    // WITHOUT ROWID + INTEGER PRIMARY KEY autoincrement 는 SQLite 비호환
    if args.without_rowid {
        if cols.iter().any(|c| c.autoincrement) {
            return Err(syn::Error::new(struct_ident.span(), "without_rowid 테이블에서는 autoincrement 를 쓸 수 없습니다"));
        }
        if !cols.iter().any(|c| c.pk) {
            return Err(syn::Error::new(struct_ident.span(), "without_rowid 테이블에는 PRIMARY KEY 가 필요합니다"));
        }
    }

    let ddl = render_ddl(&table, &cols, &primary_key_columns, &args)?;
    let strict = args.strict;
    let without_rowid = args.without_rowid;
    let primary_key_error = match primary_key_error {
        Some(message) => quote! { Some(#message) },
        None => quote! { None },
    };
    let columns_joined = cols.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", ");

    // INSERT 메타 — autoincrement PK는 항상 생략(명세 §12c)
    let ins_cols: Vec<&Column> = cols.iter().filter(|c| !c.autoincrement).collect();
    let ins_columns = ins_cols.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", ");
    let ins_placeholders = (1..=ins_cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let keep_cols = cols.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", ");
    let keep_placeholders = (1..=cols.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");

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
    let ignored_reads: Vec<TokenStream> = ignored_idents.iter().map(|ident| quote! { #ident: ::core::default::Default::default() }).collect();

    // 보조 속성 제거 후 구조체 재방출
    for field in item.fields.iter_mut() {
        strip_helper_attrs(&mut field.attrs);
    }

    let ddl_lits: Vec<LitStr> = ddl.iter().map(|s| LitStr::new(s, struct_ident.span())).collect();

    // 컬럼 메타 — 스냅샷 생성·해시 대조용 (명세 §7)
    let column_metas: Vec<TokenStream> = cols
        .iter()
        .map(|c| {
            let name = &c.name;
            let sql_type = &c.sql_type;
            let not_null = c.not_null;
            let pk = c.pk;
            let renamed = match &c.renamed_from {
                Some(s) => quote! { Some(#s) },
                None => quote! { None },
            };
            let default_sql = match &c.default {
                Some(s) => quote! { Some(#s) },
                None => quote! { None },
            };
            let collate = match &c.collate {
                Some(s) => quote! { Some(#s) },
                None => quote! { None },
            };
            let generated = match &c.generated {
                Some(expr) => {
                    let stored = c.generated_stored;
                    quote! {
                        Some(::roomrs::GeneratedColumnMeta {
                            expr: #expr,
                            stored: #stored,
                        })
                    }
                }
                None => quote! { None },
            };
            quote! {
                ::roomrs::ColumnMeta {
                    name: #name,
                    sql_type: #sql_type,
                    not_null: #not_null,
                    pk: #pk,
                    renamed_from: #renamed,
                    default_sql: #default_sql,
                    collate: #collate,
                    generated: #generated,
                }
            }
        })
        .collect();

    let trigger_metas: Vec<TokenStream> = triggers
        .iter()
        .map(|t| {
            let path = &t.rel_path;
            let hash = t.content_hash;
            quote! {
                ::roomrs::TriggerMeta {
                    path: #path,
                    content_hash: #hash,
                }
            }
        })
        .collect();

    // trigger 파일 변경 → rustc 재전개 (결정 46). 사장 상수는 링커가 제거.
    let trigger_deps: Vec<TokenStream> = triggers
        .iter()
        .map(|t| {
            let abs = &t.abs_path;
            quote! { const _: &[u8] = ::core::include_bytes!(#abs); }
        })
        .collect();

    Ok(quote! {
        #item

        #(#trigger_deps)*

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
            const TRIGGERS: &'static [::roomrs::TriggerMeta] = &[#(#trigger_metas),*];
            const STRICT: bool = #strict;
            const WITHOUT_ROWID: bool = #without_rowid;
            const SCHEMA_VALIDATION_ERROR: Option<&'static str> = #primary_key_error;
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

    /// trigger expand — include_bytes! 의존성 방출 + content_hash 일치 (결정 46)
    #[test]
    fn trigger_expand_emits_include_bytes_dep() {
        let input = quote::quote! {
            struct T {
                #[pk]
                id: i64,
            }
        };
        let args = quote::quote! {
            table = "t",
            trigger = "fixtures/sample_trigger.sql"
        };
        let out = expand(args, input).expect("expand");
        let s = out.to_string();
        assert!(s.contains("include_bytes"), "trigger 파일 의존성 등록 필수: {s}");
        assert!(s.contains("content_hash"), "hash 메타 방출: {s}");
        // 파일 bytes 와 동일 hash 가 리터럴로 박혀 있는지
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/sample_trigger.sql")).unwrap();
        let h = fnv1a64(&bytes);
        assert!(s.contains(&h.to_string()), "content_hash={h} 리터럴 포함: {s}");
    }

    /// 복합 PK DDL — table-level PRIMARY KEY
    #[test]
    fn render_composite_pk_ddl() {
        let cols = vec![
            Column {
                ident: Ident::new("a", Span::call_site()),
                name: "a".into(),
                ty: syn::parse_quote!(String),
                sql_type: "TEXT".into(),
                not_null: true,
                pk: true,
                autoincrement: false,
                unique: false,
                index: false,
                default: None,
                json: false,
                renamed_from: None,
                collate: None,
                generated: None,
                generated_stored: false,
            },
            Column {
                ident: Ident::new("b", Span::call_site()),
                name: "b".into(),
                ty: syn::parse_quote!(String),
                sql_type: "TEXT".into(),
                not_null: true,
                pk: true,
                autoincrement: false,
                unique: false,
                index: false,
                default: None,
                json: false,
                renamed_from: None,
                collate: None,
                generated: None,
                generated_stored: false,
            },
        ];
        let ddl = render_ddl("t", &cols, &["a".to_owned(), "b".to_owned()], &EntityArgs::default()).unwrap();
        assert!(ddl[0].contains("PRIMARY KEY (\"a\", \"b\")"), "{}", ddl[0]);
        assert!(!ddl[0].contains("\"a\" TEXT PRIMARY KEY"), "{}", ddl[0]);
    }

    /// entity-level 복합 PK가 필드 메타와 table-level DDL에 반영된다.
    #[test]
    fn entity_primary_key_expands_composite_ddl() {
        let expanded = expand(
            quote! { table = "payments", primary_key(payment_id, store_id) },
            quote! {
                struct Payment {
                    store_id: String,
                    payment_id: String,
                    amount: i64,
                }
            },
        )
        .unwrap()
        .to_string();

        assert!(expanded.contains("PRIMARY KEY (\\\"payment_id\\\", \\\"store_id\\\")"), "{expanded}");
        assert!(expanded.contains("SCHEMA_VALIDATION_ERROR : Option < & 'static str > = None"), "{expanded}");
    }

    /// 동일한 이중 PK 표기는 허용하고 다른 목록은 export용 오류 메타로 남긴다.
    #[test]
    fn duplicate_primary_key_declarations_require_exact_match() {
        let same = expand(
            quote! { primary_key(store_id, payment_id) },
            quote! {
                struct Payment {
                    #[pk]
                    store_id: String,
                    #[pk]
                    payment_id: String,
                }
            },
        )
        .unwrap()
        .to_string();
        assert!(same.contains("SCHEMA_VALIDATION_ERROR : Option < & 'static str > = None"), "{same}");

        let conflict = expand(
            quote! { primary_key(store_id, payment_id) },
            quote! {
                struct Payment {
                    #[pk]
                    store_id: String,
                    payment_id: String,
                    #[pk]
                    sequence: i64,
                }
            },
        )
        .unwrap()
        .to_string();
        assert!(conflict.contains("PRIMARY KEY 선언 불일치"), "{conflict}");
        assert!(conflict.contains("한쪽을 제거하거나 목록과 순서를 같게 맞추세요"), "{conflict}");
    }
}
