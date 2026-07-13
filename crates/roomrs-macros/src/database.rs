//! `#[database]` 전개 (명세 §5.4)
//!
//! 생성물:
//!   - `struct Db { inner: ::roomrs::Database }` (유닛 구조체 재작성)
//!   - `impl DatabaseSpec` (버전·DDL 수집)
//!   - `builder()` / `run_sync()`
//!   - `DbSync<'a>` — SyncHandle Deref + DAO 접근자
//!   - `DbTxDaos` — Tx용 DAO 접근자 확장 trait

use crate::util::to_snake_case;
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, Path};

/// `#[database(...)]` 인자
struct DatabaseArgs {
    entities: Vec<Path>,
    daos: Vec<Path>,
    version: u32,
}

/// 인자 파싱 — entities(...), daos(...), version = N
fn parse_args(args: TokenStream, span: proc_macro2::Span) -> syn::Result<DatabaseArgs> {
    let mut entities = Vec::new();
    let mut daos = Vec::new();
    let mut version: Option<u32> = None;

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("entities") {
            meta.parse_nested_meta(|inner| {
                entities.push(inner.path.clone());
                Ok(())
            })
        } else if meta.path.is_ident("daos") {
            meta.parse_nested_meta(|inner| {
                daos.push(inner.path.clone());
                Ok(())
            })
        } else if meta.path.is_ident("version") {
            let lit: syn::LitInt = meta.value()?.parse()?;
            version = Some(lit.base10_parse()?);
            Ok(())
        } else {
            Err(meta.error("알 수 없는 database 인자 — entities/daos/version 만 지원"))
        }
    });
    parser.parse2(args)?;

    if entities.is_empty() {
        return Err(syn::Error::new(
            span,
            "entities(...)에 엔티티를 1개 이상 지정해야 합니다",
        ));
    }
    let entity_keys: Vec<String> = entities
        .iter()
        .map(|entity| entity.to_token_stream().to_string())
        .collect();
    for (index, entity) in entities.iter().enumerate() {
        if entity_keys[..index].contains(&entity_keys[index]) {
            return Err(syn::Error::new(
                entity.span(),
                "entities(...)에 같은 엔티티를 중복 지정할 수 없습니다",
            ));
        }
    }
    let Some(version) = version else {
        return Err(syn::Error::new(span, "version = N 이 필요합니다"));
    };
    if version == 0 {
        return Err(syn::Error::new(
            span,
            "version은 1 이상이어야 합니다 (0 = 신규 DB 마커)",
        ));
    }
    Ok(DatabaseArgs {
        entities,
        daos,
        version,
    })
}

/// 스냅샷 스캔 결과 — 현재 버전 해시 · 압축 임베드 · 파일 의존성 토큰
struct SnapshotMeta {
    /// `SNAPSHOT_HASH` 초기화 토큰 — 현재 버전 파일 부재 = `None`
    snapshot_hash: TokenStream,
    /// `EMBEDDED_SCHEMAS` 엔트리들 (버전 오름차순)
    embedded_entries: Vec<TokenStream>,
    /// `include_bytes!` 의존성 등록 상수들 (리뷰 C-1)
    dep_consts: Vec<TokenStream>,
    /// 전개 시점에 현재 버전 스냅샷 파일이 존재했는지 — export 테스트의
    /// fail-open 창 차단용 `SNAPSHOT_FILE_SEEN` 상수 값 (결정 28, D-3b)
    file_seen: bool,
}

/// 스키마 디렉토리에서 `{db}.{N}.json` 전 버전을 스캔한다 (명세 §7.2/§8.4).
/// 파손 파일 = 하드 에러(부재와 구분, M-19), 버전 > database version = 에러,
/// 각 파일은 include_bytes 의존성 등록 + miniz_oxide 압축 임베드 (결정 21c)
fn scan_snapshots(
    db_snake: &str,
    version: u32,
    span: proc_macro2::Span,
) -> syn::Result<SnapshotMeta> {
    let mut meta = SnapshotMeta {
        snapshot_hash: quote! { None },
        embedded_entries: Vec::new(),
        dep_consts: Vec::new(),
        file_seen: false,
    };
    // CARGO_MANIFEST_DIR 부재 = 침묵 빈 경로 진행 대신 하드 에러 —
    // migrations_dir! 과 동일 정책 (L-13)
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        syn::Error::new(
            span,
            "CARGO_MANIFEST_DIR 없음 — #[database]는 cargo 빌드에서만 사용할 수 있습니다",
        )
    })?;
    let dir = roomrs_migrate::resolve_schema_dir(&manifest);
    let files = roomrs_migrate::list_snapshot_versions(&dir, db_snake).map_err(|e| {
        syn::Error::new(
            span,
            format!("스냅샷 디렉토리 읽기 실패: {} — {e}", dir.display()),
        )
    })?;
    for (ver, path) in &files {
        // 버전 단조성 — database version을 넘는 스냅샷은 정의 오류
        if *ver > version {
            return Err(syn::Error::new(
                span,
                format!(
                    "스냅샷 버전이 database version보다 큽니다: {} (version = {version})",
                    path.display()
                ),
            ));
        }
        let raw = std::fs::read(path).map_err(|e| {
            syn::Error::new(
                span,
                format!("스냅샷 파일 읽기 실패: {} — {e}", path.display()),
            )
        })?;
        // 존재하는데 파손 = 컴파일 하드 에러 — 부재(스킵)와 구분 (M-19)
        let snap = roomrs_migrate::SchemaSnapshot::from_slice(&raw).map_err(|e| {
            syn::Error::new(
                span,
                format!(
                    "스냅샷 파일 파손: {} — 파스 실패: {e} (명세 §7.4)",
                    path.display()
                ),
            )
        })?;
        if snap.version != *ver {
            return Err(syn::Error::new(
                span,
                format!(
                    "스냅샷 내부 version({})이 파일명 버전({ver})과 다릅니다: {}",
                    snap.version,
                    path.display()
                ),
            ));
        }
        // 현재 버전 파일 = 런타임 스테일 검증용 해시 임베드 (명세 §7.4b).
        // 존재 자체도 기록 — export 테스트의 fail-open 창 차단 (D-3b)
        if *ver == version {
            let h = snap.hash();
            meta.snapshot_hash = quote! { Some(#h) };
            meta.file_seen = true;
        }
        // include_bytes 의존성 등록 (리뷰 C-1) — 기존 파일 **갱신** = 재전개 보장.
        // 경로는 resolve_schema_dir 절대화 경로 기반 — fs::read 와 동일 파일 (M-8).
        // 한계: **신규** 파일 추가는 등록 자체가 불가(디렉토리 의존성 미지원) —
        // export 테스트가 생성 시에도 실패해 재빌드를 강제한다 (결정 28)
        let path_str = path.to_string_lossy().replace('\\', "/");
        meta.dep_consts
            .push(quote! { const _: &[u8] = ::core::include_bytes!(#path_str); });
        // 압축 바이트 임베드 (결정 21c) — 커밋된 전 버전을 누적 임베드하므로
        // 바이너리가 버전 수에 단조 증가한다. 절삭("최근 K개+갭") 정책은 후속
        // 검토 — #[database] rustdoc에 명시 (L-16)
        let compressed = roomrs_migrate::compress_snapshot(&raw);
        let bytes = proc_macro2::Literal::byte_string(&compressed);
        let v = *ver;
        meta.embedded_entries
            .push(quote! { ::roomrs::EmbeddedSchema { version: #v, compressed: #bytes } });
    }
    Ok(meta)
}

/// `#[database]` 본체
pub fn expand(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let item: ItemStruct = syn::parse2(input)?;
    let args = parse_args(args, item.span())?;

    if !matches!(item.fields, Fields::Unit) {
        return Err(syn::Error::new(
            item.span(),
            "#[database]는 유닛 구조체에만 사용할 수 있습니다: struct AppDb;",
        ));
    }

    let db_ident = item.ident.clone();
    let vis = item.vis.clone();
    let attrs: Vec<&syn::Attribute> = item.attrs.iter().collect();
    let sync_ident = format_ident!("{}Sync", db_ident);
    let tx_ext_ident = format_ident!("{}TxDaos", db_ident);
    let version = args.version;
    let entities = &args.entities;

    let async_ident = format_ident!("{}Async", db_ident);

    // db이름 = 구조체명 snake_case (명세 §7.2, 결정 21) — 스냅샷 파일명 프리픽스
    let db_snake = to_snake_case(&db_ident.to_string());

    // 스냅샷 파일 스캔 + 해시/압축 임베드 (명세 §7.2/§8.4, 결정 21b/21c)
    let snap_meta = scan_snapshots(&db_snake, version, item.span())?;
    let snapshot_hash = snap_meta.snapshot_hash;
    let embedded_entries = snap_meta.embedded_entries;
    let dep_consts = snap_meta.dep_consts;
    let file_seen = snap_meta.file_seen;

    // export 테스트 (명세 §7.4, 결정 21b) — cargo test 시 현재 버전 스냅샷
    // 생성/스테일 검증. 항상 방출 — 최초 `cargo test`가 초기 스냅샷을 만든다.
    let export_fn = format_ident!("__roomrs_schema_export_{}", db_snake);

    // DAO 접근자 — TodoDao → fn todo_dao()
    let mut sync_accessors: Vec<TokenStream> = Vec::new();
    let mut async_accessors: Vec<TokenStream> = Vec::new();
    let mut tx_decls: Vec<TokenStream> = Vec::new();
    let mut tx_impls: Vec<TokenStream> = Vec::new();
    for dao in &args.daos {
        let dao_name = dao
            .segments
            .last()
            .ok_or_else(|| syn::Error::new(dao.span(), "빈 DAO 경로"))?
            .ident
            .clone();
        let method = format_ident!("{}", to_snake_case(&dao_name.to_string()));
        let on_ident = {
            // 경로 마지막 세그먼트를 XxxOn으로 치환
            let mut p = dao.clone();
            let last = p.segments.last_mut().expect("위에서 검증");
            last.ident = format_ident!("{}On", last.ident);
            p
        };

        let async_on_ident = {
            let mut p = dao.clone();
            let last = p.segments.last_mut().expect("위에서 검증");
            last.ident = format_ident!("{}AsyncOn", last.ident);
            p
        };

        sync_accessors.push(quote! {
            /// DAO 접근자 — 풀-바운드 (#[database] 생성)
            #vis fn #method(&self) -> #on_ident<::roomrs::SyncHandle<'_>> {
                #on_ident::__new(self.h)
            }
        });
        async_accessors.push(quote! {
            /// DAO 접근자 — 비동기 (#[database] 생성)
            #vis fn #method(&self) -> #async_on_ident {
                #async_on_ident::__new(self.h.clone())
            }
        });
        tx_decls.push(quote! {
            /// DAO 접근자 — tx-바운드 (#[database] 생성, 명세 §5.9)
            fn #method(&self) -> #on_ident<&Self>;
        });
        tx_impls.push(quote! {
            fn #method(&self) -> #on_ident<&Self> {
                #on_ident::__new(self)
            }
        });
    }

    Ok(quote! {
        #(#attrs)*
        #vis struct #db_ident {
            inner: ::roomrs::Database,
        }

        // 스냅샷 파일 의존성 등록 (리뷰 C-1) — 파일 갱신 = 매크로 재전개 보장.
        // 사장 상수는 링커가 제거한다 (명세 §8.4)
        #(#dep_consts)*

        /// 스냅샷 export/스테일 검증 테스트 (#[database] 생성, 명세 §7.4).
        /// `cargo test` 시 현재 버전 스냅샷 파일 부재 = 생성 후 실패(커밋+**재빌드**
        /// 유도 — 신규 파일은 include_bytes 의존성 미등록, 결정 28), 스테일 =
        /// 재생성 후 실패(커밋 유도). `ROOMRS_SCHEMA_EXPORT=0` 으로 비활성.
        #[cfg(test)]
        #[test]
        fn #export_fn() {
            ::roomrs::export_schema_for_test::<#db_ident>(::core::env!("CARGO_MANIFEST_DIR"))
                .unwrap();
        }

        impl ::roomrs::DatabaseSpec for #db_ident {
            const VERSION: u32 = #version;
            const DB_NAME: &'static str = #db_snake;
            const SNAPSHOT_HASH: Option<u64> = #snapshot_hash;
            const SNAPSHOT_FILE_SEEN: bool = #file_seen;
            const EMBEDDED_SCHEMAS: &'static [::roomrs::EmbeddedSchema] = &[
                #(#embedded_entries,)*
            ];

            /// 엔티티 DDL·메타 수집 (#[database] 생성)
            fn schema() -> ::roomrs::SchemaDef {
                let mut ddl: Vec<&'static str> = Vec::new();
                #(ddl.extend_from_slice(<#entities as ::roomrs::Entity>::DDL);)*
                let tables = vec![
                    #(::roomrs::TableMeta {
                        name: <#entities as ::roomrs::Entity>::TABLE,
                        columns: <#entities as ::roomrs::Entity>::COLUMNS_META,
                        ddl: <#entities as ::roomrs::Entity>::DDL,
                    },)*
                ];
                ::roomrs::SchemaDef { version: #version, ddl, tables }
            }

            /// core Database 래핑
            fn from_database(db: ::roomrs::Database) -> Self {
                Self { inner: db }
            }
        }

        impl #db_ident {
            /// 빌더 (명세 §5.4)
            #vis fn builder() -> ::roomrs::DatabaseBuilder<#db_ident> {
                ::core::default::Default::default()
            }

            /// 동기 핸들 (명세 §5.0)
            #vis fn run_sync(&self) -> #sync_ident<'_> {
                #sync_ident { h: self.inner.run_sync() }
            }
        }

        /// 동기 핸들 래퍼 — 직접 쿼리 API(Deref) + DAO 접근자 (#[database] 생성)
        #vis struct #sync_ident<'a> {
            h: ::roomrs::SyncHandle<'a>,
        }

        impl<'a> ::core::ops::Deref for #sync_ident<'a> {
            type Target = ::roomrs::SyncHandle<'a>;
            /// 직접 쿼리 API 위임 (명세 §5.7)
            fn deref(&self) -> &Self::Target {
                &self.h
            }
        }

        impl #sync_ident<'_> {
            #(#sync_accessors)*
        }

        /// 쿼리빌더 핸들 대칭 실행 (명세 §5.3 [C-6]) — SyncHandle 위임 (#[database] 생성)
        impl ::roomrs::Execute for #sync_ident<'_> {
            type Out<R: Send + 'static> = ::roomrs::Result<R>;
            fn run_all<T: ::roomrs::FromRow + Send + 'static>(
                self,
                sql: String,
                params: Vec<::roomrs::rusqlite::types::Value>,
            ) -> Self::Out<Vec<T>> {
                ::roomrs::Execute::run_all(self.h, sql, params)
            }
            fn run_optional<T: ::roomrs::FromRow + Send + 'static>(
                self,
                sql: String,
                params: Vec<::roomrs::rusqlite::types::Value>,
            ) -> Self::Out<Option<T>> {
                ::roomrs::Execute::run_optional(self.h, sql, params)
            }
            fn run_one<T: ::roomrs::FromRow + Send + 'static>(
                self,
                sql: String,
                params: Vec<::roomrs::rusqlite::types::Value>,
            ) -> Self::Out<T> {
                ::roomrs::Execute::run_one(self.h, sql, params)
            }
            fn run_scalar(
                self,
                sql: String,
                params: Vec<::roomrs::rusqlite::types::Value>,
            ) -> Self::Out<i64> {
                ::roomrs::Execute::run_scalar(self.h, sql, params)
            }
            fn fail<R: Send + 'static>(e: ::roomrs::Error) -> Self::Out<R> {
                Err(e)
            }
        }

        /// Tx에 DAO 접근자를 붙이는 확장 trait (#[database] 생성).
        /// 트랜잭션 클로저에서 `tx.xxx_dao()` 사용 시 이 trait가 스코프에 있어야 한다.
        #vis trait #tx_ext_ident {
            #(#tx_decls)*
        }

        impl #tx_ext_ident for ::roomrs::Tx<'_> {
            #(#tx_impls)*
        }

        ::roomrs::__if_async! {
            impl #db_ident {
                /// 비동기 핸들 (명세 §5.0) — 동일 메서드명, await 소비
                #vis fn run_async(&self) -> #async_ident {
                    #async_ident { h: ::roomrs::AsyncHandle::from_database(&self.inner) }
                }
            }

            /// 비동기 핸들 래퍼 — 직접 쿼리 API(Deref) + DAO 접근자 (#[database] 생성)
            #vis struct #async_ident {
                h: ::roomrs::AsyncHandle,
            }

            impl ::core::ops::Deref for #async_ident {
                type Target = ::roomrs::AsyncHandle;
                /// 직접 쿼리 API 위임 (명세 §5.7 비동기 대칭)
                fn deref(&self) -> &Self::Target {
                    &self.h
                }
            }

            impl #async_ident {
                #(#async_accessors)*
            }

            /// 쿼리빌더 핸들 대칭 실행 — AsyncHandle 위임 (#[database] 생성)
            impl ::roomrs::Execute for #async_ident {
                type Out<R: Send + 'static> = ::std::pin::Pin<
                    Box<dyn ::core::future::Future<Output = ::roomrs::Result<R>> + Send + 'static>,
                >;
                fn run_all<T: ::roomrs::FromRow + Send + 'static>(
                    self,
                    sql: String,
                    params: Vec<::roomrs::rusqlite::types::Value>,
                ) -> Self::Out<Vec<T>> {
                    ::roomrs::Execute::run_all(&self.h, sql, params)
                }
                fn run_optional<T: ::roomrs::FromRow + Send + 'static>(
                    self,
                    sql: String,
                    params: Vec<::roomrs::rusqlite::types::Value>,
                ) -> Self::Out<Option<T>> {
                    ::roomrs::Execute::run_optional(&self.h, sql, params)
                }
                fn run_one<T: ::roomrs::FromRow + Send + 'static>(
                    self,
                    sql: String,
                    params: Vec<::roomrs::rusqlite::types::Value>,
                ) -> Self::Out<T> {
                    ::roomrs::Execute::run_one(&self.h, sql, params)
                }
                fn run_scalar(
                    self,
                    sql: String,
                    params: Vec<::roomrs::rusqlite::types::Value>,
                ) -> Self::Out<i64> {
                    ::roomrs::Execute::run_scalar(&self.h, sql, params)
                }
                fn fail<R: Send + 'static>(e: ::roomrs::Error) -> Self::Out<R> {
                    Box::pin(async move { Err(e) })
                }
            }
        }
    })
}
