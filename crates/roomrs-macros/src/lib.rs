//! roomrs-macros — procedural macros: `#[entity]`, `#[dao]`, `#[database]`.
//!
//! Internal crate — use the `roomrs` facade instead. Generated code
//! references the `::roomrs` facade paths.

use proc_macro::TokenStream;

mod dao;
mod database;
mod entity;
mod relation;
mod schema;
mod util;

/// syn 에러 → 컴파일 에러 토큰
fn into_tokens(result: syn::Result<proc_macro2::TokenStream>) -> TokenStream {
    match result {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[entity(table = "…")]` — 구조체를 테이블에 매핑 (명세 §5.1).
/// 필드 속성: `#[pk(autoincrement)]` · `#[column(name/unique/index/default/ignore)]` · `#[json]`
#[proc_macro_attribute]
pub fn entity(args: TokenStream, input: TokenStream) -> TokenStream {
    into_tokens(entity::expand(args.into(), input.into()))
}

/// `#[dao]` — trait DSL을 소비해 동기 DAO 구현을 생성 (명세 §5.2).
/// 메서드 속성: `#[query("…")]` · `#[insert(on_conflict/keep_pk)]` · `#[update("…")]` · `#[delete("…")]`
#[proc_macro_attribute]
pub fn dao(_args: TokenStream, input: TokenStream) -> TokenStream {
    into_tokens(dao::expand(input.into()))
}

/// `#[derive(Relation)]` — 관계 뷰 조립 코드 생성 (명세 결정 로그 7)
#[proc_macro_derive(Relation, attributes(embedded, relation))]
pub fn derive_relation(input: TokenStream) -> TokenStream {
    into_tokens(relation::expand(input.into()))
}

/// `#[database(entities(…), daos(…), version = N)]` — DB 진입점 생성 (명세 §5.4)
///
/// # `DB_NAME` uniqueness (M-11)
///
/// The database name is the struct identifier converted to snake_case and
/// prefixes the snapshot file names (`{db_name}.{version}.json`). It must
/// be unique across every `#[database]` in the crate: two structs whose
/// identifiers collide after conversion (e.g. `AB` and `A_b` both become
/// `a_b`) would share snapshot files and ping-pong the stale check. The
/// macro cannot detect collisions across modules at expansion time.
///
/// # Embedded snapshot growth (L-16)
///
/// Every committed snapshot version is compressed and embedded into the
/// binary, so compile memory and binary size grow monotonically with the
/// number of snapshot files. A pruning policy ("recent K versions plus
/// migration gaps") is a candidate for a future release.
///
/// # Limitation: newly created snapshot files are not tracked (decision 28)
///
/// The macro emits an `include_bytes!` dependency for every snapshot file
/// it reads, so *modifying* a file triggers re-expansion. A *newly created*
/// file cannot be registered (proc-macros cannot depend on a directory),
/// which is why the generated export test fails with a "commit and rebuild"
/// message even on initial snapshot creation — the rebuild picks the new
/// file up.
#[proc_macro_attribute]
pub fn database(args: TokenStream, input: TokenStream) -> TokenStream {
    into_tokens(database::expand(args.into(), input.into()))
}

/// `migrations_dir!("migrations")` — 디렉터리의 `{from}_{to}_이름.sql` 파일들을
/// 컴파일 타임에 임베드해 `Vec<Migration>` 으로 전개 (명세 §8.2).
///
/// 검증: `from < to`(다운그레이드 금지) + `from` 중복 금지 (L-15).
///
/// 한계: 기존 파일의 **내용** 변경은 `include_str!` 의존성으로 재빌드가
/// 보장되지만, 디렉터리에 **새 파일을 추가**한 것은 감지되지 않는다 —
/// 매크로 호출부 파일을 touch 하거나 `cargo clean -p <크레이트>` 후 빌드해야
/// 반영된다 (proc-macro는 디렉터리 자체를 의존성으로 등록할 수 없다).
#[proc_macro]
pub fn migrations_dir(input: TokenStream) -> TokenStream {
    into_tokens(migrations_dir_impl(input.into()))
}

/// migrations_dir! 구현 — 파일명 규칙 검증 + include_str 임베드
fn migrations_dir_impl(input: proc_macro2::TokenStream) -> syn::Result<proc_macro2::TokenStream> {
    let lit: syn::LitStr = syn::parse2(input)?;
    let rel = lit.value();
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| syn::Error::new(lit.span(), "CARGO_MANIFEST_DIR 없음"))?;
    let dir = std::path::Path::new(&manifest).join(&rel);
    let files = scan_migration_files(&dir, lit.span())?;

    let items = files.iter().map(|(from, to, p)| {
        quote::quote! { ::roomrs::Migration::sql(#from, #to, include_str!(#p)) }
    });
    Ok(quote::quote! { vec![#(#items),*] })
}

/// 마이그레이션 디렉터리 스캔 — `{from}_{to}_이름.sql` 규칙 검증.
/// from < to(다운그레이드 금지)·from 중복 금지·선행 0 금지 (L-15)
fn scan_migration_files(
    dir: &std::path::Path,
    span: proc_macro2::Span,
) -> syn::Result<Vec<(u32, u32, String)>> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        syn::Error::new(
            span,
            format!(
                "마이그레이션 디렉터리를 읽을 수 없습니다 ({}): {e}",
                dir.display()
            ),
        )
    })?;

    // (from, to, 절대경로) 수집
    let mut files: Vec<(u32, u32, String)> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| syn::Error::new(span, e.to_string()))?
            .path();
        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let mut parts = name.splitn(3, '_');
        let (Some(f), Some(t)) = (parts.next(), parts.next()) else {
            return Err(syn::Error::new(
                span,
                format!("파일명 규칙 위반: {name}.sql — {{from}}_{{to}}_이름.sql 형식 필요"),
            ));
        };
        let (Ok(from), Ok(to)) = (f.parse::<u32>(), t.parse::<u32>()) else {
            return Err(syn::Error::new(
                span,
                format!("파일명 버전 파싱 실패: {name}.sql — 숫자_{{from}}_{{to}} 필요"),
            ));
        };
        // 선행 0 금지 — "01"과 "1"이 같은 버전으로 중복될 수 있다.
        // 스냅샷 파일명 정책과 동일 규칙 (L-15)
        if (f.len() > 1 && f.starts_with('0')) || (t.len() > 1 && t.starts_with('0')) {
            return Err(syn::Error::new(
                span,
                format!("파일명 버전 선행 0 금지: {name}.sql — \"01\" 대신 \"1\" 을 사용하세요"),
            ));
        }
        // 다운그레이드/자기참조 금지 (L-15)
        if from >= to {
            return Err(syn::Error::new(
                span,
                format!("마이그레이션 버전 역행: {name}.sql — from({from}) < to({to}) 여야 합니다"),
            ));
        }
        files.push((from, to, path.to_string_lossy().replace('\\', "/")));
    }
    files.sort();

    // 같은 from 에서 출발하는 마이그레이션 2개 = 체인 모호 (L-15)
    for w in files.windows(2) {
        if w[0].0 == w[1].0 {
            return Err(syn::Error::new(
                span,
                format!(
                    "중복 from 버전: {} — 같은 버전에서 출발하는 마이그레이션이 2개 이상입니다 ({}_{} / {}_{})",
                    w[0].0, w[0].0, w[0].1, w[1].0, w[1].1
                ),
            ));
        }
    }
    Ok(files)
}

#[cfg(test)]
mod lib_tests {
    use super::scan_migration_files;
    use proc_macro2::Span;

    /// 지정 이름 파일들이 있는 임시 디렉터리 생성
    fn dir_with(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for n in names {
            std::fs::write(dir.path().join(n), "-- sql").unwrap();
        }
        dir
    }

    /// 정상 스캔 — from 오름차순 정렬
    #[test]
    fn scan_ok_sorted() {
        let d = dir_with(&["2_3_b.sql", "1_2_a.sql", "readme.txt"]);
        let files = scan_migration_files(d.path(), Span::call_site()).unwrap();
        let pairs: Vec<(u32, u32)> = files.iter().map(|(f, t, _)| (*f, *t)).collect();
        assert_eq!(pairs, vec![(1, 2), (2, 3)]);
    }

    /// 선행 0 = 에러 — 스냅샷 파일명 정책과 일관 (L-15)
    #[test]
    fn scan_rejects_leading_zeros() {
        let d = dir_with(&["01_2_x.sql"]);
        let err = scan_migration_files(d.path(), Span::call_site()).unwrap_err();
        assert!(err.to_string().contains("선행 0"), "{err}");
        let d2 = dir_with(&["1_02_x.sql"]);
        let err2 = scan_migration_files(d2.path(), Span::call_site()).unwrap_err();
        assert!(err2.to_string().contains("선행 0"), "{err2}");
        // "0_1_init.sql" 은 유효 — 0 자체는 선행 0이 아니다
        let d3 = dir_with(&["0_1_init.sql"]);
        assert!(scan_migration_files(d3.path(), Span::call_site()).is_ok());
    }

    /// 버전 역행·중복 from = 에러 (기존 L-15 규칙 회귀 방지)
    #[test]
    fn scan_rejects_downgrade_and_dup_from() {
        let d = dir_with(&["2_1_bad.sql"]);
        assert!(scan_migration_files(d.path(), Span::call_site()).is_err());
        let d2 = dir_with(&["1_2_a.sql", "1_3_b.sql"]);
        let err = scan_migration_files(d2.path(), Span::call_site()).unwrap_err();
        assert!(err.to_string().contains("중복 from"), "{err}");
    }
}
