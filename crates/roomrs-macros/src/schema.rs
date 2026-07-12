//! 스냅샷 로드 · 정적 SQL 검증 (명세 §7.2/§7.3)
//!
//! 정책[B-4]:
//! - 스냅샷 파일 부재 = 검증 스킵 (신규 프로젝트 온보딩 마찰 방지)
//! - 스냅샷 파일 존재 + 파손 = **컴파일 하드 에러** — 부재와 구분 (M-19)
//! - sqlparser 파싱 실패 = 검증 스킵 (stable proc-macro는 경고 방출 불가 — 문서화)
//! - 테이블 존재는 항상 검증, 컬럼 존재는 단일 테이블·별칭 없는 쿼리의
//!   명시 식별자에 한정 (SELECT * · 표현식 · 서브쿼리 제외)

use roomrs_migrate::SchemaSnapshot;
use sqlparser::ast::{
    Expr, Query, Select, SelectItem, SetExpr, Statement, TableFactor, TableWithJoins,
};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;
use std::path::Path;

/// 검증용 스냅샷 로드 (명세 §7.2, 결정 21) — 스키마 디렉토리의 `{db}.{N}.json`
/// 전부를 스캔해 **db별 최신 버전**의 합집합을 반환한다.
/// 디렉토리/파일 부재 = Ok(빈 벡터)(검증 스킵), 파일 존재 + 파손 = Err(한국어
/// 메시지) — 호출자가 syn::Error로 승격한다 (M-19).
pub fn load_validation_snapshots() -> Result<Vec<SchemaSnapshot>, String> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let dir = roomrs_migrate::resolve_schema_dir(&manifest);
    load_snapshots_in_dir(&dir)
}

/// 디렉토리 스캔 — db별 최고 버전 파일만 파스 (이름순 = 결정적 순서)
fn load_snapshots_in_dir(dir: &Path) -> Result<Vec<SchemaSnapshot>, String> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        // 디렉토리 부재 = 스킵 (명세 §7.4c)
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        // 권한 등 그 외 IO 에러 = 부재와 구분해 하드 에러 — 침묵 스킵하면
        // 스냅샷이 있는데도 검증 없이 통과한다 (L-13)
        Err(e) => {
            return Err(format!(
                "스냅샷 디렉토리 읽기 실패: {} — {e} (명세 §7.4)",
                dir.display()
            ));
        }
    };
    // db이름 → (최고 버전, 경로). BTreeMap = 이름순 결정적 조회 순서
    let mut latest: std::collections::BTreeMap<String, (u32, std::path::PathBuf)> =
        std::collections::BTreeMap::new();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((db, num)) = parse_snapshot_file_name(name) else {
            continue;
        };
        // 전부 숫자인데 파스 실패 = u32 오버플로 — 침묵 무시 금지 (L-14)
        let Ok(ver) = num.parse::<u32>() else {
            return Err(format!("스냅샷 파일명 버전이 u32 범위를 넘습니다: {name}"));
        };
        match latest.get(&db) {
            Some((v, _)) if *v >= ver => {}
            _ => {
                latest.insert(db, (ver, entry.path()));
            }
        }
    }
    let mut out = Vec::new();
    for (_, (_, path)) in latest {
        match SchemaSnapshot::read_from(&path) {
            Ok(s) => out.push(s),
            // 존재하는데 파손 = 하드 에러 (M-19)
            Err(e) => {
                return Err(format!(
                    "스냅샷 파일 파손: {} — 파스 실패: {e} (명세 §7.4)",
                    path.display()
                ));
            }
        }
    }
    Ok(out)
}

/// `{db}.{N}.json` 파일명 파스 — (db이름, 버전 숫자 문자열). 규칙 위반 = None.
/// 숫자 파스는 호출자가 수행 — u32 오버플로를 침묵 무시하지 않기 위함 (L-14)
fn parse_snapshot_file_name(name: &str) -> Option<(String, &str)> {
    let stem = name.strip_suffix(".json")?;
    let (db, num) = stem.rsplit_once('.')?;
    if db.is_empty() || num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // 선행 0 금지 — list_snapshot_versions와 동일 규칙
    if num.len() > 1 && num.starts_with('0') {
        return None;
    }
    Some((db.to_string(), num))
}

/// DAO watch DEPENDS_ON용 — SQL의 참조 테이블 목록 (실패 = None, 명세 §9.3 정적 수집).
/// UNION/CTE/파생 테이블/서브쿼리를 재귀 수집한다 (H-9). CTE 이름은 가상
/// 테이블이므로 결과에서 제외. 결과는 대소문자 무시 중복 제거(원본 표기 유지, M-18).
pub fn depends_on(sql: &str) -> Option<Vec<String>> {
    let stmts = Parser::parse_sql(&SQLiteDialect {}, sql).ok()?;
    let mut refs = Refs::default();
    for stmt in &stmts {
        collect_stmt(stmt, &mut refs);
    }
    // 자기 가림 CTE(비재귀 WITH 본문이 자기 이름의 실 테이블 참조) = 의존 판단
    // 불가 — 미상(None) 처리해 core가 UnknownDependencies 경로로 라우팅한다 (H-4)
    if refs.cte_self_shadow {
        return None;
    }
    // 대소문자 무시 중복 제거 — SQLite 식별자 의미론 (M-18)
    let mut out: Vec<String> = Vec::new();
    for t in refs.tables {
        if !out.iter().any(|e| e.eq_ignore_ascii_case(&t)) {
            out.push(t);
        }
    }
    // 재귀 후에도 테이블 0개 = 미상 처리 — 호출자(dao)가 빈 슬라이스로 넘기면
    // core가 UnknownDependencies 경로로 라우팅한다
    if out.is_empty() { None } else { Some(out) }
}

/// SQL에서 수집한 참조
#[derive(Default)]
struct Refs {
    tables: Vec<String>,
    /// 단일 테이블·별칭 없음일 때만 수집되는 평문 컬럼 식별자
    columns: Vec<String>,
    /// 별칭·서브쿼리 존재 = 컬럼 검증 포기
    aliased: bool,
    /// 비재귀 CTE 본문이 자기 이름과 같은 실 테이블을 참조(자기 가림) —
    /// 의존 판단 불가로 depends_on 전체를 미상 처리한다 (H-4)
    cte_self_shadow: bool,
}

/// 정적 SQL 검증 — 에러 메시지 반환(None = 통과 또는 스킵).
/// 스냅샷 합집합 대조: 테이블 존재 = 어느 스냅샷이든 있으면 통과,
/// 컬럼 검증 = 테이블을 포함한 **첫 스냅샷** 기준 (명세 §7.2 union 정책)
pub fn validate_sql(sql: &str, snaps: &[SchemaSnapshot]) -> Option<String> {
    // 파싱 실패 = 스킵 [B-4]
    let stmts = Parser::parse_sql(&SQLiteDialect {}, sql).ok()?;

    // 합집합 테이블 조회 — 첫 일치 스냅샷의 테이블 반환.
    // SQLite 의미론대로 대소문자 무시 비교 (M-18)
    let find_table = |name: &str| {
        snaps
            .iter()
            .find_map(|s| s.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name)))
    };

    for stmt in &stmts {
        let mut refs = Refs::default();
        collect_stmt(stmt, &mut refs);

        // 테이블 존재 검증
        for t in &refs.tables {
            if find_table(t).is_none() {
                return Some(format!("스냅샷에 없는 테이블 참조: \"{t}\" (명세 §7.2)"));
            }
        }

        // 컬럼 검증 — 단일 테이블·별칭 없음 한정 [B-4], 대소문자 무시 (M-18).
        // 동명 테이블이 여러 스냅샷(다른 db)에 서로 다른 컬럼 집합으로 존재하면
        // 어느 db의 테이블인지 판별할 수 없다 — 첫 일치 스냅샷 기준 검증은
        // 다른 db의 유효 SQL을 오탐 하드 에러로 만들었다. 그 경우 이 테이블의
        // 컬럼 검증만 스킵한다(테이블 존재 검증은 위에서 그대로 수행, M-10)
        if !refs.aliased && refs.tables.len() == 1 {
            if let Some(table) = find_table(&refs.tables[0]) {
                let ambiguous = snaps
                    .iter()
                    .filter_map(|s| {
                        s.tables
                            .iter()
                            .find(|t| t.name.eq_ignore_ascii_case(&table.name))
                    })
                    .any(|other| !same_column_names(table, other));
                if ambiguous {
                    continue;
                }
                for c in &refs.columns {
                    if !table
                        .columns
                        .iter()
                        .any(|col| col.name.eq_ignore_ascii_case(c))
                    {
                        return Some(format!(
                            "테이블 \"{}\"에 없는 컬럼 참조: \"{c}\" (명세 §7.2)",
                            table.name
                        ));
                    }
                }
            }
        }
    }
    None
}

/// 두 테이블 스냅샷의 컬럼 **이름 집합**이 같은지 — 대소문자 무시 (M-10)
fn same_column_names(a: &roomrs_migrate::TableSnapshot, b: &roomrs_migrate::TableSnapshot) -> bool {
    a.columns.len() == b.columns.len()
        && a.columns.iter().all(|ca| {
            b.columns
                .iter()
                .any(|cb| cb.name.eq_ignore_ascii_case(&ca.name))
        })
}

/// 문 단위 수집
fn collect_stmt(stmt: &Statement, refs: &mut Refs) {
    match stmt {
        Statement::Query(q) => collect_query(q, refs),
        Statement::Insert(ins) => {
            // 테이블명 + 명시 컬럼
            refs.tables.push(object_name_last(&ins.table_name));
            for c in &ins.columns {
                refs.columns.push(c.value.clone());
            }
            if let Some(src) = &ins.source {
                collect_query(src, refs);
            }
        }
        Statement::Update {
            table,
            assignments,
            selection,
            ..
        } => {
            collect_table_with_joins(table, refs);
            for a in assignments {
                if let sqlparser::ast::AssignmentTarget::ColumnName(name) = &a.target {
                    refs.columns.push(object_name_last(name));
                }
            }
            if let Some(sel) = selection {
                collect_expr(sel, refs);
            }
        }
        Statement::Delete(del) => {
            let from = match &del.from {
                sqlparser::ast::FromTable::WithFromKeyword(v)
                | sqlparser::ast::FromTable::WithoutKeyword(v) => v,
            };
            for t in from {
                collect_table_with_joins(t, refs);
            }
            if let Some(sel) = &del.selection {
                collect_expr(sel, refs);
            }
        }
        _ => {
            // 기타 문(PRAGMA 등)은 검증 대상 아님
        }
    }
}

/// 쿼리(SELECT 계열) 수집 — WITH(CTE)·UNION 재귀 (H-9)
fn collect_query(q: &Query, refs: &mut Refs) {
    // CTE — 본문의 실 테이블은 수집하되, CTE 이름 자체는 가상 테이블이므로
    // 결과에서 제외. 컬럼 검증은 포기 유지(aliased) — 오탐 방지 [B-4].
    // 제외는 **이 쿼리 스코프에서 수집한 테이블에만** 적용한다 — 전역 retain은
    // CTE 이름과 같은 바깥 쿼리의 실 테이블까지 지웠다 (H-4)
    let scope_start = refs.tables.len();
    let mut cte_names: Vec<String> = Vec::new();
    if let Some(with) = &q.with {
        refs.aliased = true;
        for cte in &with.cte_tables {
            let name = cte.alias.name.value.clone();
            let body_start = refs.tables.len();
            collect_query(&cte.query, refs);
            // 자기 가림 검사 — 비재귀 WITH의 본문에서 자기 이름은 **실 테이블**이다
            // (`WITH todos AS (SELECT * FROM todos …) …`). 아래 제외가 그 실 의존을
            // 지우므로 전체를 미상 처리하도록 표시한다. WITH RECURSIVE의 자기
            // 참조는 가상 테이블이라 제외가 올바르다 (H-4)
            if !with.recursive
                && refs.tables[body_start..]
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&name))
            {
                refs.cte_self_shadow = true;
            }
            cte_names.push(name);
        }
    }
    collect_set_expr(q.body.as_ref(), refs);
    if !cte_names.is_empty() {
        // 이 쿼리 스코프(scope_start 이후)에서 수집한 테이블만 CTE 이름 제외 (H-4)
        let scoped = refs.tables.split_off(scope_start);
        refs.tables.extend(
            scoped
                .into_iter()
                .filter(|t| !cte_names.iter().any(|c| c.eq_ignore_ascii_case(t))),
        );
    }
}

/// SetExpr 수집 — UNION/EXCEPT/INTERSECT 좌우 재귀 (H-9)
fn collect_set_expr(se: &SetExpr, refs: &mut Refs) {
    match se {
        SetExpr::Select(sel) => collect_select(sel, refs),
        SetExpr::Query(q) => collect_query(q, refs),
        SetExpr::SetOperation { left, right, .. } => {
            collect_set_expr(left, refs);
            collect_set_expr(right, refs);
        }
        SetExpr::Insert(s) | SetExpr::Update(s) => collect_stmt(s, refs),
        _ => {
            // VALUES/TABLE 등 — 테이블 참조 없음
        }
    }
}

/// SELECT 절 수집
fn collect_select(sel: &Select, refs: &mut Refs) {
    for twj in &sel.from {
        collect_table_with_joins(twj, refs);
    }
    for item in &sel.projection {
        match item {
            SelectItem::UnnamedExpr(e) => collect_expr(e, refs),
            SelectItem::ExprWithAlias { expr, .. } => collect_expr(expr, refs),
            // SELECT * — 컬럼 검증 제외 [B-4]
            _ => {}
        }
    }
    if let Some(where_) = &sel.selection {
        collect_expr(where_, refs);
    }
}

/// FROM/JOIN 항 수집
fn collect_table_with_joins(twj: &TableWithJoins, refs: &mut Refs) {
    collect_table_factor(&twj.relation, refs);
    for j in &twj.joins {
        collect_table_factor(&j.relation, refs);
    }
}

/// 테이블 팩터 — 실 테이블명 수집, 별칭/서브쿼리 = 컬럼 검증 포기
fn collect_table_factor(tf: &TableFactor, refs: &mut Refs) {
    match tf {
        TableFactor::Table { name, alias, .. } => {
            if alias.is_some() {
                refs.aliased = true;
            }
            refs.tables.push(object_name_last(name));
        }
        // FROM (SELECT …) 파생 테이블 — 내부 실 테이블 재귀 수집 (H-9)
        TableFactor::Derived { subquery, .. } => {
            refs.aliased = true;
            collect_query(subquery, refs);
        }
        _ => {
            refs.aliased = true;
        }
    }
}

/// 식에서 평문 컬럼 식별자 수집 — 함수 인자·이항식 재귀, 그 외는 무시(보수적)
fn collect_expr(e: &Expr, refs: &mut Refs) {
    match e {
        Expr::Identifier(id) => refs.columns.push(id.value.clone()),
        Expr::CompoundIdentifier(_) => {
            // table.col 형태 — 별칭 가능성, 컬럼 검증 포기
            refs.aliased = true;
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_expr(left, refs);
            collect_expr(right, refs);
        }
        Expr::UnaryOp { expr, .. } | Expr::Nested(expr) => collect_expr(expr, refs),
        Expr::IsNull(inner) | Expr::IsNotNull(inner) => collect_expr(inner, refs),
        Expr::InList { expr, .. } => collect_expr(expr, refs),
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_expr(expr, refs);
            collect_expr(low, refs);
            collect_expr(high, refs);
        }
        Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
            collect_expr(expr, refs);
            collect_expr(pattern, refs);
        }
        // IN (SELECT …) — 좌변 컬럼 + 서브쿼리 실 테이블 재귀 수집 (H-9)
        Expr::InSubquery { expr, subquery, .. } => {
            collect_expr(expr, refs);
            refs.aliased = true;
            collect_query(subquery, refs);
        }
        // 스칼라 서브쿼리 / EXISTS — 실 테이블 재귀 수집 (H-9)
        Expr::Subquery(q) => {
            refs.aliased = true;
            collect_query(q, refs);
        }
        Expr::Exists { subquery, .. } => {
            refs.aliased = true;
            collect_query(subquery, refs);
        }
        _ => {
            // 리터럴·플레이스홀더·함수 등 — 컬럼 아님 또는 검증 제외
        }
    }
}

/// ObjectName 마지막 세그먼트 (스키마 프리픽스 무시) — Ident.value 직접 사용으로
/// 따옴표 종류(" ` [])와 무관하게 원본 식별자를 얻는다 (M-18)
fn object_name_last(name: &sqlparser::ast::ObjectName) -> String {
    name.0.last().map(|id| id.value.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use roomrs_migrate::{ColumnSnapshot, TableSnapshot};

    /// 컬럼 스냅샷 생성 헬퍼
    fn col(name: &str) -> ColumnSnapshot {
        ColumnSnapshot {
            name: name.into(),
            sql_type: "TEXT".into(),
            not_null: false,
            pk: false,
            renamed_from: None,
        }
    }

    /// 단일 테이블 스냅샷 생성 헬퍼
    fn snap_with(table: &str, cols: &[&str]) -> SchemaSnapshot {
        SchemaSnapshot {
            version: 1,
            tables: vec![TableSnapshot {
                name: table.into(),
                columns: cols.iter().map(|c| col(c)).collect(),
                ddl: vec![],
            }],
        }
    }

    /// H-4(a) — 서브쿼리 CTE 이름이 바깥 쿼리의 동명 실 테이블을 지우지 않는다
    #[test]
    fn cte_exclusion_scoped_to_own_query() {
        let deps =
            depends_on("SELECT * FROM x, (WITH x AS (SELECT 1 FROM other) SELECT * FROM x) d")
                .expect("의존 수집");
        assert!(
            deps.iter().any(|t| t == "x"),
            "바깥 실 테이블 x 유지: {deps:?}"
        );
        assert!(
            deps.iter().any(|t| t == "other"),
            "CTE 본문 실 테이블: {deps:?}"
        );
    }

    /// H-4(b) — 자기 가림 CTE(본문이 자기 이름의 실 테이블 참조) = 미상(None)
    #[test]
    fn cte_self_shadow_degrades_to_none() {
        assert_eq!(
            depends_on("WITH todos AS (SELECT * FROM todos WHERE done = 0) SELECT * FROM todos"),
            None
        );
    }

    /// H-4(c) — 일반 CTE는 이름만 제외, 본문 실 테이블 유지
    #[test]
    fn cte_normal_excludes_alias_keeps_real_tables() {
        let deps =
            depends_on("WITH recent AS (SELECT * FROM logs) SELECT * FROM recent").expect("수집");
        assert_eq!(deps, vec!["logs".to_string()]);
    }

    /// H-4 — WITH RECURSIVE 자기 참조는 가상 테이블 — 미상 강등 없이 실 테이블 수집
    #[test]
    fn recursive_cte_self_reference_not_degraded() {
        let deps = depends_on(
            "WITH RECURSIVE r AS (SELECT id FROM t UNION ALL SELECT id + 1 FROM r) SELECT * FROM r",
        )
        .expect("재귀 CTE 수집");
        assert_eq!(deps, vec!["t".to_string()]);
    }

    /// M-10 — 동명 테이블(다른 db, 다른 컬럼 집합) = 컬럼 검증 스킵(오탐 방지)
    #[test]
    fn union_same_table_name_different_columns_skips_column_check() {
        let snaps = vec![
            snap_with("items", &["id", "name"]),
            snap_with("items", &["id", "price"]),
        ];
        // price는 첫 스냅샷에 없다 — 종전엔 첫 일치 기준 하드 에러(오탐)
        assert_eq!(
            validate_sql("SELECT price FROM items", &snaps),
            None,
            "동명·컬럼 상이 = 컬럼 검증 스킵"
        );
        // 테이블 존재 검증은 유지
        assert!(
            validate_sql("SELECT id FROM ghost", &snaps).is_some(),
            "없는 테이블은 여전히 에러"
        );
    }

    /// M-10 — 동명 테이블이라도 컬럼 집합이 같으면 컬럼 검증 수행
    #[test]
    fn union_same_table_same_columns_still_checks() {
        let snaps = vec![
            snap_with("items", &["id", "name"]),
            snap_with("items", &["id", "name"]),
        ];
        assert!(
            validate_sql("SELECT ghost_col FROM items", &snaps).is_some(),
            "컬럼 집합 동일 = 검증 유지"
        );
        assert_eq!(validate_sql("SELECT name FROM items", &snaps), None);
    }
}
