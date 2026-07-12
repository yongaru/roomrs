//! 마이그레이션 스텝 · 체인 러너 (명세 §8, 결정 로그 6)

use crate::error::{Error, Result};
use crate::handle::Tx;

/// 마이그레이션 실행 클로저 타입
type UpFn = Box<dyn Fn(&Tx<'_>) -> Result<()> + Send + Sync>;

/// 수동 코드 스텝 trait — (from,to) 쌍 모델 [C-3] (명세 §8.3)
pub trait MigrationStep: Send + Sync + 'static {
    /// 시작 버전 (명세 [C-3] 확정 이름 — 관례 린트 예외)
    #[allow(clippy::wrong_self_convention)]
    fn from_version(&self) -> u32;
    /// 도착 버전
    fn to_version(&self) -> u32;
    /// 정방향 실행 — 트랜잭션 안에서 호출된다
    fn up(&self, tx: &Tx<'_>) -> Result<()>;
    /// 역방향(선택) — v1 러너는 up만 실행. down은 사용자 수동 실행용.
    fn down(&self, _tx: &Tx<'_>) -> Result<()> {
        Err(Error::Migration(
            "down 마이그레이션이 구현되지 않았습니다".into(),
        ))
    }
}

/// 마이그레이션 스텝 — 세 소스(SQL 파일/인라인 SQL/코드)의 공통 표현 (명세 §8.2)
pub struct Migration {
    from: u32,
    to: u32,
    up: UpFn,
}

impl Migration {
    /// 인라인 SQL 스텝 — `Migration::sql(1, 2, "ALTER TABLE …")` (명세 §8.2)
    pub fn sql(from: u32, to: u32, sql: impl Into<String>) -> Self {
        let sql = sql.into();
        Self::code(from, to, move |tx| tx.execute_batch(&sql))
    }

    /// 여러 문장 SQL 스텝 — execute_batch와 동일(별칭, 의도 명시용)
    pub fn sql_batch(from: u32, to: u32, sql: impl Into<String>) -> Self {
        Self::sql(from, to, sql)
    }

    /// 코드 스텝
    pub fn code(
        from: u32,
        to: u32,
        f: impl Fn(&Tx<'_>) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            from,
            to,
            up: Box::new(f),
        }
    }

    /// trait 구현체 래핑 (명세 §8.3)
    #[allow(clippy::self_named_constructors, clippy::wrong_self_convention)]
    pub fn from_step(step: impl MigrationStep) -> Self {
        let step = std::sync::Arc::new(step);
        Self {
            from: step.from_version(),
            to: step.to_version(),
            up: Box::new(move |tx| step.up(tx)),
        }
    }

    /// 시작 버전 (명세 [C-3] 확정 이름)
    #[allow(clippy::wrong_self_convention)]
    pub fn from_version(&self) -> u32 {
        self.from
    }

    /// 도착 버전
    pub fn to_version(&self) -> u32 {
        self.to
    }

    /// 정방향 실행 — 러너 전용
    pub(crate) fn run_up(&self, tx: &Tx<'_>) -> Result<()> {
        (self.up)(tx)
    }
}

/// 체인 검증 + 실행 계획 — current에서 target까지의 스텝 나열.
/// 중복 구간 = 에러, 갭 = `Err(None 계획)` 대신 명확한 에러 메시지 반환.
/// 참조 슬라이스를 받는다 — 등록 스텝 + 합성 스텝(명세 §8.4)을 복제 없이 합친다.
pub(crate) fn plan_chain<'a>(
    steps: &[&'a Migration],
    current: u32,
    target: u32,
) -> Result<Vec<&'a Migration>> {
    // 유효성: to > from
    for s in steps {
        if s.to <= s.from {
            return Err(Error::Migration(format!(
                "잘못된 마이그레이션 구간: {} -> {} (to는 from보다 커야 함)",
                s.from, s.to
            )));
        }
    }
    // 같은 from 중복 = 에러 (명세 §8.3 같은 구간 중복)
    let mut froms: Vec<u32> = steps.iter().map(|s| s.from).collect();
    froms.sort_unstable();
    if let Some(w) = froms.windows(2).find(|w| w[0] == w[1]) {
        return Err(Error::Migration(format!(
            "중복 마이그레이션 구간: from={} 스텝이 2개 이상",
            w[0]
        )));
    }

    if current > target {
        return Err(Error::Migration(format!(
            "다운그레이드는 지원하지 않습니다 (DB={current}, 코드={target}) — down은 수동 실행"
        )));
    }

    // 그리디 체인 — from==v 스텝 순차 적용
    let mut plan = Vec::new();
    let mut v = current;
    while v < target {
        let Some(step) = steps.iter().copied().find(|s| s.from == v) else {
            return Err(Error::Migration(format!(
                "마이그레이션 체인 갭: v{v} -> v{target} 구간을 잇는 스텝이 없습니다"
            )));
        };
        if step.to > target {
            return Err(Error::Migration(format!(
                "마이그레이션 스텝이 목표를 지나칩니다: {} -> {} (목표 {target})",
                step.from, step.to
            )));
        }
        v = step.to;
        plan.push(step);
    }
    Ok(plan)
}

// Tx::execute_batch는 handle.rs로 이동 (M-2) — 배치 write 무효화 수집 경로 공유

#[cfg(test)]
mod tests {
    use super::*;

    /// 체인 계획 — 정상/갭/중복/다운그레이드
    #[test]
    fn chain_plan() {
        let owned = [
            Migration::sql(1, 2, "SELECT 1"),
            Migration::sql(2, 3, "SELECT 1"),
        ];
        let steps: Vec<&Migration> = owned.iter().collect();
        assert_eq!(plan_chain(&steps, 1, 3).unwrap().len(), 2);
        assert_eq!(plan_chain(&steps, 2, 3).unwrap().len(), 1);
        assert_eq!(plan_chain(&steps, 3, 3).unwrap().len(), 0);
        assert!(plan_chain(&steps, 0, 3).is_err(), "갭(0->1)");
        assert!(plan_chain(&steps, 3, 1).is_err(), "다운그레이드");

        let dup_owned = [Migration::sql(1, 2, ""), Migration::sql(1, 3, "")];
        let dup: Vec<&Migration> = dup_owned.iter().collect();
        assert!(plan_chain(&dup, 1, 3).is_err(), "중복 from");
    }
}
