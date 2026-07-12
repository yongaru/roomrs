//! `#[dao]` 전개 (명세 §5.2, §5.9)
//!
//! 입력 trait는 DSL — 매크로가 소비·재작성한다(명세 §5 주의).
//! 생성물:
//!   - 동기 trait `XxxDao` (속성 제거된 시그니처)
//!   - `XxxDaoOn<C: SqlContext>` — 풀-바운드/tx-바운드 공용 구현체
//!
//! 파라미터 정합성(명세 §5.2): `:name` ↔ 인자 누락/미사용 = 컴파일 에러 (스냅샷 무관 로컬 검증)

use crate::util::extract_named_params;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{FnArg, ItemTrait, LitStr, Pat, ReturnType, TraitItem, Type};

/// 메서드 종류 — SQL은 LitStr로 보존해 에러 span을 SQL 리터럴에 맞춘다 (L-16)
enum MethodKind {
    /// #[query("SQL")] — unchecked/with_relations 플래그
    Query {
        sql: LitStr,
        unchecked: bool,
        with_relations: bool,
    },
    /// #[insert] — on_conflict, keep_pk
    Insert {
        on_conflict: Option<String>,
        keep_pk: bool,
    },
    /// #[update("SQL")] / #[delete("SQL")] (+ unchecked)
    Write { sql: LitStr, unchecked: bool },
    /// #[transaction] — 본문 있는 유일한 종류, tx-바운드 DAO 문맥으로 재작성 (명세 §5.9)
    Transaction,
}

/// 본문 안의 `self`를 tx-바운드 DAO 식별자로 치환하는 방문자 (명세 §5.9 재작성).
/// 매크로 토큰 안의 `self`는 재작성 불가 — 발견 시 에러 수집 (H-8)
struct ReplaceSelf {
    /// 매크로 토큰 안에서 발견한 `self` 에러들 (H-8)
    errors: Vec<syn::Error>,
}

impl syn::visit_mut::VisitMut for ReplaceSelf {
    /// `self` 경로 식(수신자 포함)을 `__tx_dao`로 교체
    fn visit_expr_mut(&mut self, e: &mut syn::Expr) {
        if let syn::Expr::Path(p) = e {
            if p.path.is_ident("self") {
                *e = syn::parse_quote!(__tx_dao);
                return;
            }
        }
        syn::visit_mut::visit_expr_mut(self, e);
    }

    /// 매크로 호출(matches!/assert! 등) 토큰은 syn이 파싱하지 않아 치환이
    /// 침묵 실패한다 — 토큰 스캔으로 `self`를 찾으면 컴파일 에러 (H-8)
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        if let Some(span) = find_self_in_tokens(mac.tokens.clone()) {
            self.errors.push(syn::Error::new(
                span,
                "트랜잭션 본문의 매크로 호출 안에서는 self 를 사용할 수 없습니다 — 지역 변수로 추출하세요",
            ));
        }
        syn::visit_mut::visit_macro_mut(self, mac);
    }
}

/// TokenStream 재귀 스캔 — `self` 식별자 발견 시 해당 토큰의 span 반환 (H-8).
/// 문자열 리터럴 안의 `{self}` / `{self:…}` 포맷 암묵 캡처도 탐지한다 —
/// `format!("{self}")` 는 Ident 토큰이 아니라 리터럴이라 종전엔 놓쳤다 (L-18)
fn find_self_in_tokens(ts: TokenStream) -> Option<proc_macro2::Span> {
    for tt in ts {
        match tt {
            proc_macro2::TokenTree::Ident(id) if id == "self" => return Some(id.span()),
            proc_macro2::TokenTree::Literal(lit) if literal_captures_self(&lit.to_string()) => {
                return Some(lit.span());
            }
            proc_macro2::TokenTree::Group(g) => {
                if let Some(s) = find_self_in_tokens(g.stream()) {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// 리터럴 소스 표기에서 `{self}` / `{self:…}` 포맷 캡처 검사 —
/// `{{` 이스케이프(리터럴 중괄호)는 캡처가 아니므로 제외한다 (L-18)
fn literal_captures_self(src: &str) -> bool {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // `{{` = 이스케이프된 중괄호 — 캡처 아님
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                i += 2;
                continue;
            }
            // 포맷 캡처는 공백 없이 `{self}` 또는 `{self:스펙}` 형태만 유효
            let rest = &src[i + 1..];
            if let Some(after) = rest.strip_prefix("self") {
                if after.starts_with('}') || after.starts_with(':') {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// SQL 속성 인자 파싱 — `"SQL"` / `unchecked,` / `with_relations,` 플래그 조합
fn parse_sql_args(attr: &syn::Attribute) -> syn::Result<(LitStr, bool, bool)> {
    attr.parse_args_with(|input: syn::parse::ParseStream| {
        let mut unchecked = false;
        let mut with_relations = false;
        while input.peek(syn::Ident) {
            let ident: syn::Ident = input.parse()?;
            if ident == "unchecked" {
                unchecked = true;
            } else if ident == "with_relations" {
                with_relations = true;
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    "알 수 없는 플래그 — unchecked / with_relations 만 지원",
                ));
            }
            input.parse::<syn::Token![,]>()?;
        }
        let lit: LitStr = input.parse()?;
        Ok((lit, unchecked, with_relations))
    })
}

/// write 판별 대상 DML 키워드
const DML_KEYWORDS: [&str; 4] = ["INSERT", "UPDATE", "DELETE", "REPLACE"];

/// SQL write 판별 (M-12 컴파일 검사용). 첫 키워드 기준이되, `WITH`(CTE) 선두는
/// 최상위(괄호 깊이 0) 토큰에서 DML 키워드를 스캔한다 — 첫 키워드만 보면
/// `WITH … DELETE` 를 read로 오분류해 Result<u64> 반환이 오탐 에러가 됐다 (L-17)
fn sql_is_write(sql: &str) -> bool {
    let first = sql.split_whitespace().next().unwrap_or("");
    if DML_KEYWORDS.iter().any(|k| first.eq_ignore_ascii_case(k)) {
        return true;
    }
    if first.eq_ignore_ascii_case("WITH") {
        return has_top_level_dml(sql);
    }
    false
}

/// 괄호 깊이 0의 단어 토큰 중 DML 키워드 존재 검사 — 문자열('…')·인용
/// 식별자(" ` [)·주석(-- /* */) 안은 스킵한다 (L-17)
fn has_top_level_dml(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut depth: u32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // 문자열 리터럴 — '' 이스케이프
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            // 인용 식별자
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                i += 1;
            }
            b'`' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    i += 1;
                }
                i += 1;
            }
            b'[' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                i += 1;
            }
            // 라인 주석
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            // 블록 주석
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            // 단어 토큰 — 깊이 0에서만 DML 대조
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                if depth == 0 {
                    let word = &sql[start..i];
                    if DML_KEYWORDS.iter().any(|k| word.eq_ignore_ascii_case(k)) {
                        return true;
                    }
                }
            }
            _ => i += 1,
        }
    }
    false
}

/// 반환 타입 분류 — Result<...> 내부 형태
enum RetShape {
    /// Result<Vec<T>>
    Many(Type),
    /// Result<Option<T>>
    Optional(Type),
    /// Result<T> (1건 — 0건이면 NotFound)
    One(Type),
    /// Result<u64|usize> — 영향 행 수
    Affected,
    /// Result<i64> + #[insert] — 새 rowid
    InsertId,
    /// LiveQuery<Vec<T> | Option<T> | T> — 라이브 쿼리 (명세 §5.6)
    Live(LiveShape, Type),
}

/// LiveQuery 내부 형태
enum LiveShape {
    Many,
    Optional,
    Scalar,
}

/// `Result<X>` 에서 X 추출
fn unwrap_result(ret: &ReturnType) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = ret else {
        return Err(syn::Error::new(
            ret.span(),
            "DAO 메서드는 Result<...>를 반환해야 합니다",
        ));
    };
    if let Type::Path(tp) = ty.as_ref() {
        if let Some(seg) = tp.path.segments.last() {
            if seg.ident == "Result" {
                if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = ab.args.first() {
                        return Ok(inner.clone());
                    }
                }
            }
        }
    }
    Err(syn::Error::new(
        ty.span(),
        "DAO 메서드는 Result<...>를 반환해야 합니다",
    ))
}

/// 제네릭 1개 타입(Vec<T>/Option<T>)의 내부 추출
fn unwrap_generic(ty: &Type, name: &str) -> Option<Type> {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            if seg.ident == name {
                if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = ab.args.first() {
                        return Some(inner.clone());
                    }
                }
            }
        }
    }
    None
}

/// 반환 타입 분류
fn classify_return(ret: &ReturnType, is_insert: bool) -> syn::Result<RetShape> {
    // LiveQuery<...>는 Result 밖 (명세 §5.2 반환 규칙)
    if let ReturnType::Type(_, ty) = ret {
        if let Some(live_inner) = unwrap_generic(ty, "LiveQuery") {
            if let Some(t) = unwrap_generic(&live_inner, "Vec") {
                return Ok(RetShape::Live(LiveShape::Many, t));
            }
            if let Some(t) = unwrap_generic(&live_inner, "Option") {
                return Ok(RetShape::Live(LiveShape::Optional, t));
            }
            return Ok(RetShape::Live(LiveShape::Scalar, live_inner));
        }
    }

    let inner = unwrap_result(ret)?;
    if unwrap_generic(&inner, "LiveQuery").is_some() {
        return Err(syn::Error::new(
            inner.span(),
            "LiveQuery는 Result로 감싸지 않고 직접 반환해야 합니다",
        ));
    }
    if let Some(t) = unwrap_generic(&inner, "Vec") {
        // Vec<u8>은 스칼라 취급이 자연스러우나 DAO 반환으로는 비현실적 — Many 유지
        return Ok(RetShape::Many(t));
    }
    if let Some(t) = unwrap_generic(&inner, "Option") {
        return Ok(RetShape::Optional(t));
    }
    if let Type::Path(tp) = &inner {
        if let Some(seg) = tp.path.segments.last() {
            let name = seg.ident.to_string();
            if is_insert && name == "i64" {
                return Ok(RetShape::InsertId);
            }
            if name == "u64" || name == "usize" {
                return Ok(RetShape::Affected);
            }
        }
    }
    Ok(RetShape::One(inner))
}

/// 메서드 인자 이름 수집 (&self 제외)
fn arg_idents(sig: &syn::Signature) -> syn::Result<Vec<syn::Ident>> {
    let mut out = Vec::new();
    for arg in sig.inputs.iter().skip(1) {
        let FnArg::Typed(pt) = arg else {
            return Err(syn::Error::new(
                arg.span(),
                "DAO 메서드 수신자는 &self여야 합니다",
            ));
        };
        let Pat::Ident(pi) = pt.pat.as_ref() else {
            return Err(syn::Error::new(
                pt.pat.span(),
                "DAO 인자는 단순 식별자여야 합니다",
            ));
        };
        out.push(pi.ident.clone());
    }
    Ok(out)
}

/// DAO 메서드가 불변 참조 수신자 `&self`로 시작하는지 검증한다.
fn validate_receiver(sig: &syn::Signature) -> syn::Result<()> {
    match sig.inputs.first() {
        Some(FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() =>
        {
            Ok(())
        }
        _ => Err(syn::Error::new(
            sig.inputs.span(),
            "DAO 메서드 수신자는 &self여야 합니다",
        )),
    }
}

/// SQL 토큰에 RETURNING 절 키워드가 있는지 판별한다.
fn sql_has_returning(sql: &str) -> bool {
    use sqlparser::ast::Statement;
    use sqlparser::dialect::SQLiteDialect;
    use sqlparser::parser::Parser;

    let Ok(statements) = Parser::parse_sql(&SQLiteDialect {}, sql) else {
        return false;
    };
    matches!(
        statements.as_slice(),
        [Statement::Update {
            returning: Some(_),
            ..
        }] | [Statement::Delete(sqlparser::ast::Delete {
            returning: Some(_),
            ..
        })]
    )
}

/// SQL 명명 파라미터 ↔ 메서드 인자 정합성 검증 (명세 §5.2).
/// 원인 span: 파라미터가 SQL 쪽에 있으면 SQL 리터럴, 인자 쪽이면 인자 식별자 (L-16)
fn check_params(sql: &LitStr, sig: &syn::Signature) -> syn::Result<Vec<String>> {
    let names = extract_named_params(&sql.value());
    let args = arg_idents(sig)?;
    let arg_names: Vec<String> = args.iter().map(|a| a.to_string()).collect();

    for n in &names {
        if !arg_names.contains(n) {
            return Err(syn::Error::new(
                sql.span(),
                format!("SQL 파라미터 :{n} 에 대응하는 메서드 인자가 없습니다"),
            ));
        }
    }
    for (a, ident) in arg_names.iter().zip(&args) {
        if !names.contains(a) {
            return Err(syn::Error::new(
                ident.span(),
                format!("메서드 인자 {a} 가 SQL에서 사용되지 않습니다 (:{a} 없음)"),
            ));
        }
    }
    Ok(names)
}

/// 메서드 속성 파싱 — query/insert/update/delete/transaction 중 정확히 하나 (M-17)
fn parse_method_kind(method: &syn::TraitItemFn) -> syn::Result<MethodKind> {
    let mut kind: Option<MethodKind> = None;
    for attr in &method.attrs {
        let path = attr.path();
        let new_kind = if path.is_ident("query") {
            let (sql, unchecked, with_relations) = parse_sql_args(attr)?;
            Some(MethodKind::Query {
                sql,
                unchecked,
                with_relations,
            })
        } else if path.is_ident("update") || path.is_ident("delete") {
            let (sql, unchecked, with_relations) = parse_sql_args(attr)?;
            if with_relations {
                return Err(syn::Error::new(
                    attr.span(),
                    "with_relations는 #[query]에서만 지원됩니다",
                ));
            }
            Some(MethodKind::Write { sql, unchecked })
        } else if path.is_ident("insert") {
            let mut on_conflict = None;
            let mut keep_pk = false;
            if !matches!(attr.meta, syn::Meta::Path(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("on_conflict") {
                        let lit: LitStr = meta.value()?.parse()?;
                        on_conflict = Some(lit.value());
                        Ok(())
                    } else if meta.path.is_ident("keep_pk") {
                        keep_pk = true;
                        Ok(())
                    } else {
                        Err(meta.error("알 수 없는 insert 인자 — on_conflict/keep_pk 만 지원"))
                    }
                })?;
            }
            Some(MethodKind::Insert {
                on_conflict,
                keep_pk,
            })
        } else if path.is_ident("transaction") {
            Some(MethodKind::Transaction)
        } else {
            None
        };
        if let Some(k) = new_kind {
            // 마지막 승자 침묵 방지 — 두 번째 SQL 속성은 즉시 에러 (M-17)
            if kind.is_some() {
                return Err(syn::Error::new(
                    attr.span(),
                    "SQL 속성이 2개 이상입니다 — 하나만 지정하세요",
                ));
            }
            kind = Some(k);
        }
    }
    kind.ok_or_else(|| {
        syn::Error::new(
            method.sig.span(),
            "DAO 메서드에는 #[query]/#[insert]/#[update]/#[delete] 중 하나가 필요합니다",
        )
    })
}

/// SQL 실행 메서드 본문 생성 (query/update/delete 공용)
fn sql_method_body(
    sql: &str,
    names: &[String],
    shape: &RetShape,
    sig: &syn::Signature,
) -> syn::Result<TokenStream> {
    // 명명 파라미터 바인딩 배열
    let binds: Vec<TokenStream> = names
        .iter()
        .map(|n| {
            let key = format!(":{n}");
            let ident = format_ident!("{n}");
            quote! { (#key, &#ident as &dyn ::roomrs::ToSql) }
        })
        .collect();
    let params = quote! {
        let __params: &[(&str, &dyn ::roomrs::ToSql)] = &[#(#binds),*];
    };

    let call = match shape {
        RetShape::Many(t) => quote! { self.cx.ctx_query_all::<#t, _>(#sql, __params) },
        RetShape::Optional(t) => quote! { self.cx.ctx_query_optional::<#t, _>(#sql, __params) },
        RetShape::One(t) => quote! { self.cx.ctx_query_one::<#t, _>(#sql, __params) },
        RetShape::Affected => quote! { self.cx.ctx_execute(#sql, __params) },
        RetShape::InsertId => {
            return Err(syn::Error::new(
                sig.span(),
                "i64 rowid 반환은 #[insert] 메서드에서만 지원됩니다",
            ));
        }
        RetShape::Live(..) => unreachable!("Live 형태는 live_method_body에서 처리"),
    };
    Ok(quote! { #params #call })
}

/// 관계 로딩 메서드 본문 (명세 결정 로그 7) — 부모 조회 + RelationView::load를
/// 자동 트랜잭션으로 래핑. is_async = 워커 위임 버전.
fn relation_method_body(
    sql: &str,
    names: &[String],
    shape: &RetShape,
    sig: &syn::Signature,
    is_async: bool,
) -> syn::Result<TokenStream> {
    // 뷰 타입 + 형태 어댑터
    let (view_ty, adapt) = match shape {
        RetShape::Many(t) => (t.clone(), quote! { Ok(__views) }),
        RetShape::Optional(t) => (
            t.clone(),
            quote! {
                Ok(if __views.is_empty() { None } else { Some(__views.remove(0)) })
            },
        ),
        RetShape::One(t) => (
            t.clone(),
            quote! {
                if __views.is_empty() {
                    Err(::roomrs::Error::NotFound)
                } else {
                    Ok(__views.remove(0))
                }
            },
        ),
        _ => {
            return Err(syn::Error::new(
                sig.span(),
                "with_relations는 Result<Vec<V>|Option<V>|V> 반환만 지원합니다",
            ));
        }
    };

    let tx_body = quote! {
        |__tx| {
            let __parents: Vec<<#view_ty as ::roomrs::RelationView>::Parent> =
                ::roomrs::SqlContext::ctx_query_all(__tx, #sql, __params)?;
            #[allow(unused_mut)]
            let mut __views = <#view_ty as ::roomrs::RelationView>::load(__tx, __parents)?;
            #adapt
        }
    };

    if !is_async {
        // 동기 — 빌린 명명 파라미터
        let binds: Vec<TokenStream> = names
            .iter()
            .map(|n| {
                let key = format!(":{n}");
                let ident = format_ident!("{n}");
                quote! { (#key, &#ident as &dyn ::roomrs::ToSql) }
            })
            .collect();
        Ok(quote! {
            let __params: &[(&str, &dyn ::roomrs::ToSql)] = &[#(#binds),*];
            ::roomrs::SqlContext::ctx_transaction(&self.cx, #tx_body)
        })
    } else {
        // 비동기 — 소유 Value 추출 후 워커에서 트랜잭션 실행
        let extracts: Vec<TokenStream> = names
            .iter()
            .map(|n| {
                let key = format!(":{n}");
                let ident = format_ident!("{n}");
                quote! { (#key, ::roomrs::to_owned_value(&#ident)?) }
            })
            .collect();
        Ok(quote! {
            let __prep: ::roomrs::Result<Vec<(&'static str, ::roomrs::rusqlite::types::Value)>> =
                (|| Ok(vec![#(#extracts),*]))();
            let __h = self.h.clone();
            async move {
                let __owned = __prep?;
                __h.run(move |__sh| {
                    let __ref_vec: Vec<(&str, &dyn ::roomrs::ToSql)> = __owned
                        .iter()
                        .map(|(k, v)| (*k, v as &dyn ::roomrs::ToSql))
                        .collect();
                    let __params: &[(&str, &dyn ::roomrs::ToSql)] = &__ref_vec;
                    ::roomrs::SqlContext::ctx_transaction(&__sh, #tx_body)
                })
                .await
            }
        })
    }
}

/// 라이브 쿼리 메서드 본문 (명세 §5.6) — 동기/비동기 공용(WatchContext), 수신자만 다름
fn live_method_body(
    sql: &str,
    names: &[String],
    shape: &RetShape,
    tables: &[String],
    receiver: TokenStream,
) -> syn::Result<TokenStream> {
    // 소유 명명 파라미터 추출 (재조회에 필요)
    let extracts: Vec<TokenStream> = names
        .iter()
        .map(|n| {
            let key = format!(":{n}");
            let ident = format_ident!("{n}");
            quote! { (#key.to_string(), ::roomrs::to_owned_value(&#ident)?) }
        })
        .collect();
    let table_lits: Vec<TokenStream> = tables.iter().map(|t| quote! { #t }).collect();

    let RetShape::Live(kind, t) = shape else {
        unreachable!("live_method_body는 Live 형태 전용");
    };
    let call = match kind {
        LiveShape::Many => quote! { ctx_watch_all_named::<#t> },
        LiveShape::Optional => quote! { ctx_watch_optional_named::<#t> },
        LiveShape::Scalar => quote! { ctx_watch_scalar_named::<#t> },
    };

    Ok(quote! {
        let __params: ::roomrs::Result<Vec<(String, ::roomrs::rusqlite::types::Value)>> =
            (|| Ok(vec![#(#extracts),*]))();
        ::roomrs::WatchContext::#call(&#receiver, #sql, __params, &[#(#table_lits),*])
    })
}

/// #[insert] 메서드 본문 생성 — 엔티티 타입은 유일한 참조 인자에서 추론
fn insert_method_body(
    on_conflict: &Option<String>,
    keep_pk: bool,
    sig: &syn::Signature,
) -> syn::Result<TokenStream> {
    // 인자: &self + 엔티티 참조 1개
    let mut args = sig.inputs.iter().skip(1);
    let (Some(FnArg::Typed(pt)), None) = (args.next(), args.next()) else {
        return Err(syn::Error::new(
            sig.span(),
            "#[insert] 메서드는 엔티티 참조 인자 1개만 받아야 합니다: fn add(&self, e: &T) -> Result<i64>",
        ));
    };
    let Pat::Ident(arg) = pt.pat.as_ref() else {
        return Err(syn::Error::new(
            pt.pat.span(),
            "#[insert] 인자는 단순 식별자여야 합니다",
        ));
    };
    let Type::Reference(r) = pt.ty.as_ref() else {
        return Err(syn::Error::new(
            pt.ty.span(),
            "#[insert] 인자는 &Entity 참조여야 합니다",
        ));
    };
    let entity_ty = r.elem.as_ref();
    let arg_ident = &arg.ident;

    let verb = insert_verb(on_conflict, sig)?;
    let (cols_const, ph_const, params_fn) = insert_consts(keep_pk);

    Ok(quote! {
        // SQL은 프로세스 수명 동안 1회 조립 (콘스트 조합은 런타임 포맷 필요).
        // 컬럼 0개(단일 autoincrement PK 엔티티) = DEFAULT VALUES (M-15)
        static __SQL: ::std::sync::LazyLock<String> = ::std::sync::LazyLock::new(|| {
            let __cols = <#entity_ty as ::roomrs::Insertable>::#cols_const;
            if __cols.is_empty() {
                format!(
                    "{} INTO \"{}\" DEFAULT VALUES",
                    #verb,
                    <#entity_ty as ::roomrs::Entity>::TABLE,
                )
            } else {
                format!(
                    "{} INTO \"{}\" ({}) VALUES ({})",
                    #verb,
                    <#entity_ty as ::roomrs::Entity>::TABLE,
                    __cols,
                    <#entity_ty as ::roomrs::Insertable>::#ph_const,
                )
            }
        });
        let __values = <#entity_ty as ::roomrs::Insertable>::#params_fn(#arg_ident)?;
        self.cx.ctx_insert(&__SQL, ::roomrs::params_from_iter(__values))
    })
}

/// 비동기 메서드 시그니처 — 출력을 `impl Future<Output = …> + Send`로 변환 (명세 B-3).
/// 제네릭·where 절을 그대로 보존한다 (M-20)
fn async_sig(sig: &syn::Signature) -> syn::Result<TokenStream> {
    let inner = unwrap_result(&sig.output)?;
    let ident = &sig.ident;
    let inputs = &sig.inputs;
    // Generics의 ToTokens는 `<…>` 파라미터만 방출 — where 절은 별도 방출
    let generics = &sig.generics;
    let where_clause = &sig.generics.where_clause;
    Ok(quote! {
        fn #ident #generics (#inputs) -> impl ::core::future::Future<Output = ::roomrs::Result<#inner>> + Send + 'static
        #where_clause
    })
}

/// 비동기 SQL 메서드 본문 — 소유 Value 추출(동기 단계) 후 워커 위임 (명세 §2.4)
fn async_sql_method_body(
    sql: &str,
    names: &[String],
    shape: &RetShape,
    sig: &syn::Signature,
) -> syn::Result<TokenStream> {
    // 동기 단계: 빌린 인자를 소유 Value로 변환
    let extracts: Vec<TokenStream> = names
        .iter()
        .map(|n| {
            let key = format!(":{n}");
            let ident = format_ident!("{n}");
            quote! { (#key, ::roomrs::to_owned_value(&#ident)?) }
        })
        .collect();

    let call = match shape {
        RetShape::Many(t) => {
            quote! { ::roomrs::SqlContext::ctx_query_all::<#t, _>(&__sh, #sql, __refs) }
        }
        RetShape::Optional(t) => {
            quote! { ::roomrs::SqlContext::ctx_query_optional::<#t, _>(&__sh, #sql, __refs) }
        }
        RetShape::One(t) => {
            quote! { ::roomrs::SqlContext::ctx_query_one::<#t, _>(&__sh, #sql, __refs) }
        }
        RetShape::Affected => quote! { ::roomrs::SqlContext::ctx_execute(&__sh, #sql, __refs) },
        RetShape::InsertId => {
            return Err(syn::Error::new(
                sig.span(),
                "i64 rowid 반환은 #[insert] 메서드에서만 지원됩니다",
            ));
        }
        RetShape::Live(..) => unreachable!("Live 형태는 live_method_body에서 처리"),
    };

    Ok(quote! {
        let __prep: ::roomrs::Result<Vec<(&'static str, ::roomrs::rusqlite::types::Value)>> =
            (|| Ok(vec![#(#extracts),*]))();
        let __h = self.h.clone();
        async move {
            let __owned = __prep?;
            __h.run(move |__sh| {
                let __ref_vec: Vec<(&str, &dyn ::roomrs::ToSql)> = __owned
                    .iter()
                    .map(|(k, v)| (*k, v as &dyn ::roomrs::ToSql))
                    .collect();
                let __refs: &[(&str, &dyn ::roomrs::ToSql)] = &__ref_vec;
                #call
            })
            .await
        }
    })
}

/// 비동기 #[insert] 본문 — insert_params를 소유 Value로 변환 후 워커 위임
fn async_insert_method_body(
    on_conflict: &Option<String>,
    keep_pk: bool,
    sig: &syn::Signature,
) -> syn::Result<TokenStream> {
    // 동기 본문 생성 로직과 동일한 검증·SQL 조립 재사용을 위해 인자 재파싱
    let mut args = sig.inputs.iter().skip(1);
    let (Some(FnArg::Typed(pt)), None) = (args.next(), args.next()) else {
        return Err(syn::Error::new(
            sig.span(),
            "#[insert] 메서드는 엔티티 참조 인자 1개만 받아야 합니다",
        ));
    };
    let Pat::Ident(arg) = pt.pat.as_ref() else {
        return Err(syn::Error::new(
            pt.pat.span(),
            "#[insert] 인자는 단순 식별자여야 합니다",
        ));
    };
    let Type::Reference(r) = pt.ty.as_ref() else {
        return Err(syn::Error::new(
            pt.ty.span(),
            "#[insert] 인자는 &Entity 참조여야 합니다",
        ));
    };
    let entity_ty = r.elem.as_ref();
    let arg_ident = &arg.ident;

    let verb = insert_verb(on_conflict, sig)?;
    let (cols_const, ph_const, params_fn) = insert_consts(keep_pk);

    Ok(quote! {
        // 컬럼 0개(단일 autoincrement PK 엔티티) = DEFAULT VALUES (M-15)
        static __SQL: ::std::sync::LazyLock<String> = ::std::sync::LazyLock::new(|| {
            let __cols = <#entity_ty as ::roomrs::Insertable>::#cols_const;
            if __cols.is_empty() {
                format!(
                    "{} INTO \"{}\" DEFAULT VALUES",
                    #verb,
                    <#entity_ty as ::roomrs::Entity>::TABLE,
                )
            } else {
                format!(
                    "{} INTO \"{}\" ({}) VALUES ({})",
                    #verb,
                    <#entity_ty as ::roomrs::Entity>::TABLE,
                    __cols,
                    <#entity_ty as ::roomrs::Insertable>::#ph_const,
                )
            }
        });
        // 동기 단계: 소유 Value 추출 (빌린 &Entity는 여기서 끝)
        let __prep: ::roomrs::Result<Vec<::roomrs::rusqlite::types::Value>> =
            <#entity_ty as ::roomrs::Insertable>::#params_fn(#arg_ident)
                .and_then(::roomrs::outputs_to_values);
        let __h = self.h.clone();
        async move {
            let __values = __prep?;
            __h.run(move |__sh| {
                ::roomrs::SqlContext::ctx_insert(&__sh, &__SQL, ::roomrs::params_from_iter(__values))
            })
            .await
        }
    })
}

/// INSERT 동사 결정 — on_conflict 허용 목록 검증
fn insert_verb(on_conflict: &Option<String>, sig: &syn::Signature) -> syn::Result<String> {
    Ok(match on_conflict.as_deref() {
        None => "INSERT".to_string(),
        Some("replace") => "INSERT OR REPLACE".to_string(),
        Some("ignore") => "INSERT OR IGNORE".to_string(),
        Some("abort") => "INSERT OR ABORT".to_string(),
        Some("rollback") => "INSERT OR ROLLBACK".to_string(),
        Some("fail") => "INSERT OR FAIL".to_string(),
        Some(other) => {
            return Err(syn::Error::new(
                sig.span(),
                format!("알 수 없는 on_conflict 값 '{other}' — replace/ignore/abort/rollback/fail"),
            ));
        }
    })
}

/// keep_pk에 따른 Insertable 상수·메서드 선택
fn insert_consts(keep_pk: bool) -> (TokenStream, TokenStream, TokenStream) {
    if keep_pk {
        (
            quote!(INSERT_COLUMNS_KEEP_PK),
            quote!(INSERT_PLACEHOLDERS_KEEP_PK),
            quote!(insert_params_keep_pk),
        )
    } else {
        (
            quote!(INSERT_COLUMNS),
            quote!(INSERT_PLACEHOLDERS),
            quote!(insert_params),
        )
    }
}

/// `#[dao]` 본체
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let item: ItemTrait = syn::parse2(input)?;
    let trait_ident = item.ident.clone();
    let vis = item.vis.clone();
    let on_ident = format_ident!("{}On", trait_ident);
    let async_trait_ident = format_ident!("{}Async", trait_ident);
    let async_on_ident = format_ident!("{}AsyncOn", trait_ident);

    let mut trait_methods: Vec<TokenStream> = Vec::new();
    let mut impl_methods: Vec<TokenStream> = Vec::new();
    let mut async_trait_methods: Vec<TokenStream> = Vec::new();
    let mut async_impl_methods: Vec<TokenStream> = Vec::new();
    // watch 메서드 존재 시 C: WatchContext 바운드 필요 (feature live 요구)
    let mut has_live = false;

    // 스냅샷 1회 로드 — 부재 = 정적 스키마 대조 스킵, 파손 = 하드 에러 (M-19).
    // db별 최신 버전 스냅샷 합집합으로 대조한다 (명세 §7.2, 결정 21)
    let snapshots = crate::schema::load_validation_snapshots()
        .map_err(|msg| syn::Error::new(trait_ident.span(), msg))?;

    for ti in &item.items {
        let TraitItem::Fn(method) = ti else {
            return Err(syn::Error::new(
                ti.span(),
                "#[dao] trait에는 메서드만 선언할 수 있습니다",
            ));
        };
        let kind = parse_method_kind(method)?;
        let sig = &method.sig;
        validate_receiver(sig)?;

        // 본문 규칙: #[transaction]만 본문 필수, 나머지는 본문 금지 (명세 §5.9)
        let is_tx = matches!(kind, MethodKind::Transaction);
        match (&method.default, is_tx) {
            (None, true) => {
                return Err(syn::Error::new(
                    sig.span(),
                    "#[transaction] 메서드에는 본문이 필요합니다 — 본문이 트랜잭션 안에서 실행됩니다",
                ));
            }
            (Some(_), false) => {
                return Err(syn::Error::new(
                    sig.span(),
                    "#[dao] 메서드에 기본 구현을 둘 수 없습니다 — 매크로가 구현을 생성합니다 (#[transaction] 제외)",
                ));
            }
            _ => {}
        }

        let is_insert = matches!(kind, MethodKind::Insert { .. });
        let shape = classify_return(&sig.output, is_insert)?;
        if !is_tx && matches!(&shape, RetShape::One(Type::Tuple(tuple)) if tuple.elems.is_empty()) {
            return Err(syn::Error::new(
                sig.output.span(),
                "DAO 메서드는 Result<()>를 반환할 수 없습니다 — write 영향 행 수는 Result<u64>를 사용하세요",
            ));
        }
        if matches!(kind, MethodKind::Insert { on_conflict: Some(ref value), .. } if value == "ignore")
            && matches!(shape, RetShape::InsertId)
        {
            return Err(syn::Error::new(
                sig.output.span(),
                "on_conflict = \"ignore\"는 0행일 수 있어 Result<i64> rowid 반환을 지원하지 않습니다",
            ));
        }
        if let MethodKind::Write { sql, .. } = &kind {
            if !matches!(shape, RetShape::Affected) && !sql_has_returning(&sql.value()) {
                return Err(syn::Error::new(
                    sig.output.span(),
                    "#[update]/#[delete]의 비-u64 반환에는 SQL RETURNING 절이 필요합니다",
                ));
            }
        }

        // 문서·조건부 컴파일·린트 속성은 생성 trait 선언과 impl 양쪽에 보존 (M-20)
        let kept_attrs: Vec<&syn::Attribute> = method
            .attrs
            .iter()
            .filter(|a| {
                let p = a.path();
                p.is_ident("doc") || p.is_ident("cfg") || p.is_ident("allow")
            })
            .collect();

        // 라이브 쿼리 메서드 (명세 §5.6) — 동기/비동기 모두 LiveQuery를 직접 반환
        if let RetShape::Live(..) = &shape {
            let MethodKind::Query { sql, unchecked, .. } = &kind else {
                return Err(syn::Error::new(
                    sig.span(),
                    "LiveQuery 반환은 #[query] 메서드에서만 지원됩니다",
                ));
            };
            let sql_str = sql.value();
            let names = check_params(sql, sig)?;
            if !unchecked && !snapshots.is_empty() {
                if let Some(msg) = crate::schema::validate_sql(&sql_str, &snapshots) {
                    return Err(syn::Error::new(sql.span(), msg));
                }
            }
            has_live = true;
            // 의존 추출 실패(None) = 빈 슬라이스 — core가 힌트 없음으로 간주해
            // 자체 추출을 시도하고, 그마저 실패하면 UnknownDependencies 경로로
            // 라우팅한다(.watching()으로 해소 가능, H-9 계약 유지)
            let tables = crate::schema::depends_on(&sql_str).unwrap_or_default();
            let body = live_method_body(&sql_str, &names, &shape, &tables, quote!(self.cx))?;
            let abody = live_method_body(&sql_str, &names, &shape, &tables, quote!(self.h))?;

            trait_methods.push(quote! { #(#kept_attrs)* #sig; });
            impl_methods.push(quote! { #(#kept_attrs)* #sig { #body } });
            // 비동기 trait도 동일 시그니처 (LiveQuery 직접 반환 — Future 아님)
            async_trait_methods.push(quote! { #(#kept_attrs)* #sig; });
            async_impl_methods.push(quote! { #(#kept_attrs)* #sig { #abody } });
            continue;
        }

        let (body, async_body) = match &kind {
            MethodKind::Query {
                sql,
                unchecked,
                with_relations,
            } => {
                let sql_str = sql.value();
                let names = check_params(sql, sig)?;
                if !unchecked && !snapshots.is_empty() {
                    if let Some(msg) = crate::schema::validate_sql(&sql_str, &snapshots) {
                        return Err(syn::Error::new(sql.span(), msg));
                    }
                }
                // #[query] SELECT + u64/usize = 영향 행 수 오분류 — 런타임
                // ExecuteReturnedResults 대신 컴파일 에러 (M-12)
                if matches!(shape, RetShape::Affected) && !sql_is_write(&sql_str) {
                    return Err(syn::Error::new(
                        sig.output.span(),
                        "#[query] SELECT 는 u64/usize 반환을 지원하지 않습니다 — i64 를 사용하세요 (영향 행 수 반환은 INSERT/UPDATE/DELETE SQL 전용)",
                    ));
                }
                if *with_relations {
                    (
                        relation_method_body(&sql_str, &names, &shape, sig, false)?,
                        relation_method_body(&sql_str, &names, &shape, sig, true)?,
                    )
                } else {
                    (
                        sql_method_body(&sql_str, &names, &shape, sig)?,
                        async_sql_method_body(&sql_str, &names, &shape, sig)?,
                    )
                }
            }
            MethodKind::Write { sql, unchecked } => {
                // #[query]는 read/write 라우팅이 SQL 첫 키워드로 결정되고(core),
                // #[update]/#[delete]는 Affected 반환이 자연스럽다 — 형태는 반환 타입이 결정
                let sql_str = sql.value();
                let names = check_params(sql, sig)?;

                // 정적 스키마 대조 (명세 §7.2) — unchecked 해치·스냅샷 부재·파싱 실패 = 스킵
                if !unchecked && !snapshots.is_empty() {
                    if let Some(msg) = crate::schema::validate_sql(&sql_str, &snapshots) {
                        return Err(syn::Error::new(sql.span(), msg));
                    }
                }

                (
                    sql_method_body(&sql_str, &names, &shape, sig)?,
                    async_sql_method_body(&sql_str, &names, &shape, sig)?,
                )
            }
            MethodKind::Insert {
                on_conflict,
                keep_pk,
            } => {
                if !matches!(shape, RetShape::InsertId) {
                    return Err(syn::Error::new(
                        sig.span(),
                        "#[insert] 메서드는 Result<i64>(새 rowid)를 반환해야 합니다 (명세 §12c)",
                    ));
                }
                (
                    insert_method_body(on_conflict, *keep_pk, sig)?,
                    async_insert_method_body(on_conflict, *keep_pk, sig)?,
                )
            }
            MethodKind::Transaction => {
                // 본문 재작성: self → tx-바운드 DAO (명세 §5.9 메커니즘 2)
                let mut body = method.default.clone().expect("위에서 본문 존재 검증");
                let mut visitor = ReplaceSelf { errors: Vec::new() };
                syn::visit_mut::VisitMut::visit_block_mut(&mut visitor, &mut body);
                // 매크로 토큰 안의 self = 재작성 불가 — 하드 에러 (H-8)
                let mut errs = visitor.errors.into_iter();
                if let Some(mut e) = errs.next() {
                    for extra in errs {
                        e.combine(extra);
                    }
                    return Err(e);
                }

                let sync_body = quote! {
                    ::roomrs::SqlContext::ctx_transaction(&self.cx, |__tx| {
                        let __tx_dao = #on_ident::__new(__tx);
                        #body
                    })
                };
                // 비동기: 본문 전체가 워커에서 원자 실행 (명세 §5.5 동기 클로저형).
                // 빌린 인자는 'static 제약으로 컴파일 에러 — 소유 인자만 지원(문서화)
                let async_body = quote! {
                    let __h = self.h.clone();
                    async move {
                        __h.run(move |__sh| {
                            ::roomrs::SqlContext::ctx_transaction(&__sh, |__tx| {
                                let __tx_dao = #on_ident::__new(__tx);
                                #body
                            })
                        })
                        .await
                    }
                };
                (sync_body, async_body)
            }
        };

        // 동기: trait 선언(SQL 속성 제거, doc/cfg/allow 보존) + impl (M-20)
        trait_methods.push(quote! { #(#kept_attrs)* #sig; });
        impl_methods.push(quote! {
            #(#kept_attrs)*
            #sig {
                #body
            }
        });

        // 비동기: 동일 메서드명, impl Future + Send (명세 §2.4, B-3)
        let asig = async_sig(sig)?;
        async_trait_methods.push(quote! { #(#kept_attrs)* #asig; });
        async_impl_methods.push(quote! {
            #(#kept_attrs)*
            #asig {
                #async_body
            }
        });
    }

    let trait_docs: Vec<&syn::Attribute> = item
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .collect();

    // watch 메서드 존재 시 WatchContext 바운드 (feature live 필요 — 기본 on)
    let maybe_watch_bound = if has_live {
        quote! { + ::roomrs::WatchContext }
    } else {
        quote! {}
    };

    Ok(quote! {
        #(#trait_docs)*
        #vis trait #trait_ident {
            #(#trait_methods)*
        }

        /// #[dao] 생성 구현체 — 풀-바운드(SyncHandle)/tx-바운드(&Tx) 공용 (명세 §5.9)
        #vis struct #on_ident<C> {
            cx: C,
        }

        impl<C> #on_ident<C> {
            /// 내부 생성자 — #[database] 생성 코드 전용
            #[doc(hidden)]
            pub fn __new(cx: C) -> Self {
                Self { cx }
            }
        }

        impl<C: ::roomrs::SqlContext #maybe_watch_bound> #trait_ident for #on_ident<C> {
            #(#impl_methods)*
        }

        ::roomrs::__if_async! {
            /// #[dao] 생성 비동기 trait — 동일 메서드명, `+ Send` Future (명세 §2.4)
            #vis trait #async_trait_ident {
                #(#async_trait_methods)*
            }

            /// #[dao] 생성 비동기 구현체 — AsyncHandle 바운드
            #vis struct #async_on_ident {
                h: ::roomrs::AsyncHandle,
            }

            impl #async_on_ident {
                /// 내부 생성자 — #[database] 생성 코드 전용
                #[doc(hidden)]
                pub fn __new(h: ::roomrs::AsyncHandle) -> Self {
                    Self { h }
                }
            }

            impl #async_trait_ident for #async_on_ident {
                #(#async_impl_methods)*
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    /// write 분류 — 첫 키워드 DML + WITH 선두 CTE-DML (L-17)
    #[test]
    fn sql_is_write_with_cte_dml() {
        assert!(sql_is_write("UPDATE t SET a = 1"));
        assert!(sql_is_write("insert into t values (1)"));
        assert!(!sql_is_write("SELECT 1"));
        // CTE 뒤 최상위 DML = write (L-17)
        assert!(sql_is_write(
            "WITH x AS (SELECT 1) DELETE FROM t WHERE id IN (SELECT * FROM x)"
        ));
        assert!(sql_is_write("with x as (select 1) update t set a = 1"));
        // CTE 뒤 SELECT = read — 본문 괄호 안 SELECT/문자열은 무시
        assert!(!sql_is_write("WITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(!sql_is_write(
            "WITH x AS (SELECT 'delete') SELECT * FROM x -- update 주석"
        ));
        // 인용 식별자 안의 DML 단어 = 무시
        assert!(!sql_is_write(
            r#"WITH x AS (SELECT 1) SELECT "delete" FROM x"#
        ));
    }

    /// 리터럴 `{self}` 포맷 캡처 탐지 — `{{self}}` 이스케이프 제외 (L-18)
    #[test]
    fn literal_self_capture_detection() {
        assert!(literal_captures_self("\"{self}\""));
        assert!(literal_captures_self("\"id={self:?}\""));
        assert!(literal_captures_self("r\"{self}\""));
        assert!(!literal_captures_self("\"{{self}}\""));
        assert!(!literal_captures_self("\"{ self }\"")); // 포맷 스펙상 공백 불허 = 캡처 아님
        assert!(!literal_captures_self("\"{selfx}\""));
        assert!(!literal_captures_self("\"self\""));
    }

    /// find_self_in_tokens — Ident·리터럴 캡처 양쪽 탐지 (H-8/L-18)
    #[test]
    fn find_self_covers_ident_and_literal() {
        assert!(find_self_in_tokens(quote! { self.find(1) }).is_some());
        assert!(find_self_in_tokens(quote! { ("{self}") }).is_some());
        assert!(find_self_in_tokens(quote! { ("{{self}}") }).is_none());
        assert!(find_self_in_tokens(quote! { other.find(1) }).is_none());
    }
}
