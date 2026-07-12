// 컴파일 성공/실패 검증 하네스 [명세 A-4, §5.2, §7, §8.4]
// 스냅샷 기반 케이스는 ROOMRS_SCHEMA_DIR env로 임시 스키마 디렉토리 주입 —
// env는 프로세스 전역이므로 모든 trybuild 케이스를 이 한 테스트에 몰아넣는다.
// 디렉토리에는 `{db}.{버전}.json` 버전 파일들을 기록한다 (결정 21).

use roomrs::{DatabaseSpec, Entity, SchemaDef, TableMeta, dao, database, entity};

#[entity(table = "todos")]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

// 스냅샷 생성용 최소 dao/db (검증 케이스들과 동일 스키마)
#[dao]
trait TodoDao {
    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Option<Todo>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct SnapDb;

// ── 내장 스냅샷 케이스용 픽스처 엔티티 (fixtures의 EmbedDb/GadgetDb와 동일 스키마) ──

/// embed_db v1 — note 컬럼·인덱스 없음
#[entity(table = "embed_items")]
struct EmbedItemV1 {
    #[pk(autoincrement)]
    id: i64,
    name: String,
}

/// embed_db v2 — nullable note 컬럼 + name 인덱스 추가 (안전 연산만)
#[entity(table = "embed_items")]
struct EmbedItemV2 {
    #[pk(autoincrement)]
    id: i64,
    #[column(index)]
    name: String,
    note: Option<String>,
}

/// gadget_db v1 — c: TEXT
#[entity(table = "gadgets")]
struct GadgetV1 {
    #[pk(autoincrement)]
    id: i64,
    c: String,
}

/// gadget_db v2 — c: INTEGER (타입 변경 = 파괴적)
#[entity(table = "gadgets")]
struct GadgetV2 {
    #[pk(autoincrement)]
    id: i64,
    c: i64,
}

/// 엔티티 1개짜리 스냅샷 생성 헬퍼
fn snap_of<E: Entity>(version: u32) -> roomrs::SchemaSnapshot {
    SchemaDef {
        version,
        ddl: E::DDL.to_vec(),
        tables: vec![TableMeta {
            name: E::TABLE,
            columns: E::COLUMNS_META,
            ddl: E::DDL,
            multi_instance: E::MULTI_INSTANCE,
        }],
    }
    .to_snapshot()
}

/// pass = 속성 충돌·유효 SQL·해치·내장 자동 마이그레이션 · fail = 파라미터/스키마 위반
#[test]
fn ui() {
    // 임시 스키마 디렉토리 — 케이스 크레이트의 CARGO_MANIFEST_DIR과 무관하게 env로 주입
    let dir = tempfile::tempdir().expect("tempdir");
    // 검증 케이스용 (struct Db, version = 1)
    <SnapDb as DatabaseSpec>::schema()
        .to_snapshot()
        .write_to(&dir.path().join("db.1.json"))
        .expect("db 스냅샷 저장");
    // 내장/자동 마이그레이션 케이스용 (struct EmbedDb·GadgetDb, version = 2)
    snap_of::<EmbedItemV1>(1)
        .write_to(&dir.path().join("embed_db.1.json"))
        .expect("embed v1 저장");
    snap_of::<EmbedItemV2>(2)
        .write_to(&dir.path().join("embed_db.2.json"))
        .expect("embed v2 저장");
    snap_of::<GadgetV1>(1)
        .write_to(&dir.path().join("gadget_db.1.json"))
        .expect("gadget v1 저장");
    snap_of::<GadgetV2>(2)
        .write_to(&dir.path().join("gadget_db.2.json"))
        .expect("gadget v2 저장");

    // SAFETY: trybuild가 띄우는 자식 cargo/rustc에 상속시키기 위한 프로세스 전역 env.
    // 이 테스트 파일은 단일 #[test]라 동시 변경 경합 없음 (매크로 생성 export
    // 테스트는 리포 설정 ROOMRS_SCHEMA_EXPORT=0 으로 항상 스킵된다).
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ROOMRS_SCHEMA_DIR", dir.path());
    }

    let t = trybuild::TestCases::new();
    // [A-4] 속성 충돌
    t.pass("tests/ui/pass/attr_names.rs");
    // [§7.2] 스냅샷 일치 + 정상 빌드/CRUD + unchecked 해치
    t.pass("tests/ui/pass/schema_ok.rs");
    // [§7.4b] 스테일 스냅샷 = 런타임 SnapshotStale
    t.pass("tests/ui/pass/schema_stale_runtime.rs");
    // [§8.4/결정 21c·21d] 내장 스냅샷 + 자동 마이그레이션 (안전 연산)
    t.pass("tests/ui/pass/schema_embed.rs");
    // [§8.4] 파괴적 구간 = 명확한 에러, 등록 스텝 우선
    t.pass("tests/ui/pass/schema_embed_destructive.rs");
    // [H-9] UNION/CTE watch = DEPENDS_ON 정확 수집 (미상이면 첫 recv 에러)
    t.pass("tests/ui/pass/live_union_cte_deps.rs");
    // [M-15] 단일 autoincrement PK = DEFAULT VALUES insert 왕복
    t.pass("tests/ui/pass/insert_default_values.rs");
    // [H-4] 비-u64 write는 RETURNING 절이 있으면 허용
    t.pass("tests/ui/pass/dao_write_returning.rs");
    // [§5.2] 파라미터 정합성
    t.compile_fail("tests/ui/fail/param_missing_arg.rs");
    t.compile_fail("tests/ui/fail/param_unused_arg.rs");
    // [§7.2] 스키마 대조
    t.compile_fail("tests/ui/fail/schema_bad_table.rs");
    t.compile_fail("tests/ui/fail/schema_bad_column.rs");
    // [§5.1] entity 구조 제약
    t.compile_fail("tests/ui/fail/entity_duplicate_pk.rs");
    t.compile_fail("tests/ui/fail/entity_tuple_struct.rs");
    // [L-13] #[column(name)] 컬럼명 중복
    t.compile_fail("tests/ui/fail/entity_duplicate_column.rs");
    // [M-16] default = "nan" — SQLite DEFAULT 표현 불가
    t.compile_fail("tests/ui/fail/entity_default_nan.rs");
    // [M-17] SQL 속성 2개 = 침묵 승자 대신 에러
    t.compile_fail("tests/ui/fail/dao_two_sql_attrs.rs");
    // [§12c] #[insert] 시그니처 위반
    t.compile_fail("tests/ui/fail/dao_insert_bad_sig.rs");
    // [§5.2] Result 미반환 / SQL 속성 부재
    t.compile_fail("tests/ui/fail/dao_non_result_return.rs");
    t.compile_fail("tests/ui/fail/dao_missing_sql_attr.rs");
    // [H-8] #[transaction] 매크로 토큰 안 self
    t.compile_fail("tests/ui/fail/dao_self_in_macro.rs");
    // [M-12] #[query] SELECT + u64 반환
    t.compile_fail("tests/ui/fail/dao_query_select_u64.rs");
    // [H-1/H-4/L-10/L-13] DAO write 계약과 진단
    t.compile_fail("tests/ui/fail/dao_insert_ignore_rowid.rs");
    t.compile_fail("tests/ui/fail/dao_update_without_returning.rs");
    t.compile_fail("tests/ui/fail/dao_delete_without_returning.rs");
    t.compile_fail("tests/ui/fail/dao_missing_receiver.rs");
    t.compile_fail("tests/ui/fail/dao_result_live_query.rs");
    t.compile_fail("tests/ui/fail/dao_result_unit.rs");
    // [M-13] relation 키 무효 식별자
    t.compile_fail("tests/ui/fail/relation_bad_key_ident.rs");
    // [§5.4] database 인자 검증
    t.compile_fail("tests/ui/fail/database_version_zero.rs");
    t.compile_fail("tests/ui/fail/database_no_entities.rs");
    // [M-4/L-8/L-9/L-11] 생성 SQL과 매크로 입력의 침묵 실패 차단
    t.compile_fail("tests/ui/fail/entity_quoted_identifier.rs");
    t.compile_fail("tests/ui/fail/relation_conflicting_attrs.rs");
    t.compile_fail("tests/ui/fail/relation_quoted_identifier.rs");
    t.compile_fail("tests/ui/fail/database_duplicate_entity.rs");
    t.compile_fail("tests/ui/fail/dao_unsupported_parameter.rs");
}
