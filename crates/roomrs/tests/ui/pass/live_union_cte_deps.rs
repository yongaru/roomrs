// [H-9] UNION/CTE watch 쿼리도 DEPENDS_ON 이 정확히 수집되는지 실측.
// 매크로가 의존 테이블(todos)을 못 넘기면 core 자체 추출도 UNION/CTE 에서
// 실패해 첫 recv 가 UnknownDependencies 에러가 된다 — 성공 = 매크로 수집 증명.
use roomrs::{LiveQuery, dao, database, entity, params};
use std::time::Duration;

#[entity(table = "todos")]
#[derive(Debug, Clone, PartialEq)]
struct Todo {
    #[pk(autoincrement)]
    id: i64,
    title: String,
    done: bool,
}

#[dao]
trait TodoDao {
    #[insert]
    fn add(&self, t: &Todo) -> roomrs::Result<i64>;

    // UNION — SetOperation 좌우 재귀 수집 (H-9)
    #[query("SELECT * FROM todos WHERE done = :done UNION SELECT * FROM todos WHERE id = :id")]
    fn watch_union(&self, done: bool, id: i64) -> LiveQuery<Vec<Todo>>;

    // CTE — WITH 본문 재귀 수집 + CTE 이름(recent) 제외 (H-9)
    #[query("WITH recent AS (SELECT * FROM todos) SELECT * FROM recent WHERE done = :done")]
    fn watch_cte(&self, done: bool) -> LiveQuery<Vec<Todo>>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

/// emit 대기 헬퍼 — UnknownDependencies 면 recv 가 Err 를 반환한다
fn next(q: &LiveQuery<Vec<Todo>>) -> Vec<Todo> {
    q.recv_timeout(Duration::from_secs(2))
        .expect("의존 테이블 수집 실패 (UnknownDependencies?)")
        .expect("emit 타임아웃")
}

/// 기대 행 수 수렴 대기 — 과잉 emit 흡수
fn wait_len(q: &LiveQuery<Vec<Todo>>, expected: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut last = usize::MAX;
    while std::time::Instant::now() < deadline {
        match q.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(v)) => {
                if v.len() == expected {
                    return;
                }
                last = v.len();
            }
            Ok(None) => {}
            Err(e) => panic!("수신 에러: {e}"),
        }
    }
    panic!("기대 행 수 {expected} 미도달 (마지막: {last})");
}

fn main() {
    let db = Db::builder().in_memory().build().expect("빌드");
    let h = db.run_sync();
    let dao = h.todo_dao();

    let union_q = dao.watch_union(false, 1);
    let cte_q = dao.watch_cte(false);

    // 구독 즉시 1회 emit — UnknownDependencies 였다면 여기서 Err
    assert_eq!(next(&union_q).len(), 0, "UNION 초기 emit");
    assert_eq!(next(&cte_q).len(), 0, "CTE 초기 emit");

    // todos write → 두 쿼리 모두 재조회 emit = DEPENDS_ON 에 todos 포함 증명
    h.execute(
        "INSERT INTO todos (title, done) VALUES ('a', 0)",
        params![],
    )
    .expect("insert");
    wait_len(&union_q, 1);
    wait_len(&cte_q, 1);
}
