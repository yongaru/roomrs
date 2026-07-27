//! `#[database]` 전개 (명세 §5.4)
//!
//! 생성물:
//!   - `struct Db { inner: ::roomrs::Database }` (유닛 구조체 재작성)
//!   - `impl DatabaseSpec` (버전·DDL 수집)
//!   - `builder()` / `run_sync()`
//!   - `DbSync<'a>` — SyncHandle Deref + DAO 접근자
//!   - `DbTxDaos` — Tx용 DAO 접근자 확장 trait

use crate::util::{to_snake_case, validate_sql_identifier};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{Fields, ItemStruct, LitStr, Path};

/// version 모드 — 수동 N 또는 auto (결정 48)
enum VersionMode {
    Manual(u32),
    Auto,
}

/// `#[database(...)]` 인자
struct DatabaseArgs {
    entities: Vec<Path>,
    daos: Vec<Path>,
    version: VersionMode,
    triggers: Vec<DatabaseTrigger>,
}

/// DB-level trigger 선언.
struct DatabaseTrigger {
    name: LitStr,
    sql: LitStr,
    file: Option<LitStr>,
    dependency_path: Option<LitStr>,
}

/// 인자 파싱 — entities(...), daos(...), version = N|auto, trigger(...)
fn parse_args(args: TokenStream, span: proc_macro2::Span) -> syn::Result<DatabaseArgs> {
    let mut entities = Vec::new();
    let mut daos = Vec::new();
    let mut version: Option<VersionMode> = None;
    let mut triggers = Vec::new();

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
            let value = meta.value()?;
            if value.peek(syn::LitInt) {
                let lit: syn::LitInt = value.parse()?;
                let n: u32 = lit.base10_parse()?;
                if n == 0 {
                    return Err(meta.error("version은 1 이상이어야 합니다 (0 = 신규 DB 마커)"));
                }
                version = Some(VersionMode::Manual(n));
            } else if value.peek(syn::Ident) {
                let id: syn::Ident = value.parse()?;
                if id == "auto" {
                    version = Some(VersionMode::Auto);
                } else {
                    return Err(meta.error("version 은 정수 N 또는 auto 만 지원"));
                }
            } else {
                return Err(meta.error("version 은 정수 N 또는 auto 만 지원"));
            }
            Ok(())
        } else if meta.path.is_ident("trigger") {
            let mut name: Option<LitStr> = None;
            let mut sql: Option<LitStr> = None;
            let mut file: Option<LitStr> = None;
            meta.parse_nested_meta(|inner| {
                if inner.path.is_ident("name") {
                    name = Some(inner.value()?.parse()?);
                } else if inner.path.is_ident("sql") {
                    sql = Some(inner.value()?.parse()?);
                } else if inner.path.is_ident("file") {
                    file = Some(inner.value()?.parse()?);
                } else {
                    return Err(inner.error("알 수 없는 trigger 인자 — name/sql/file 만 지원"));
                }
                Ok(())
            })?;
            let name = name.ok_or_else(|| meta.error("trigger에 name = \"...\" 이 필요합니다"))?;
            validate_sql_identifier(&name.value(), name.span())?;
            let (sql, file, dependency_path) = match (sql, file) {
                (Some(sql), None) => (sql, None, None),
                (None, Some(file)) => {
                    let (sql, path) = load_trigger_file(&file)?;
                    let dependency_path = LitStr::new(&path.to_string_lossy().replace('\\', "/"), file.span());
                    (LitStr::new(&sql, file.span()), Some(file), Some(dependency_path))
                }
                (Some(_), Some(_)) => return Err(meta.error("trigger에는 sql 또는 file 중 하나만 지정해야 합니다")),
                (None, None) => return Err(meta.error("trigger에는 sql = \"...\" 또는 file = \"...\" 이 필요합니다")),
            };
            validate_trigger(&name, &sql)?;
            triggers.push(DatabaseTrigger { name, sql, file, dependency_path });
            Ok(())
        } else {
            Err(meta.error("알 수 없는 database 인자 — entities/daos/version/trigger 만 지원"))
        }
    });
    parser.parse2(args)?;

    if entities.is_empty() {
        return Err(syn::Error::new(span, "entities(...)에 엔티티를 1개 이상 지정해야 합니다"));
    }
    let entity_keys: Vec<String> = entities.iter().map(|entity| entity.to_token_stream().to_string()).collect();
    for (index, entity) in entities.iter().enumerate() {
        if entity_keys[..index].contains(&entity_keys[index]) {
            return Err(syn::Error::new(entity.span(), "entities(...)에 같은 엔티티를 중복 지정할 수 없습니다"));
        }
    }
    for (index, trigger) in triggers.iter().enumerate() {
        if triggers[..index].iter().any(|previous| previous.name.value().eq_ignore_ascii_case(&trigger.name.value())) {
            return Err(syn::Error::new(trigger.name.span(), format!("database trigger 이름 중복: {}", trigger.name.value())));
        }
    }
    let Some(version) = version else {
        return Err(syn::Error::new(span, "version = N 또는 version = auto 가 필요합니다"));
    };
    Ok(DatabaseArgs { entities, daos, version, triggers })
}

/// manifest 기준 trigger SQL 파일을 읽는다.
fn load_trigger_file(file: &LitStr) -> syn::Result<(String, std::path::PathBuf)> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| syn::Error::new(file.span(), "CARGO_MANIFEST_DIR 없음 — trigger file은 cargo 빌드에서만 사용할 수 있습니다"))?;
    let path = std::path::Path::new(&manifest).join(file.value());
    let sql = std::fs::read_to_string(&path).map_err(|error| syn::Error::new(file.span(), format!("trigger SQL 파일 읽기 실패: {} — {error}", path.display())))?;
    Ok((sql, path))
}

/// trigger SQL 개수와 선언 이름을 검증한다.
fn validate_trigger(name: &LitStr, sql: &LitStr) -> syn::Result<()> {
    if roomrs_migrate::count_create_trigger_statements(&sql.value()) != 1 {
        return Err(syn::Error::new(sql.span(), "trigger sql/file은 non-TEMP CREATE TRIGGER 문을 정확히 하나 포함해야 합니다"));
    }
    let actual = roomrs_migrate::parse_create_trigger_name(&sql.value()).ok_or_else(|| syn::Error::new(sql.span(), "trigger SQL은 CREATE TRIGGER 문으로 시작해야 합니다"))?;
    if !actual.eq_ignore_ascii_case(&name.value()) {
        return Err(syn::Error::new(name.span(), format!("trigger name과 SQL 이름이 다릅니다: 선언={}, SQL={actual}", name.value())));
    }
    Ok(())
}

/// 스냅샷 스캔 결과 — 현재 버전 해시 · 압축 임베드 · 파일 의존성 토큰
struct SnapshotMeta {
    /// `SNAPSHOT_HASH` 초기화 토큰 — 현재 버전 파일 부재 = `None`
    snapshot_hash: TokenStream,
    /// `EMBEDDED_SCHEMAS` 엔트리들 (버전 오름차순)
    embedded_entries: Vec<TokenStream>,
    /// `include_bytes!` 의존성 등록 상수들 (리뷰 C-1)
    dep_consts: Vec<TokenStream>,
    /// 전개 시점에 현재 버전 스냅샷 파일이 존재했는지 — 런타임 스테일 검증용
    /// `SNAPSHOT_FILE_SEEN` 상수 값 (결정 28, D-3b)
    file_seen: bool,
}

/// auto 모드: 디스크 최대 version (없으면 1) — 컴파일 시점 현재 revision (결정 48)
fn resolve_auto_version(db_snake: &str, span: proc_macro2::Span) -> syn::Result<u32> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| syn::Error::new(span, "CARGO_MANIFEST_DIR 없음 — #[database]는 cargo 빌드에서만 사용할 수 있습니다"))?;
    let dir = roomrs_migrate::resolve_schema_dir(&manifest);
    let files = roomrs_migrate::list_snapshot_versions(&dir, db_snake).map_err(|e| syn::Error::new(span, format!("스냅샷 디렉토리 읽기 실패: {} — {e}", dir.display())))?;
    Ok(files.last().map(|(v, _)| *v).unwrap_or(1))
}

/// 스키마 디렉토리에서 `{db}.{N}.json` 전 버전을 스캔한다 (명세 §7.2/§8.4).
/// 파손 파일 = 하드 에러(부재와 구분, M-19), 버전 > database version = 에러,
/// 각 파일은 include_bytes 의존성 등록 + miniz_oxide 압축 임베드 (결정 21c)
fn scan_snapshots(db_snake: &str, version: u32, span: proc_macro2::Span) -> syn::Result<SnapshotMeta> {
    let mut meta = SnapshotMeta {
        snapshot_hash: quote! { None },
        embedded_entries: Vec::new(),
        dep_consts: Vec::new(),
        file_seen: false,
    };
    // CARGO_MANIFEST_DIR 부재 = 침묵 빈 경로 진행 대신 하드 에러 —
    // migrations_dir! 과 동일 정책 (L-13)
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| syn::Error::new(span, "CARGO_MANIFEST_DIR 없음 — #[database]는 cargo 빌드에서만 사용할 수 있습니다"))?;
    let dir = roomrs_migrate::resolve_schema_dir(&manifest);
    let files = roomrs_migrate::list_snapshot_versions(&dir, db_snake).map_err(|e| syn::Error::new(span, format!("스냅샷 디렉토리 읽기 실패: {} — {e}", dir.display())))?;
    for (ver, path) in &files {
        // 버전 단조성 — database version을 넘는 스냅샷은 정의 오류
        if *ver > version {
            return Err(syn::Error::new(span, format!("스냅샷 버전이 database version보다 큽니다: {} (version = {version})", path.display())));
        }
        let raw = std::fs::read(path).map_err(|e| syn::Error::new(span, format!("스냅샷 파일 읽기 실패: {} — {e}", path.display())))?;
        // 존재하는데 파손 = 컴파일 하드 에러 — 부재(스킵)와 구분 (M-19)
        let snap = roomrs_migrate::SchemaSnapshot::from_slice(&raw).map_err(|e| syn::Error::new(span, format!("스냅샷 파일 파손: {} — 파스 실패: {e} (명세 §7.4)", path.display())))?;
        if snap.version != *ver {
            return Err(syn::Error::new(span, format!("스냅샷 내부 version({})이 파일명 버전({ver})과 다릅니다: {}", snap.version, path.display())));
        }
        // 현재 버전 파일 = 런타임 스테일 검증용 해시 임베드 (명세 §7.4b).
        // 존재 자체도 기록 — 런타임 스테일 검증의 fail-open 창 차단 (D-3b)
        if *ver == version {
            let h = snap.hash();
            meta.snapshot_hash = quote! { Some(#h) };
            meta.file_seen = true;
        }
        // include_bytes 의존성 등록 (리뷰 C-1) — 기존 파일 **갱신** = 재전개 보장.
        // 경로는 resolve_schema_dir 절대화 경로 기반 — fs::read 와 동일 파일 (M-8).
        // 한계: **신규** 파일 추가는 등록 자체가 불가(디렉토리 의존성 미지원) —
        // 명시 export 뒤 사용자가 재빌드해 재전개한다 (결정 28)
        let path_str = path.to_string_lossy().replace('\\', "/");
        meta.dep_consts.push(quote! { const _: &[u8] = ::core::include_bytes!(#path_str); });
        // 압축 바이트 임베드 (결정 21c) — 커밋된 전 버전을 누적 임베드하므로
        // 바이너리가 버전 수에 단조 증가한다. 절삭("최근 K개+갭") 정책은 후속
        // 검토 — #[database] rustdoc에 명시 (L-16)
        let compressed = roomrs_migrate::compress_snapshot(&raw);
        let bytes = proc_macro2::Literal::byte_string(&compressed);
        let v = *ver;
        meta.embedded_entries.push(quote! { ::roomrs::EmbeddedSchema { version: #v, compressed: #bytes } });
    }
    Ok(meta)
}

/// `#[database]` 본체
pub fn expand(args: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let item: ItemStruct = syn::parse2(input)?;
    let args = parse_args(args, item.span())?;

    if !matches!(item.fields, Fields::Unit) {
        return Err(syn::Error::new(item.span(), "#[database]는 유닛 구조체에만 사용할 수 있습니다: struct AppDb;"));
    }

    let db_ident = item.ident.clone();
    let vis = item.vis.clone();
    let attrs: Vec<&syn::Attribute> = item.attrs.iter().collect();
    let sync_ident = format_ident!("{}Sync", db_ident);
    let tx_ext_ident = format_ident!("{}TxDaos", db_ident);
    let entities = &args.entities;

    let async_ident = format_ident!("{}Async", db_ident);

    // db이름 = 구조체명 snake_case (명세 §7.2, 결정 21) — 스냅샷 파일명 프리픽스
    let db_snake = to_snake_case(&db_ident.to_string());
    let export_ident = format_ident!("__roomrs_export_{}", db_snake);
    let export_entrypoint = if std::env::var_os("ROOMRS_SCHEMA_ACTION").is_some() {
        quote! {
            #[::roomrs::roomrs_export]
            fn #export_ident() -> ::roomrs::Result<()> {
                println!("__ROOMRS_SCHEMA_ENTRYPOINT__");
                let manifest = ::core::env!("CARGO_MANIFEST_DIR");
                match ::std::env::var("ROOMRS_SCHEMA_ACTION").as_deref() {
                    Ok("export") => {
                        for path in ::roomrs::run_registered_schema_export(manifest)? {
                            println!("{}", path.display());
                        }
                        Ok(())
                    }
                    Ok("check") => ::roomrs::run_registered_schema_check(manifest),
                    Ok(other) => Err(::roomrs::Error::Config(format!("알 수 없는 ROOMRS_SCHEMA_ACTION: {other}"))),
                    Err(_) => Err(::roomrs::Error::Config("ROOMRS_SCHEMA_ACTION이 없습니다 — cargo roomrs schema export/check로 실행하세요".into())),
                }
            }
        }
    } else {
        quote! {}
    };

    // version = auto 는 디스크 최신 revision 사용 (결정 48)
    let version_is_auto = matches!(args.version, VersionMode::Auto);
    let version = match &args.version {
        VersionMode::Manual(n) => *n,
        VersionMode::Auto => resolve_auto_version(&db_snake, item.span())?,
    };

    // 스냅샷 파일 스캔 + 해시/압축 임베드 (명세 §7.2/§8.4, 결정 21b/21c)
    let snap_meta = scan_snapshots(&db_snake, version, item.span())?;
    let snapshot_hash = snap_meta.snapshot_hash;
    let embedded_entries = snap_meta.embedded_entries;
    let dep_consts = snap_meta.dep_consts;
    let file_seen = snap_meta.file_seen;
    let trigger_meta = args.triggers.iter().map(|trigger| {
        let name = &trigger.name;
        let sql = &trigger.sql;
        let file = trigger.file.as_ref().map_or_else(|| quote! { None }, |file| quote! { Some(#file) });
        quote! {
            ::roomrs::DatabaseTriggerMeta {
                name: #name,
                sql: #sql,
                file: #file,
            }
        }
    });
    let trigger_dep_consts = args.triggers.iter().filter_map(|trigger| trigger.dependency_path.as_ref().map(|path| quote! { const _: &str = ::core::include_str!(#path); }));

    // DAO 접근자 — TodoDao → fn todo_dao()
    let mut sync_accessors: Vec<TokenStream> = Vec::new();
    let mut async_accessors: Vec<TokenStream> = Vec::new();
    let mut tx_decls: Vec<TokenStream> = Vec::new();
    let mut tx_impls: Vec<TokenStream> = Vec::new();
    for dao in &args.daos {
        let dao_name = dao.segments.last().ok_or_else(|| syn::Error::new(dao.span(), "빈 DAO 경로"))?.ident.clone();
        let method = format_ident!("{}", to_snake_case(&dao_name.to_string()));
        let on_ident = {
            // 경로 마지막 세그먼트를 XxxOn으로 치환
            let mut p = dao.clone();
            let last = p.segments.last_mut().ok_or_else(|| syn::Error::new(dao.span(), "빈 DAO 경로"))?;
            last.ident = format_ident!("{}On", last.ident);
            p
        };

        let async_on_ident = {
            let mut p = dao.clone();
            let last = p.segments.last_mut().ok_or_else(|| syn::Error::new(dao.span(), "빈 DAO 경로"))?;
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
        #(#trigger_dep_consts)*

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
                        strict: <#entities as ::roomrs::Entity>::STRICT,
                        without_rowid: <#entities as ::roomrs::Entity>::WITHOUT_ROWID,
                    },)*
                ];
                ::roomrs::SchemaDef {
                    version: #version,
                    ddl,
                    tables,
                    triggers: vec![#(#trigger_meta,)*],
                }
            }

            /// core Database 래핑
            fn from_database(db: ::roomrs::Database) -> Self {
                Self { inner: db }
            }
        }

        // schema export registry (결정 47/48) — cargo roomrs schema export 가 순회
        ::roomrs::__private::inventory::submit! {
            ::roomrs::SchemaExportEntry {
                db_name: <#db_ident as ::roomrs::DatabaseSpec>::DB_NAME,
                version: <#db_ident as ::roomrs::DatabaseSpec>::VERSION,
                auto: #version_is_auto,
                plan: |manifest_dir| {
                    #(
                        if let Some(message) = <#entities as ::roomrs::Entity>::SCHEMA_VALIDATION_ERROR {
                            return Err(::roomrs::Error::Config(message.to_owned()));
                        }
                    )*
                    ::roomrs::plan_export_for_entry(
                        <#db_ident as ::roomrs::DatabaseSpec>::DB_NAME,
                        <#db_ident as ::roomrs::DatabaseSpec>::VERSION,
                        #version_is_auto,
                        &<#db_ident as ::roomrs::DatabaseSpec>::schema(),
                        manifest_dir,
                    )
                },
            }
        }

        // `cargo roomrs schema export/check` 전용 stable harness 진입점(결정 55).
        // CLI 환경과 custom cfg 빌드 지문이 일반 cargo build/test 산출물과 분리한다.
        #export_entrypoint

        impl #db_ident {
            /// 빌더 (명세 §5.4)
            #vis fn builder() -> ::roomrs::DatabaseBuilder<#db_ident> {
                ::core::default::Default::default()
            }

            /// 동기 핸들 (명세 §5.0)
            #vis fn run_sync(&self) -> #sync_ident<'_> {
                #sync_ident { h: self.inner.run_sync() }
            }

            /// LiveQuery 관측성 스냅샷 (명세 §9.5 P2).
            /// Requires the `live` feature on `roomrs`.
            #vis fn live_metrics(&self) -> ::roomrs::LiveMetrics {
                self.inner.live_metrics()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// DB trigger inline SQL과 file 입력을 같은 모델로 파싱한다.
    #[test]
    fn parses_inline_and_file_triggers() {
        let args = quote! {
            entities(Item),
            version = 1,
            trigger(
                name = "inline_trigger",
                sql = "CREATE TRIGGER inline_trigger AFTER INSERT ON items BEGIN SELECT 1; END"
            ),
            trigger(
                name = "sample",
                file = "fixtures/sample_trigger.sql"
            )
        };
        let parsed = parse_args(args, proc_macro2::Span::call_site()).expect("database trigger 파스");
        assert_eq!(parsed.triggers.len(), 2);
        assert!(parsed.triggers[0].file.is_none());
        assert_eq!(parsed.triggers[1].file.as_ref().map(LitStr::value).as_deref(), Some("fixtures/sample_trigger.sql"));
        assert!(parsed.triggers[1].sql.value().contains("CREATE TRIGGER sample"));
    }
}
