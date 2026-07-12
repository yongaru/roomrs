//! roomrs-migrate — schema snapshot model shared between the proc-macros
//! (compile time) and the runtime (명세 §3, §7).
//!
//! Internal crate — use the `roomrs` facade instead.
#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 스키마 스냅샷 — 리포에 커밋되는 검증 진실 소스 (명세 §7.1)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaSnapshot {
    /// 스키마 버전
    pub version: u32,
    /// 테이블 목록
    pub tables: Vec<TableSnapshot>,
}

/// 테이블 스냅샷
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableSnapshot {
    pub name: String,
    pub columns: Vec<ColumnSnapshot>,
    /// 테이블·인덱스 DDL — diff 초안·해시에 사용 (M3)
    #[serde(default)]
    pub ddl: Vec<String>,
}

/// 컬럼 스냅샷
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnSnapshot {
    pub name: String,
    /// SQLite 타입 (빈 문자열 = typeless)
    pub sql_type: String,
    pub not_null: bool,
    pub pk: bool,
    /// rename 힌트 (명세 §8.3) — diff 초안 전용, 해시 제외
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

impl SchemaSnapshot {
    /// 파일에서 로드
    pub fn read_from(path: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw).map_err(std::io::Error::other)
    }

    /// Parse a snapshot from raw JSON bytes (e.g. a decompressed embedded blob).
    pub fn from_slice(bytes: &[u8]) -> std::io::Result<Self> {
        serde_json::from_slice(bytes).map_err(std::io::Error::other)
    }

    /// Serialize to the pretty JSON format used by snapshot files.
    pub fn to_json(&self) -> std::io::Result<String> {
        serde_json::to_string_pretty(self).map_err(std::io::Error::other)
    }

    /// 파일로 저장 (pretty JSON — 리뷰 가능한 diff)
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_json()?)
    }

    /// 테이블 조회
    pub fn table(&self, name: &str) -> Option<&TableSnapshot> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// 정준 문자열 — 해시 입력. 테이블은 이름순 정렬, 컬럼은 선언 순서 유지.
    /// 각 가변 필드는 길이 접두사(`{len}:{값}`)로 인코딩해 필드 경계 충돌을
    /// 원천 차단한다 (L-14).
    pub fn canonical_string(&self) -> String {
        // 길이 접두사 필드 인코딩 — 구분자가 값 안에 있어도 모호하지 않다 (L-14)
        fn push_field(out: &mut String, s: &str) {
            out.push_str(&format!("{}:{}", s.len(), s));
        }
        let mut tables: Vec<&TableSnapshot> = self.tables.iter().collect();
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out = format!("v{};", self.version);
        for t in tables {
            out.push('t');
            push_field(&mut out, &t.name);
            out.push('(');
            for c in &t.columns {
                out.push('c');
                push_field(&mut out, &c.name);
                out.push('y');
                push_field(&mut out, &c.sql_type);
                out.push_str(&format!("n{}p{},", c.not_null as u8, c.pk as u8));
            }
            out.push_str(");");
            // 인덱스 등 DDL 변경도 스테일로 감지되도록 해시에 포함 (M3)
            for d in &t.ddl {
                out.push('d');
                push_field(&mut out, d);
                out.push(';');
            }
        }
        out
    }

    /// FNV-1a 64 해시 — 스테일 감지용 (명세 §7.4). 암호학적 강도 불필요.
    pub fn hash(&self) -> u64 {
        fnv1a64(self.canonical_string().as_bytes())
    }
}

impl TableSnapshot {
    /// 컬럼 존재 확인
    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c.name == name)
    }
}

/// FNV-1a 64비트 해시
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ─────────────────────── 압축 (결정 21c) ───────────────────────

/// 압축 해제 상한 — 스냅샷 JSON이 이 크기를 넘으면 손상으로 간주 (64MB)
const DECOMPRESS_LIMIT: usize = 64 * 1024 * 1024;

/// Compress snapshot JSON bytes with miniz_oxide (raw deflate, level 8).
///
/// Used by `#[database]` to embed every committed snapshot into the binary
/// (spec §8.4, decision 21c). Reverse with [`decompress_snapshot`].
pub fn compress_snapshot(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(bytes, 8)
}

/// Decompress bytes produced by [`compress_snapshot`].
///
/// Fails when the stream is corrupt or the decompressed size exceeds the
/// 64 MiB sanity limit.
pub fn decompress_snapshot(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_with_limit(bytes, DECOMPRESS_LIMIT)
        .map_err(|_| std::io::Error::other("스냅샷 압축 해제 실패: 스트림 손상 또는 64MB 초과"))
}

// ─────────────────────── 스냅샷 파일 경로 (결정 21) ───────────────────────

/// Standard snapshot directory relative to the crate manifest (spec §7.2).
pub const SCHEMA_DIR_RELATIVE: &str = "migrations/schema";

/// Resolve the snapshot directory: the `ROOMRS_SCHEMA_DIR` environment
/// variable wins when set and non-empty, otherwise
/// `{manifest_dir}/migrations/schema` (spec §7.2, decision 21).
///
/// A relative `ROOMRS_SCHEMA_DIR` is joined onto `manifest_dir`, so that
/// compile-time file reads and the `include_bytes!` dependency paths the
/// macros emit resolve to the same files regardless of the working
/// directory (M-8). The variable must be set identically for build and
/// test runs — otherwise the embedded snapshot chain and the exported
/// files silently diverge.
pub fn resolve_schema_dir(manifest_dir: &str) -> PathBuf {
    match std::env::var("ROOMRS_SCHEMA_DIR") {
        Ok(p) if !p.is_empty() => {
            let p = PathBuf::from(p);
            // 상대 경로 = manifest 기준 절대화 — fs::read(CWD 기준)와
            // include_bytes!(소스 기준)가 서로 다른 파일을 보는 이중 해석 차단 (M-8)
            if p.is_relative() {
                Path::new(manifest_dir).join(p)
            } else {
                p
            }
        }
        _ => Path::new(manifest_dir).join(SCHEMA_DIR_RELATIVE),
    }
}

/// File name of a versioned snapshot: `{db_name}.{version}.json`
/// (decision 21 — the `current.json` single-file model is gone).
pub fn snapshot_file_name(db_name: &str, version: u32) -> String {
    format!("{db_name}.{version}.json")
}

/// Full path of a versioned snapshot inside a schema directory.
pub fn snapshot_path(dir: &Path, db_name: &str, version: u32) -> PathBuf {
    dir.join(snapshot_file_name(db_name, version))
}

/// List every `{db_name}.{N}.json` snapshot in `dir`, sorted ascending by
/// version. `N` is parsed strictly (digits only, no leading zeros), so
/// duplicate versions are impossible by file name. A missing directory
/// yields an empty list.
pub fn list_snapshot_versions(dir: &Path, db_name: &str) -> std::io::Result<Vec<(u32, PathBuf)>> {
    let mut out: Vec<(u32, PathBuf)> = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        // 디렉토리 부재 = 스냅샷 없음 (온보딩 마찰 방지, 명세 §7.4c)
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    let prefix = format!("{db_name}.");
    for entry in rd {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(v) = parse_snapshot_version(name, &prefix)? else {
            continue;
        };
        out.push((v, entry.path()));
    }
    out.sort_by_key(|(v, _)| *v);
    Ok(out)
}

/// `{prefix}{N}.json` 파일명에서 버전 N 엄격 파스 — 숫자만, 선행 0 금지.
/// 무관 파일 = Ok(None), 숫자 형식인데 u32 범위 초과 = 침묵 무시 대신
/// 하드 에러 — 해당 버전이 조용히 빠지는 것을 차단한다 (L-14)
fn parse_snapshot_version(name: &str, prefix: &str) -> std::io::Result<Option<u32>> {
    let Some(rest) = name.strip_prefix(prefix) else {
        return Ok(None);
    };
    let Some(num) = rest.strip_suffix(".json") else {
        return Ok(None);
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    // 선행 0 금지 — "01"과 "1"이 같은 버전으로 중복되는 것을 차단
    if num.len() > 1 && num.starts_with('0') {
        return Ok(None);
    }
    match num.parse() {
        Ok(v) => Ok(Some(v)),
        // 전부 숫자인데 파스 실패 = u32 오버플로 (L-14)
        Err(_) => Err(std::io::Error::other(format!(
            "스냅샷 파일명 버전이 u32 범위를 넘습니다: {name}"
        ))),
    }
}

// ─────────────────────── 구조화 diff (H-10/H-11) ───────────────────────

/// Structured migration plan between two snapshots (spec §8.1/§8.4).
///
/// `safe` holds executable SQL statements (CREATE TABLE, nullable ADD
/// COLUMN, valid RENAME COLUMN, CREATE INDEX). `destructive` holds
/// human-review items that roomrs never runs automatically (DROP TABLE,
/// DROP COLUMN, column definition changes, NOT NULL ADD COLUMN without a
/// default, DROP INDEX). `warnings` reports ignored or invalid rename
/// hints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffPlan {
    /// Executable safe statements, in order.
    pub safe: Vec<String>,
    /// Destructive items requiring human review.
    pub destructive: Vec<String>,
    /// Non-fatal diagnostics (e.g. invalid rename hints).
    pub warnings: Vec<String>,
}

/// Compute a structured diff plan from `old` to `new` (spec §8.1, §8.4).
///
/// Rename hints (`renamed_from`) produce a safe RENAME COLUMN only when the
/// old column exists, the new schema no longer has it, and the column
/// definition is unchanged; otherwise they degrade to a warning or a
/// destructive rewrite item (H-10/H-11). A rename source column can be
/// consumed only once — a second hint pointing at the same source degrades
/// to ADD COLUMN with a warning (M-9).
///
/// Constraint-level `CREATE TABLE` changes (UNIQUE/DEFAULT/CHECK …) are not
/// visible in the column snapshots, so when the column definitions of a
/// same-name table are identical but its `CREATE TABLE` statement text
/// differs (whitespace-normalized), a destructive table-rewrite item is
/// emitted (H-5). Limitation: when column changes and constraint changes
/// happen in the same version step, the DDL text difference cannot be
/// attributed, so only the column-level items are reported.
pub fn diff_plan(old: &SchemaSnapshot, new: &SchemaSnapshot) -> DiffPlan {
    let mut plan = DiffPlan::default();

    // 새 테이블 — DDL 전체가 안전 연산
    for nt in &new.tables {
        if old.table(&nt.name).is_none() {
            plan.safe.extend(nt.ddl.iter().cloned());
        }
    }

    // 삭제된 테이블 — 파괴적
    for ot in &old.tables {
        if new.table(&ot.name).is_none() {
            plan.destructive.push(format!("DROP TABLE \"{}\"", ot.name));
        }
    }

    // 기존 테이블의 컬럼·인덱스 변경
    for nt in &new.tables {
        let Some(ot) = old.table(&nt.name) else {
            continue;
        };
        diff_columns(ot, nt, &mut plan);
        diff_indexes(ot, nt, &mut plan);
        diff_table_constraints(ot, nt, &mut plan);
    }

    plan
}

/// 동일 이름 테이블의 CREATE TABLE 문 비교 — 제약(UNIQUE/DEFAULT/CHECK 등)
/// 변경 감지 (H-5). 컬럼 스냅샷에는 제약 정보가 없어 컬럼 diff만으로는
/// 빈 계획이 나와 auto_migrate가 조용히 스키마 분기를 방치한다.
///
/// 한계(의도된 보수 정책): 컬럼 정의(이름/타입/not_null/pk)가 **완전히 동일**할
/// 때만 비교한다 — 컬럼 추가/삭제/rename이 있으면 CREATE TABLE 문은 그로
/// 인해 당연히 달라지므로, 그 차이를 재작성 필요로 오탐하지 않기 위해
/// 건너뛴다. 컬럼 변경과 제약 변경이 한 버전에서 동시에 일어나면 제약
/// 변경은 감지되지 않는다 (rustdoc의 diff_plan 한계 참조).
fn diff_table_constraints(ot: &TableSnapshot, nt: &TableSnapshot, plan: &mut DiffPlan) {
    // 컬럼 집합 동일 검사 — 이름 기준 매칭(순서 무관), 정의 필드 전부 일치
    let same_columns = ot.columns.len() == nt.columns.len()
        && nt.columns.iter().all(|n| {
            ot.columns.iter().any(|o| {
                o.name == n.name
                    && o.sql_type == n.sql_type
                    && o.not_null == n.not_null
                    && o.pk == n.pk
            })
        });
    if !same_columns {
        return;
    }
    // 구형 스냅샷(serde default)은 ddl 이 빈 벡터 — 한쪽만 비면 [] vs [CREATE …]
    // 차이를 재작성 필요로 오탐한다. 비교 스킵 + 경고로 강등 (D-2b)
    if ot.ddl.is_empty() || nt.ddl.is_empty() {
        if ot.ddl.is_empty() != nt.ddl.is_empty() {
            plan.warnings.push(format!(
                "제약 비교 스킵: \"{}\" — 한쪽 스냅샷에 ddl 정보가 없습니다(구형 스냅샷) — write_schema_snapshot 재생성 권장",
                nt.name
            ));
        }
        return;
    }
    if create_table_entries(ot) != create_table_entries(nt) {
        plan.destructive.push(format!(
            "테이블 재작성 필요(제약/DDL 변경): \"{}\" — 컬럼 정의는 동일하나 CREATE TABLE 문이 다릅니다",
            nt.name
        ));
    }
}

/// 인덱스 외 DDL(CREATE TABLE 문)만 공백 정규화해 추출 (H-5).
/// 정규화는 인용 구간 **밖**의 공백만 접는다 (D-2a)
fn create_table_entries(t: &TableSnapshot) -> Vec<String> {
    t.ddl
        .iter()
        .filter(|d| index_name(d).is_none())
        .map(|d| normalize_ws_outside_quotes(d))
        .collect()
}

/// 인용 구간('…', "…") 밖의 연속 공백을 1개로 접고 선두/말미 공백을 제거한다.
/// split_whitespace 전면 접기는 `DEFAULT 'a  b'` → `DEFAULT 'a b'` 리터럴
/// 변경을 지워 미탐이었다 (D-2a). 인용 안 `''`/`""` 이스케이프는 상태를
/// 껐다 켜도 사이에 공백이 없어 결과가 동일하다.
fn normalize_ws_outside_quotes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    // 현재 인용 문자 — None = 인용 밖
    let mut quote: Option<char> = None;
    // 인용 밖에서 만난 공백 대기 플래그 — 다음 비공백 앞에 1개만 삽입
    let mut pending_ws = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                out.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c.is_whitespace() => pending_ws = true,
            None => {
                if pending_ws && !out.is_empty() {
                    out.push(' ');
                }
                pending_ws = false;
                if c == '\'' || c == '"' {
                    quote = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

/// 동일 이름 테이블의 컬럼 diff — 정의 변경 감지(H-10)·rename 힌트 검증(H-11)·
/// rename 원본 중복 소비 차단(M-9)
fn diff_columns(ot: &TableSnapshot, nt: &TableSnapshot, plan: &mut DiffPlan) {
    // rename 힌트가 이미 소비한 원본 컬럼 — 같은 원본을 두 번 RENAME 하면
    // 두 번째 문이 런타임에 실패한다(safe 계약 위반, M-9)
    let mut consumed_sources: Vec<&str> = Vec::new();
    for nc in &nt.columns {
        // 동명 컬럼 — sql_type/not_null/pk 변경 = 테이블 재작성 필요 (H-10)
        if let Some(oc) = ot.columns.iter().find(|c| c.name == nc.name) {
            let mut changes: Vec<String> = Vec::new();
            if oc.sql_type != nc.sql_type {
                changes.push(format!(
                    "sql_type \"{}\" -> \"{}\"",
                    oc.sql_type, nc.sql_type
                ));
            }
            if oc.not_null != nc.not_null {
                changes.push(format!("not_null {} -> {}", oc.not_null, nc.not_null));
            }
            if oc.pk != nc.pk {
                changes.push(format!("pk {} -> {}", oc.pk, nc.pk));
            }
            if !changes.is_empty() {
                plan.destructive.push(format!(
                    "테이블 재작성 필요: \"{}\".\"{}\" 정의 변경 ({})",
                    nt.name,
                    nc.name,
                    changes.join(", ")
                ));
            }
            continue;
        }

        // 신규 컬럼 — rename 힌트 우선 (명세 §8.3)
        if let Some(from) = &nc.renamed_from {
            if consumed_sources.contains(&from.as_str()) {
                // 같은 원본을 이미 다른 컬럼이 rename으로 소비 — 두 번째 RENAME은
                // 런타임 실패이므로 경고 + ADD COLUMN 강등 (M-9)
                plan.warnings.push(format!(
                    "rename 힌트 중복: \"{}\".\"{from}\" 은 이미 다른 컬럼의 rename 원본으로 소비됨 — \"{}\"는 ADD COLUMN으로 처리",
                    nt.name, nc.name
                ));
            } else if nt.has_column(from) {
                // 새 스키마에 원본 컬럼이 여전히 존재 = 힌트 무효 (H-11) — ADD로 강등
                plan.warnings.push(format!(
                    "rename 힌트 무시: 새 스키마 \"{}\"에 \"{from}\" 컬럼이 여전히 존재 — \"{}\"는 ADD COLUMN으로 처리",
                    nt.name, nc.name
                ));
            } else if let Some(of) = ot.columns.iter().find(|c| &c.name == from) {
                // 원본 소비 기록 — safe RENAME이든 파괴적 재작성이든 원본은 소비됨 (M-9)
                consumed_sources.push(from);
                if of.sql_type == nc.sql_type && of.not_null == nc.not_null && of.pk == nc.pk {
                    plan.safe.push(format!(
                        "ALTER TABLE \"{}\" RENAME COLUMN \"{from}\" TO \"{}\"",
                        nt.name, nc.name
                    ));
                } else {
                    // rename + 정의 변경 동시 = RENAME COLUMN으로 불가 (H-10)
                    plan.destructive.push(format!(
                        "테이블 재작성 필요: \"{}\".\"{from}\" -> \"{}\" rename에 정의 변경이 동반됨",
                        nt.name, nc.name
                    ));
                }
                continue;
            } else {
                plan.warnings.push(format!(
                    "잘못된 rename 힌트: 옛 스키마 \"{}\"에 \"{from}\" 컬럼 없음 — \"{}\"는 ADD COLUMN으로 처리",
                    nt.name, nc.name
                ));
            }
        }

        // 일반 ADD COLUMN
        let mut col = format!("\"{}\"", nc.name);
        if !nc.sql_type.is_empty() {
            col.push_str(&format!(" {}", nc.sql_type));
        }
        if nc.pk {
            plan.destructive.push(format!(
                "테이블 재작성 필요: \"{}\".\"{}\" — PK 컬럼은 ADD COLUMN 불가",
                nt.name, nc.name
            ));
        } else if nc.not_null {
            // NOT NULL ADD COLUMN은 기존 행 때문에 DEFAULT 필요 — 스냅샷에
            // DEFAULT 정보가 없으므로 파괴적(수동 검토)으로 분류
            plan.destructive.push(format!(
                "ALTER TABLE \"{}\" ADD COLUMN {col} NOT NULL — DEFAULT 필요(기존 행)",
                nt.name
            ));
        } else {
            plan.safe
                .push(format!("ALTER TABLE \"{}\" ADD COLUMN {col}", nt.name));
        }
    }

    // 삭제된 컬럼 — 유효한 rename으로 소비된 컬럼은 제외
    for oc in &ot.columns {
        let renamed_away = nt
            .columns
            .iter()
            .any(|nc| nc.renamed_from.as_deref() == Some(oc.name.as_str()));
        if !nt.has_column(&oc.name) && !renamed_away {
            plan.destructive.push(format!(
                "ALTER TABLE \"{}\" DROP COLUMN \"{}\"",
                nt.name, oc.name
            ));
        }
    }
}

/// 동일 이름 테이블의 인덱스 diff — 이름 기준 추가/삭제/변경 감지 (H-10)
fn diff_indexes(ot: &TableSnapshot, nt: &TableSnapshot, plan: &mut DiffPlan) {
    let old_idx: Vec<(String, &String)> = index_entries(ot);
    let new_idx: Vec<(String, &String)> = index_entries(nt);

    for (name, ddl) in &new_idx {
        match old_idx.iter().find(|(n, _)| n == name) {
            // 신규 인덱스 = 안전
            None => plan.safe.push((*ddl).clone()),
            // 동명 인덱스 정의 변경 = 재생성 필요(파괴적 검토)
            Some((_, old_ddl))
                if normalize_ws_outside_quotes(old_ddl) != normalize_ws_outside_quotes(ddl) =>
            {
                plan.destructive.push(format!(
                    "인덱스 재생성 필요: DROP INDEX \"{name}\" 후 재생성 ({ddl})"
                ))
            }
            Some(_) => {}
        }
    }
    for (name, _) in &old_idx {
        if !new_idx.iter().any(|(n, _)| n == name) {
            plan.destructive.push(format!("DROP INDEX \"{name}\""));
        }
    }
}

/// 테이블 DDL에서 (인덱스명, DDL) 목록 추출
fn index_entries(t: &TableSnapshot) -> Vec<(String, &String)> {
    t.ddl
        .iter()
        .filter_map(|d| index_name(d).map(|n| (n, d)))
        .collect()
}

/// `CREATE [UNIQUE] INDEX [IF NOT EXISTS] name …` DDL에서 인덱스명 추출.
/// 인용 식별자(`"…"` / `` `…` `` / `[…]`) 안의 공백·이스케이프를 처리한다 —
/// split_whitespace 기반 추출은 `CREATE INDEX "my idx"` 를 `"my` 로 절단했다 (M-12)
fn index_name(ddl: &str) -> Option<String> {
    let mut rest = strip_keyword(ddl, "CREATE")?;
    if let Some(r) = strip_keyword(rest, "UNIQUE") {
        rest = r;
    }
    rest = strip_keyword(rest, "INDEX")?;
    if let Some(r) = strip_keyword(rest, "IF") {
        // IF NOT EXISTS 건너뛰기
        rest = strip_keyword(strip_keyword(r, "NOT")?, "EXISTS")?;
    }
    parse_leading_identifier(rest)
}

/// 선두 키워드 소비(대소문자 무시) — 매치하면 키워드 뒤 나머지 반환 (M-12).
/// 비교는 바이트 단위 — `&s[..kw.len()]` 문자열 슬라이스는 멀티바이트
/// 식별자(예: 한글 인덱스명)에서 char boundary panic 이었다 (D-1).
/// kw 는 전부 ASCII 이므로 바이트 매치 성공 = `kw.len()` 이 char boundary 보장.
fn strip_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let s = s.trim_start();
    if s.len() < kw.len() || !s.as_bytes()[..kw.len()].eq_ignore_ascii_case(kw.as_bytes()) {
        return None;
    }
    let rest = &s[kw.len()..];
    // 키워드 경계 — 바로 뒤가 식별자 문자면 키워드가 아니다 (예: INDEXED)
    match rest.chars().next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => None,
        _ => Some(rest),
    }
}

/// 선두 식별자 파스 — 인용부호별 종료·이스케이프(`""` / ```` `` ````) 처리 (M-12)
fn parse_leading_identifier(s: &str) -> Option<String> {
    let s = s.trim_start();
    let mut chars = s.chars();
    match chars.next()? {
        // "…" 또는 `…` — 짝 인용부호 2연속 = 이스케이프
        q @ ('"' | '`') => {
            let mut out = String::new();
            let mut iter = chars.peekable();
            while let Some(c) = iter.next() {
                if c == q {
                    if iter.peek() == Some(&q) {
                        iter.next();
                        out.push(q);
                        continue;
                    }
                    return Some(out);
                }
                out.push(c);
            }
            // 닫히지 않은 인용부 = 파스 실패
            None
        }
        // […] — 이스케이프 없음, `]` 가 종료
        '[' => s[1..].find(']').map(|end| s[1..1 + end].to_string()),
        _ => {
            // 비인용 — 공백 또는 여는 괄호(`name(col)` 표기) 앞까지
            let end = s
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(s.len());
            if end == 0 {
                None
            } else {
                Some(s[..end].to_string())
            }
        }
    }
}

/// 자동 diff **초안** SQL 생성 (명세 §8.1) — 절대 자동 실행하지 않는다(비목표).
/// [`diff_plan`] 위에 렌더: 안전 연산은 실행문, 파괴적 항목은 `-- TODO(파괴적)`
/// 주석, 경고는 `-- 경고` 주석으로 출력한다.
pub fn diff_sql(old: &SchemaSnapshot, new: &SchemaSnapshot) -> String {
    let plan = diff_plan(old, new);
    let mut out = format!(
        "-- roomrs 자동 diff 초안: v{} -> v{}\n-- 검토 후 사용하세요. 파괴적 변경은 주석 처리되어 있습니다.\n",
        old.version, new.version
    );
    for s in &plan.safe {
        out.push_str(&format!("{s};\n"));
    }
    for d in &plan.destructive {
        out.push_str(&format!("-- TODO(파괴적): {d}\n"));
    }
    for w in &plan.warnings {
        out.push_str(&format!("-- 경고: {w}\n"));
    }
    out.push_str(&format!("PRAGMA user_version = {};\n", new.version));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 컬럼 스냅샷 생성 헬퍼
    fn col(name: &str, sql_type: &str, not_null: bool, pk: bool) -> ColumnSnapshot {
        ColumnSnapshot {
            name: name.into(),
            sql_type: sql_type.into(),
            not_null,
            pk,
            renamed_from: None,
        }
    }

    /// 테이블 스냅샷 생성 헬퍼
    fn table(name: &str, columns: Vec<ColumnSnapshot>, ddl: Vec<&str>) -> TableSnapshot {
        TableSnapshot {
            name: name.into(),
            columns,
            ddl: ddl.into_iter().map(String::from).collect(),
        }
    }

    /// 단일 테이블 스냅샷 생성 헬퍼
    fn snap(version: u32, tables: Vec<TableSnapshot>) -> SchemaSnapshot {
        SchemaSnapshot { version, tables }
    }

    /// 정준 문자열이 테이블 순서에 불변인지
    #[test]
    fn canonical_is_order_invariant() {
        let a = snap(
            1,
            vec![table("b", vec![], vec![]), table("a", vec![], vec![])],
        );
        let mut b = a.clone();
        b.tables.reverse();
        assert_eq!(a.hash(), b.hash());
    }

    /// 컬럼 변경 = 해시 변경
    #[test]
    fn hash_changes_on_column_change() {
        let a = snap(
            1,
            vec![table("t", vec![col("id", "INTEGER", true, true)], vec![])],
        );
        let mut b = a.clone();
        b.tables[0].columns[0].name = "id2".into();
        assert_ne!(a.hash(), b.hash());
    }

    /// 정준 문자열 필드 경계 충돌 불가 (L-14) — 구분자 포함 값도 구별된다
    #[test]
    fn canonical_no_field_boundary_collision() {
        // 옛 포맷("name:type:nn:pk,")에서는 두 스냅샷 모두 "a:X:1:0:0," 로 충돌
        let a = snap(
            1,
            vec![table("t", vec![col("a", "X:1", false, false)], vec![])],
        );
        let b = snap(
            1,
            vec![table("t", vec![col("a:X", "1", false, false)], vec![])],
        );
        assert_ne!(a.canonical_string(), b.canonical_string());
        assert_ne!(a.hash(), b.hash());
    }

    /// 압축/해제 왕복
    #[test]
    fn compress_roundtrip() {
        let json = snap(
            3,
            vec![table("t", vec![col("id", "INTEGER", true, true)], vec![])],
        )
        .to_json()
        .unwrap();
        let comp = compress_snapshot(json.as_bytes());
        let back = decompress_snapshot(&comp).unwrap();
        assert_eq!(back, json.as_bytes());
        // 손상 스트림 = 에러
        assert!(decompress_snapshot(&[0xFF, 0x00, 0x12]).is_err());
    }

    /// 버전 파일 스캔 — 엄격 파스·정렬·무관 파일 무시
    #[test]
    fn list_snapshot_versions_strict() {
        let dir = tempfile::tempdir().unwrap();
        let s = snap(1, vec![]);
        for name in [
            "app.2.json",
            "app.1.json",
            "app.10.json",
            "app.01.json",  // 선행 0 — 무시
            "app.x.json",   // 숫자 아님 — 무시
            "app..json",    // 빈 버전 — 무시
            "other.3.json", // 다른 db — 무시
            "app.1.sql",    // 확장자 다름 — 무시
        ] {
            s.write_to(&dir.path().join(name)).unwrap();
        }
        let got = list_snapshot_versions(dir.path(), "app").unwrap();
        let versions: Vec<u32> = got.iter().map(|(v, _)| *v).collect();
        assert_eq!(versions, vec![1, 2, 10], "오름차순 + 엄격 파스");

        // 디렉토리 부재 = 빈 목록
        let missing = dir.path().join("없는-디렉토리");
        assert!(list_snapshot_versions(&missing, "app").unwrap().is_empty());
    }

    /// 스냅샷 경로 규칙
    #[test]
    fn snapshot_path_rule() {
        assert_eq!(snapshot_file_name("app_db", 3), "app_db.3.json");
        assert_eq!(
            snapshot_path(Path::new("/x"), "app_db", 3),
            Path::new("/x").join("app_db.3.json")
        );
    }

    /// diff_plan — 동명 컬럼 타입 변경 = 파괴적 재작성 항목 (H-10)
    #[test]
    fn diff_plan_type_change_is_destructive() {
        let old = snap(
            1,
            vec![table("t", vec![col("c", "TEXT", true, false)], vec![])],
        );
        let new = snap(
            2,
            vec![table("t", vec![col("c", "INTEGER", true, false)], vec![])],
        );
        let plan = diff_plan(&old, &new);
        assert!(plan.safe.is_empty(), "{plan:?}");
        assert_eq!(plan.destructive.len(), 1);
        assert!(
            plan.destructive[0].contains("테이블 재작성 필요"),
            "{plan:?}"
        );
        assert!(plan.destructive[0].contains("sql_type"), "{plan:?}");
    }

    /// diff_plan — 유효한 rename 힌트 = 안전 RENAME COLUMN
    #[test]
    fn diff_plan_valid_rename_is_safe() {
        let old = snap(
            1,
            vec![table("t", vec![col("title", "TEXT", true, false)], vec![])],
        );
        let mut renamed = col("subject", "TEXT", true, false);
        renamed.renamed_from = Some("title".into());
        let new = snap(2, vec![table("t", vec![renamed], vec![])]);
        let plan = diff_plan(&old, &new);
        assert_eq!(
            plan.safe,
            vec![r#"ALTER TABLE "t" RENAME COLUMN "title" TO "subject""#.to_string()]
        );
        assert!(plan.destructive.is_empty(), "{plan:?}");
        assert!(plan.warnings.is_empty(), "{plan:?}");
    }

    /// diff_plan — 새 스키마에 원본 컬럼이 남아 있는 rename 힌트 = 무시 + 경고 (H-11)
    #[test]
    fn diff_plan_rename_hint_ignored_when_source_still_exists() {
        let old = snap(
            1,
            vec![table("t", vec![col("title", "TEXT", false, false)], vec![])],
        );
        let mut copied = col("subject", "TEXT", false, false);
        copied.renamed_from = Some("title".into());
        let new = snap(
            2,
            vec![table(
                "t",
                vec![col("title", "TEXT", false, false), copied],
                vec![],
            )],
        );
        let plan = diff_plan(&old, &new);
        assert_eq!(plan.warnings.len(), 1, "{plan:?}");
        assert!(plan.warnings[0].contains("rename 힌트 무시"), "{plan:?}");
        // ADD COLUMN으로 강등 (nullable = 안전)
        assert_eq!(
            plan.safe,
            vec![r#"ALTER TABLE "t" ADD COLUMN "subject" TEXT"#.to_string()]
        );
    }

    /// diff_plan — 옛 스키마에 원본이 없는 rename 힌트 = 경고 + ADD 강등
    #[test]
    fn diff_plan_rename_hint_missing_source_warns() {
        let old = snap(
            1,
            vec![table("t", vec![col("id", "INTEGER", true, true)], vec![])],
        );
        let mut renamed = col("subject", "TEXT", false, false);
        renamed.renamed_from = Some("ghost".into());
        let new = snap(
            2,
            vec![table(
                "t",
                vec![col("id", "INTEGER", true, true), renamed],
                vec![],
            )],
        );
        let plan = diff_plan(&old, &new);
        assert_eq!(plan.warnings.len(), 1, "{plan:?}");
        assert!(plan.warnings[0].contains("잘못된 rename 힌트"), "{plan:?}");
        assert_eq!(
            plan.safe,
            vec![r#"ALTER TABLE "t" ADD COLUMN "subject" TEXT"#.to_string()]
        );
    }

    /// diff_plan — rename 힌트 + 정의 변경 동반 = 파괴적
    #[test]
    fn diff_plan_rename_with_type_change_is_destructive() {
        let old = snap(
            1,
            vec![table("t", vec![col("title", "TEXT", true, false)], vec![])],
        );
        let mut renamed = col("subject", "INTEGER", true, false);
        renamed.renamed_from = Some("title".into());
        let new = snap(2, vec![table("t", vec![renamed], vec![])]);
        let plan = diff_plan(&old, &new);
        assert!(plan.safe.is_empty(), "{plan:?}");
        assert_eq!(plan.destructive.len(), 1, "{plan:?}");
        assert!(
            plan.destructive[0].contains("테이블 재작성 필요"),
            "{plan:?}"
        );
    }

    /// diff_plan — 인덱스 추가 = 안전, 삭제 = 파괴적 (H-10)
    #[test]
    fn diff_plan_index_add_remove() {
        let ct = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY)"#;
        let idx_a = r#"CREATE INDEX IF NOT EXISTS "idx_t_a" ON "t"("a")"#;
        let idx_b = r#"CREATE UNIQUE INDEX "idx_t_b" ON "t"("b")"#;
        let cols = vec![col("id", "INTEGER", true, true)];
        let old = snap(1, vec![table("t", cols.clone(), vec![ct, idx_a])]);
        let new = snap(2, vec![table("t", cols, vec![ct, idx_b])]);
        let plan = diff_plan(&old, &new);
        assert_eq!(plan.safe, vec![idx_b.to_string()], "신규 인덱스 = 안전");
        assert_eq!(
            plan.destructive,
            vec![r#"DROP INDEX "idx_t_a""#.to_string()]
        );
    }

    /// diff_plan — NOT NULL ADD COLUMN(DEFAULT 정보 없음) = 파괴적
    #[test]
    fn diff_plan_not_null_add_is_destructive() {
        let old = snap(
            1,
            vec![table("t", vec![col("id", "INTEGER", true, true)], vec![])],
        );
        let new = snap(
            2,
            vec![table(
                "t",
                vec![
                    col("id", "INTEGER", true, true),
                    col("note", "TEXT", true, false),
                ],
                vec![],
            )],
        );
        let plan = diff_plan(&old, &new);
        assert!(plan.safe.is_empty(), "{plan:?}");
        assert_eq!(plan.destructive.len(), 1, "{plan:?}");
        assert!(plan.destructive[0].contains("DEFAULT 필요"), "{plan:?}");
        assert!(
            plan.destructive[0].contains(r#"ADD COLUMN "note""#),
            "{plan:?}"
        );
    }

    /// diff_plan — 테이블 추가/삭제 분류
    #[test]
    fn diff_plan_table_add_remove() {
        let ct = r#"CREATE TABLE "b" ("id" INTEGER PRIMARY KEY)"#;
        let old = snap(1, vec![table("a", vec![], vec![])]);
        let new = snap(
            2,
            vec![table("b", vec![col("id", "INTEGER", true, true)], vec![ct])],
        );
        let plan = diff_plan(&old, &new);
        assert_eq!(plan.safe, vec![ct.to_string()]);
        assert_eq!(plan.destructive, vec![r#"DROP TABLE "a""#.to_string()]);
    }

    /// diff_plan — 컬럼 동일 + CREATE TABLE 문 변경(UNIQUE 추가) = 파괴적 재작성 (H-5)
    #[test]
    fn diff_plan_constraint_change_is_destructive() {
        let cols = vec![
            col("id", "INTEGER", true, true),
            col("email", "TEXT", true, false),
        ];
        let ct_old = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY, "email" TEXT NOT NULL)"#;
        let ct_new = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY, "email" TEXT NOT NULL UNIQUE)"#;
        let old = snap(1, vec![table("t", cols.clone(), vec![ct_old])]);
        let new = snap(2, vec![table("t", cols, vec![ct_new])]);
        let plan = diff_plan(&old, &new);
        assert!(plan.safe.is_empty(), "{plan:?}");
        assert_eq!(plan.destructive.len(), 1, "{plan:?}");
        assert!(plan.destructive[0].contains("제약/DDL 변경"), "{plan:?}");
    }

    /// diff_plan — DDL 동일(공백만 다름) = 재작성 항목 없음 (H-5)
    #[test]
    fn diff_plan_constraint_same_ddl_no_item() {
        let cols = vec![col("id", "INTEGER", true, true)];
        let ct = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY)"#;
        let ct_ws = "CREATE TABLE  \"t\"\n(\"id\" INTEGER  PRIMARY KEY)";
        let old = snap(1, vec![table("t", cols.clone(), vec![ct])]);
        let new = snap(2, vec![table("t", cols, vec![ct_ws])]);
        let plan = diff_plan(&old, &new);
        assert!(plan.destructive.is_empty(), "공백 정규화: {plan:?}");
        assert!(plan.safe.is_empty(), "{plan:?}");
    }

    /// diff_plan — 컬럼 추가(DDL도 당연히 변경) = 안전 ADD만, 재작성 오탐 없음 (H-5)
    #[test]
    fn diff_plan_column_add_no_false_rewrite() {
        let ct_old = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY)"#;
        let ct_new = r#"CREATE TABLE "t" ("id" INTEGER PRIMARY KEY, "note" TEXT)"#;
        let old = snap(
            1,
            vec![table(
                "t",
                vec![col("id", "INTEGER", true, true)],
                vec![ct_old],
            )],
        );
        let new = snap(
            2,
            vec![table(
                "t",
                vec![
                    col("id", "INTEGER", true, true),
                    col("note", "TEXT", false, false),
                ],
                vec![ct_new],
            )],
        );
        let plan = diff_plan(&old, &new);
        assert_eq!(
            plan.safe,
            vec![r#"ALTER TABLE "t" ADD COLUMN "note" TEXT"#.to_string()]
        );
        assert!(plan.destructive.is_empty(), "재작성 오탐: {plan:?}");
    }

    /// diff_plan — 같은 rename 원본 2회 소비 = 첫 힌트만 RENAME, 둘째는 경고+ADD (M-9)
    #[test]
    fn diff_plan_duplicate_rename_source_degrades() {
        let old = snap(
            1,
            vec![table("t", vec![col("a", "TEXT", false, false)], vec![])],
        );
        let mut b = col("b", "TEXT", false, false);
        b.renamed_from = Some("a".into());
        let mut c = col("c", "TEXT", false, false);
        c.renamed_from = Some("a".into());
        let new = snap(2, vec![table("t", vec![b, c], vec![])]);
        let plan = diff_plan(&old, &new);
        assert_eq!(
            plan.safe,
            vec![
                r#"ALTER TABLE "t" RENAME COLUMN "a" TO "b""#.to_string(),
                r#"ALTER TABLE "t" ADD COLUMN "c" TEXT"#.to_string(),
            ],
            "{plan:?}"
        );
        assert_eq!(plan.warnings.len(), 1, "{plan:?}");
        assert!(plan.warnings[0].contains("rename 힌트 중복"), "{plan:?}");
        // 원본 a 는 첫 rename 이 소비 — DROP 오탐 없음
        assert!(plan.destructive.is_empty(), "{plan:?}");
    }

    /// index_name — 멀티바이트(한글) 이름 panic 없이 파스 (D-1)
    #[test]
    fn index_name_multibyte_no_panic() {
        // 인용 한글 — 종전 &s[..kw.len()] 슬라이스가 char boundary panic
        assert_eq!(
            index_name(r#"CREATE INDEX "할일_idx" ON t(a)"#),
            Some("할일_idx".to_string())
        );
        // 비인용 한글
        assert_eq!(
            index_name("CREATE INDEX 할일 ON t(a)"),
            Some("할일".to_string())
        );
        // 한글 UNIQUE 인덱스 + IF NOT EXISTS 조합
        assert_eq!(
            index_name(r#"CREATE UNIQUE INDEX IF NOT EXISTS "메모 색인" ON t(b)"#),
            Some("메모 색인".to_string())
        );
    }

    /// diff_plan — 문자열 리터럴 내부 공백 변경도 제약 변경으로 감지 (D-2a)
    #[test]
    fn diff_plan_constraint_literal_space_change_detected() {
        let cols = vec![col("a", "TEXT", false, false)];
        let ct_old = r#"CREATE TABLE "t" ("a" TEXT DEFAULT 'a  b')"#;
        let ct_new = r#"CREATE TABLE "t" ("a" TEXT DEFAULT 'a b')"#;
        let old = snap(1, vec![table("t", cols.clone(), vec![ct_old])]);
        let new = snap(2, vec![table("t", cols, vec![ct_new])]);
        let plan = diff_plan(&old, &new);
        assert_eq!(
            plan.destructive.len(),
            1,
            "리터럴 내부 공백 = 실 변경: {plan:?}"
        );
        assert!(plan.destructive[0].contains("제약/DDL 변경"), "{plan:?}");
    }

    /// diff_plan — 한쪽 ddl 빈(구형 스냅샷) = 재작성 오탐 대신 경고 강등 (D-2b)
    #[test]
    fn diff_plan_constraint_empty_ddl_degrades_to_warning() {
        let cols = vec![col("a", "TEXT", false, false)];
        let ct = r#"CREATE TABLE "t" ("a" TEXT)"#;
        let old = snap(1, vec![table("t", cols.clone(), vec![])]);
        let new = snap(2, vec![table("t", cols, vec![ct])]);
        let plan = diff_plan(&old, &new);
        assert!(plan.destructive.is_empty(), "재작성 오탐: {plan:?}");
        assert_eq!(plan.warnings.len(), 1, "{plan:?}");
        assert!(plan.warnings[0].contains("제약 비교 스킵"), "{plan:?}");
    }

    /// normalize_ws_outside_quotes — 인용 밖 접기·인용 안 보존·이스케이프
    #[test]
    fn normalize_ws_quote_aware() {
        assert_eq!(
            normalize_ws_outside_quotes("CREATE  TABLE \"t\"\n(\"a  b\" TEXT DEFAULT 'x  y')"),
            r#"CREATE TABLE "t" ("a  b" TEXT DEFAULT 'x  y')"#
        );
        // '' 이스케이프 사이엔 공백이 없어 결과 동일
        assert_eq!(
            normalize_ws_outside_quotes("DEFAULT  'o''clock  x'"),
            "DEFAULT 'o''clock  x'"
        );
    }

    /// 인덱스 DDL의 인용 밖 공백 차이는 정의 변경이 아니다.
    #[test]
    fn index_diff_ignores_whitespace_outside_quotes() {
        let old = snap(
            1,
            vec![table(
                "items",
                vec![],
                vec!["CREATE  INDEX idx_name\n ON items (name)"],
            )],
        );
        let new = snap(
            2,
            vec![table(
                "items",
                vec![],
                vec!["CREATE INDEX idx_name ON items (name)"],
            )],
        );
        assert!(diff_plan(&old, &new).destructive.is_empty());
    }

    /// 인용 내부 공백 차이는 실제 인덱스 표현식 변경으로 유지한다.
    #[test]
    fn index_diff_preserves_whitespace_inside_quotes() {
        let old = snap(
            1,
            vec![table(
                "items",
                vec![],
                vec!["CREATE INDEX idx_name ON items ('a  b')"],
            )],
        );
        let new = snap(
            2,
            vec![table(
                "items",
                vec![],
                vec!["CREATE INDEX idx_name ON items ('a b')"],
            )],
        );
        assert_eq!(diff_plan(&old, &new).destructive.len(), 1);
    }

    /// index_name — 공백 포함 인용 이름·이스케이프·백틱·대괄호 (M-12)
    #[test]
    fn index_name_quoted_variants() {
        assert_eq!(
            index_name(r#"CREATE INDEX "my idx" ON "t"("a")"#),
            Some("my idx".to_string())
        );
        assert_eq!(
            index_name(r#"CREATE UNIQUE INDEX IF NOT EXISTS "a""b" ON t(x)"#),
            Some("a\"b".to_string())
        );
        assert_eq!(
            index_name("CREATE INDEX `sp ace` ON t(a)"),
            Some("sp ace".to_string())
        );
        assert_eq!(
            index_name("CREATE INDEX [br idx] ON t(a)"),
            Some("br idx".to_string())
        );
        // 비인용 + `name(col)` 붙은 표기
        assert_eq!(
            index_name("create unique index idx_t_a(a)"),
            Some("idx_t_a".to_string())
        );
        // CREATE TABLE = 인덱스 아님, 닫히지 않은 인용부 = None
        assert_eq!(index_name(r#"CREATE TABLE "t" ("id" INTEGER)"#), None);
        assert_eq!(index_name(r#"CREATE INDEX "broken ON t(a)"#), None);
        // INDEXED 같은 접두 확장 키워드 오인 금지
        assert_eq!(index_name("CREATE INDEXED_VIEW x"), None);
    }

    /// list_snapshot_versions — u32 오버플로 파일명 = 침묵 무시 대신 에러 (L-14)
    #[test]
    fn list_snapshot_versions_overflow_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let s = snap(1, vec![]);
        s.write_to(&dir.path().join("app.1.json")).unwrap();
        // u32::MAX = 4294967295 — 그보다 큰 순수 숫자 버전
        s.write_to(&dir.path().join("app.99999999999.json"))
            .unwrap();
        let err = list_snapshot_versions(dir.path(), "app").unwrap_err();
        assert!(err.to_string().contains("u32 범위"), "{err}");
    }

    /// resolve_schema_dir — 상대 env 경로는 manifest 기준 절대화 (M-8)
    #[test]
    fn resolve_schema_dir_relative_env() {
        // SAFETY: env는 프로세스 전역 — 이 크레이트 테스트 중 ROOMRS_SCHEMA_DIR을
        // 조작하는 테스트는 이것뿐이라 경합 없음. 종료 시 원복.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("ROOMRS_SCHEMA_DIR", "custom/schema");
        }
        let got = resolve_schema_dir("/proj");
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("ROOMRS_SCHEMA_DIR");
        }
        assert_eq!(got, Path::new("/proj").join("custom/schema"));
    }

    /// diff_sql — 안전=실행문, 파괴적=TODO 주석, 경고=주석, user_version 마무리
    #[test]
    fn diff_sql_renders_plan() {
        let old = snap(
            1,
            vec![table(
                "t",
                vec![
                    col("id", "INTEGER", true, true),
                    col("gone", "TEXT", false, false),
                ],
                vec![],
            )],
        );
        let new = snap(
            2,
            vec![table(
                "t",
                vec![
                    col("id", "INTEGER", true, true),
                    col("name", "TEXT", false, false),
                ],
                vec![],
            )],
        );
        let sql = diff_sql(&old, &new);
        assert!(
            sql.contains("ALTER TABLE \"t\" ADD COLUMN \"name\" TEXT;"),
            "{sql}"
        );
        assert!(
            sql.contains("-- TODO(파괴적): ALTER TABLE \"t\" DROP COLUMN \"gone\""),
            "{sql}"
        );
        assert!(sql.contains("PRAGMA user_version = 2;"), "{sql}");
    }
}
