//! `#[derive(Relation)]` — 관계 뷰 조립 코드 생성 (명세 결정 로그 7)
//!
//! ```ignore
//! #[derive(Relation)]
//! struct UserWithPosts {
//!     #[embedded]
//!     user: User,
//!     #[relation(entity = Post, parent_key = "id", entity_key = "user_id")]
//!     posts: Vec<Post>,
//! }
//! ```
//!
//! `parent_key`/`entity_key` 는 러스트 **필드명** — 유효한 식별자여야 한다 (M-13).
//! `#[column(name = "…")]` 로 SQL 컬럼명이 필드명과 다른 엔티티는
//! `entity_column = "…"` 로 자식 테이블의 SQL 컬럼명을 따로 지정한다
//! (기본값 = entity_key, M-14). parent_key 는 SQL 에 렌더되지 않으므로
//! 대응 옵션이 없다.

use crate::util::validate_sql_identifier;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, LitStr, Type};

/// 관계 필드 메타
struct RelationField {
    ident: syn::Ident,
    /// Vec<Child> = 다건, Option<Child> = 0~1건
    many: bool,
    child_ty: Type,
    /// 부모 러스트 필드명 (SQL 미사용, M-14)
    parent_key: syn::Ident,
    /// 자식 러스트 필드명
    entity_key: syn::Ident,
    /// 자식 SQL 컬럼명 — 기본값 = entity_key (M-14)
    entity_column: String,
    junction: Option<JunctionMeta>,
}

/// N:M 정션 메타
struct JunctionMeta {
    table: String,
    parent_key: String,
    entity_key: String,
}

/// 키 문자열 → 러스트 식별자 — 무효 시 원인 속성 span에 한국어 에러 (M-13)
fn key_ident(s: &str, what: &str, attr: &syn::Attribute) -> syn::Result<syn::Ident> {
    syn::parse_str::<syn::Ident>(s).map_err(|_| {
        syn::Error::new(
            attr.span(),
            format!(
                "{what} \"{s}\" 는 유효한 러스트 식별자여야 합니다 — SQL 컬럼명이 필드명과 다르면 entity_column 을 사용하세요"
            ),
        )
    })
}

/// 필드 타입에서 Vec/Option 내부 추출
fn unwrap_container(ty: &Type) -> Option<(bool, Type)> {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                if let Some(syn::GenericArgument::Type(inner)) = ab.args.first() {
                    if seg.ident == "Vec" {
                        return Some((true, inner.clone()));
                    }
                    if seg.ident == "Option" {
                        return Some((false, inner.clone()));
                    }
                }
            }
        }
    }
    None
}

/// derive 본체
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let ident = input.ident.clone();

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "#[derive(Relation)]은 구조체 전용입니다",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new(
            input.span(),
            "named 필드 구조체만 지원합니다",
        ));
    };

    let mut embedded: Option<(syn::Ident, Type)> = None;
    let mut relations: Vec<RelationField> = Vec::new();

    for field in &fields.named {
        let fident = field.ident.clone().expect("named 필드 보장");
        let mut is_embedded = false;
        let mut rel: Option<RelationField> = None;

        for attr in &field.attrs {
            if attr.path().is_ident("embedded") {
                is_embedded = true;
            } else if attr.path().is_ident("relation") {
                let Some((many, child_ty)) = unwrap_container(&field.ty) else {
                    return Err(syn::Error::new(
                        field.ty.span(),
                        "관계 필드는 Vec<Child>(1:N/N:M) 또는 Option<Child>(1:1)여야 합니다",
                    ));
                };
                let mut parent_key = None;
                let mut entity_key = None;
                let mut entity_column = None;
                let mut junction_table = None;
                let mut j_parent = None;
                let mut j_entity = None;
                attr.parse_nested_meta(|meta| {
                    let get = |m: &syn::meta::ParseNestedMeta| -> syn::Result<String> {
                        let lit = m.value()?.parse::<LitStr>()?;
                        validate_sql_identifier(&lit.value(), lit.span())?;
                        Ok(lit.value())
                    };
                    if meta.path.is_ident("entity") {
                        // 타입은 필드 타입에서 추론 — 인자는 가독용, 소비만
                        let _: syn::Path = meta.value()?.parse()?;
                        Ok(())
                    } else if meta.path.is_ident("parent_key") {
                        parent_key = Some(get(&meta)?);
                        Ok(())
                    } else if meta.path.is_ident("entity_key") {
                        entity_key = Some(get(&meta)?);
                        Ok(())
                    } else if meta.path.is_ident("entity_column") {
                        entity_column = Some(get(&meta)?);
                        Ok(())
                    } else if meta.path.is_ident("junction") {
                        junction_table = Some(get(&meta)?);
                        Ok(())
                    } else if meta.path.is_ident("junction_parent_key") {
                        j_parent = Some(get(&meta)?);
                        Ok(())
                    } else if meta.path.is_ident("junction_entity_key") {
                        j_entity = Some(get(&meta)?);
                        Ok(())
                    } else {
                        Err(meta.error(
                            "알 수 없는 relation 인자 — entity/parent_key/entity_key/entity_column/junction/junction_parent_key/junction_entity_key",
                        ))
                    }
                })?;

                let parent_key = parent_key.ok_or_else(|| {
                    syn::Error::new(attr.span(), "parent_key = \"…\" 가 필요합니다")
                })?;
                let entity_key = entity_key.ok_or_else(|| {
                    syn::Error::new(attr.span(), "entity_key = \"…\" 가 필요합니다")
                })?;
                // SQL 컬럼명 기본값 = entity_key — #[column(name)] 사용 엔티티만
                // entity_column 으로 분리 지정 (M-14)
                let entity_column = entity_column.unwrap_or_else(|| entity_key.clone());
                // 키는 러스트 필드 접근에 쓰이므로 유효 식별자 강제 (M-13 —
                // format_ident! panic 방지)
                let parent_key = key_ident(&parent_key, "parent_key", attr)?;
                let entity_key = key_ident(&entity_key, "entity_key", attr)?;
                let junction = match (junction_table, j_parent, j_entity) {
                    (None, None, None) => None,
                    (Some(t), Some(p), Some(e)) => Some(JunctionMeta {
                        table: t,
                        parent_key: p,
                        entity_key: e,
                    }),
                    _ => {
                        return Err(syn::Error::new(
                            attr.span(),
                            "N:M은 junction/junction_parent_key/junction_entity_key 세 개가 모두 필요합니다",
                        ));
                    }
                };
                rel = Some(RelationField {
                    ident: fident.clone(),
                    many,
                    child_ty,
                    parent_key,
                    entity_key,
                    entity_column,
                    junction,
                });
            }
        }

        if is_embedded && rel.is_some() {
            return Err(syn::Error::new(
                fident.span(),
                "한 필드에 #[embedded]와 #[relation]을 함께 사용할 수 없습니다",
            ));
        }
        if is_embedded {
            if embedded.is_some() {
                return Err(syn::Error::new(
                    fident.span(),
                    "#[embedded] 필드는 1개만 허용됩니다",
                ));
            }
            embedded = Some((fident, field.ty.clone()));
        } else if let Some(r) = rel {
            relations.push(r);
        } else {
            return Err(syn::Error::new(
                fident.span(),
                "관계 뷰의 모든 필드는 #[embedded] 또는 #[relation(...)]이어야 합니다",
            ));
        }
    }

    let Some((parent_ident, parent_ty)) = embedded else {
        return Err(syn::Error::new(
            input.span(),
            "#[embedded] 부모 필드가 필요합니다",
        ));
    };

    // 관계별 로딩 코드 — 부모 키 수집 → IN 조회 → 그룹핑 맵
    let mut loaders: Vec<TokenStream> = Vec::new();
    let mut assemblers: Vec<TokenStream> = Vec::new();

    for (i, r) in relations.iter().enumerate() {
        let map_ident = format_ident!("__rel_map_{i}");
        let fident = &r.ident;
        let child_ty = &r.child_ty;
        // 필드 접근 = *_key (러스트 식별자), SQL = entity_column (M-14)
        let pk = &r.parent_key;
        let ek = &r.entity_key;
        let ek_col = &r.entity_column;

        let loader = match &r.junction {
            // 1:N / 1:1 — core 헬퍼 (클로저가 키 타입 고정)
            None => quote! {
                #[allow(unused_mut)]
                let mut #map_ident = ::roomrs::load_children(
                    cx,
                    &__keys,
                    <#child_ty as ::roomrs::Entity>::TABLE,
                    #ek_col,
                    |__c: &#child_ty| __c.#ek.clone(),
                )?;
            },
            // N:M — core 헬퍼 (정션 2쿼리)
            Some(j) => {
                let jt = &j.table;
                let jpk = &j.parent_key;
                let jek = &j.entity_key;
                quote! {
                    #[allow(unused_mut)]
                    let mut #map_ident = ::roomrs::load_junction(
                        cx,
                        &__keys,
                        #jt,
                        #jpk,
                        #jek,
                        <#child_ty as ::roomrs::Entity>::TABLE,
                        #ek_col,
                        |__c: &#child_ty| __c.#ek.clone(),
                    )?;
                }
            }
        };
        loaders.push(loader);

        let assemble = if r.many {
            quote! { #fident: #map_ident.remove(&__p.#pk).unwrap_or_default() }
        } else {
            quote! {
                #fident: #map_ident
                    .remove(&__p.#pk)
                    .and_then(|mut v| if v.is_empty() { None } else { Some(v.remove(0)) })
            }
        };
        assemblers.push(assemble);
    }

    // 부모 키 수집은 첫 관계의 parent_key 기준이 아니라 관계별로 다를 수 있으나,
    // v1은 모든 관계가 같은 parent_key를 쓰는 일반형을 지원 — 서로 다르면 각자 수집 필요.
    // 단순화: 관계별 키 수집을 각 로더 위에서 수행하도록 첫 관계 키만 공유하는 대신
    // 아래처럼 관계마다 같은 __keys를 재사용하되, parent_key가 전 관계 동일함을 검증.
    if let Some(first) = relations.first() {
        if relations.iter().any(|r| r.parent_key != first.parent_key) {
            return Err(syn::Error::new(
                ident.span(),
                "v1 제약: 모든 #[relation]의 parent_key는 동일해야 합니다",
            ));
        }
    }
    // parent_key 는 이미 검증된 Ident (M-13) — format_ident! 불필요
    let parent_key_ident = relations
        .first()
        .map(|r| r.parent_key.clone())
        .unwrap_or_else(|| format_ident!("id"));

    Ok(quote! {
        impl ::roomrs::RelationView for #ident {
            type Parent = #parent_ty;

            /// 부모 목록 → 자식 IN 일괄 조회 → 조립 (#[derive(Relation)] 생성)
            fn load<C: ::roomrs::SqlContext>(
                cx: &C,
                parents: Vec<Self::Parent>,
            ) -> ::roomrs::Result<Vec<Self>> {
                let __keys: Vec<_> = parents.iter().map(|p| p.#parent_key_ident.clone()).collect();
                #(#loaders)*
                Ok(parents
                    .into_iter()
                    .map(|__p| Self {
                        #(#assemblers,)*
                        #parent_ident: __p,
                    })
                    .collect())
            }
        }
    })
}
