//! 관계 매핑 (명세 결정 로그 7) — `#[derive(Relation)]` 생성물이 구현하는 trait

use crate::error::Result;
use crate::handle::SqlContext;

/// 관계 뷰 — 부모 엔티티 + 자식 컬렉션 조립 (Room @Embedded+@Relation 대응).
/// `#[derive(Relation)]`이 구현을 생성한다.
pub trait RelationView: Sized {
    /// `#[embedded]` 필드의 부모 엔티티 타입
    type Parent: crate::row::FromRow;

    /// 부모 목록에 자식들을 일괄 로딩해 조립 — IN 쿼리로 N+1 회피.
    /// `#[query(with_relations, …)]`가 자동 트랜잭션 안에서 호출한다.
    fn load<C: SqlContext>(cx: &C, parents: Vec<Self::Parent>) -> Result<Vec<Self>>;
}

/// IN 절 플레이스홀더 생성 (`?1, ?2, …`) — derive 생성 코드 전용
#[doc(hidden)]
pub fn in_placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

use rusqlite::ToSql;
use std::collections::HashMap;
use std::hash::Hash;

/// SQLite 빌드별 변수 한도보다 보수적인 IN 절 청크 크기.
const IN_CHUNK_SIZE: usize = 999;

/// 1:N/1:1 자식 일괄 로딩 — 부모 키 IN 조회 후 자식 키로 그룹핑.
/// derive 생성 코드 전용 (클로저가 키 타입을 고정한다).
#[doc(hidden)]
pub fn load_children<PK, C, CX, F>(
    cx: &CX,
    keys: &[PK],
    child_table: &str,
    key_col: &str,
    key_of: F,
) -> Result<HashMap<PK, Vec<C>>>
where
    PK: ToSql + Eq + Hash + Clone,
    C: crate::row::FromRow,
    CX: SqlContext,
    F: Fn(&C) -> PK,
{
    let mut map: HashMap<PK, Vec<C>> = HashMap::new();
    if keys.is_empty() {
        return Ok(map);
    }
    for chunk in keys.chunks(IN_CHUNK_SIZE) {
        let sql = format!(
            "SELECT * FROM \"{child_table}\" WHERE \"{key_col}\" IN ({})",
            in_placeholders(chunk.len())
        );
        let refs: Vec<&dyn ToSql> = chunk.iter().map(|k| k as &dyn ToSql).collect();
        let children: Vec<C> = cx.ctx_query_all(&sql, &refs[..])?;
        for c in children {
            map.entry(key_of(&c)).or_default().push(c);
        }
    }
    Ok(map)
}

/// N:M 자식 일괄 로딩 — 정션 페어 조회 → 자식 IN 조회 → 조립 (2쿼리, N+1 회피).
/// derive 생성 코드 전용.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn load_junction<PK, EK, C, CX, F>(
    cx: &CX,
    keys: &[PK],
    junction_table: &str,
    j_parent_col: &str,
    j_entity_col: &str,
    child_table: &str,
    child_key_col: &str,
    key_of: F,
) -> Result<HashMap<PK, Vec<C>>>
where
    PK: ToSql + rusqlite::types::FromSql + Eq + Hash + Clone,
    EK: ToSql + rusqlite::types::FromSql + Eq + Hash + Clone + Ord,
    C: crate::row::FromRow + Clone,
    CX: SqlContext,
    F: Fn(&C) -> EK,
{
    let mut map: HashMap<PK, Vec<C>> = HashMap::new();
    if keys.is_empty() {
        return Ok(map);
    }
    // 1) 정션 페어
    let mut pairs: Vec<(PK, EK)> = Vec::new();
    for chunk in keys.chunks(IN_CHUNK_SIZE) {
        let jsql = format!(
            "SELECT \"{j_parent_col}\", \"{j_entity_col}\" FROM \"{junction_table}\" \
             WHERE \"{j_parent_col}\" IN ({})",
            in_placeholders(chunk.len())
        );
        let refs: Vec<&dyn ToSql> = chunk.iter().map(|k| k as &dyn ToSql).collect();
        pairs.extend(cx.ctx_query_all::<(PK, EK), _>(&jsql, &refs[..])?);
    }

    // 2) 자식 일괄
    let mut child_keys: Vec<EK> = pairs.iter().map(|(_, e)| e.clone()).collect();
    child_keys.sort();
    child_keys.dedup();
    let mut by_key: HashMap<EK, C> = HashMap::new();
    if !child_keys.is_empty() {
        for chunk in child_keys.chunks(IN_CHUNK_SIZE) {
            let csql = format!(
                "SELECT * FROM \"{child_table}\" WHERE \"{child_key_col}\" IN ({})",
                in_placeholders(chunk.len())
            );
            let crefs: Vec<&dyn ToSql> = chunk.iter().map(|k| k as &dyn ToSql).collect();
            let children: Vec<C> = cx.ctx_query_all(&csql, &crefs[..])?;
            for c in children {
                by_key.insert(key_of(&c), c);
            }
        }
    }

    // 3) 페어 따라 조립
    for (p, e) in pairs {
        if let Some(c) = by_key.get(&e) {
            map.entry(p).or_default().push(c.clone());
        }
    }
    Ok(map)
}
