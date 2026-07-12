// M1b 검증 통합 테스트 (명세 §15 M1b) —
// 실행기 3종(tokio/smol/futures) CRUD · +Send(tokio::spawn) · 취소 안전성
#![cfg(feature = "async")]

use roomrs::{BuildAsyncExt, MigrationPolicy, dao, database, entity};

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

    #[query("SELECT * FROM todos WHERE id = :id")]
    fn find(&self, id: i64) -> roomrs::Result<Option<Todo>>;

    #[query("SELECT * FROM todos WHERE done = :done ORDER BY id")]
    fn by_done(&self, done: bool) -> roomrs::Result<Vec<Todo>>;

    #[update("UPDATE todos SET done = :done WHERE id = :id")]
    fn set_done(&self, id: i64, done: bool) -> roomrs::Result<u64>;

    #[delete("DELETE FROM todos WHERE id = :id")]
    fn remove(&self, id: i64) -> roomrs::Result<u64>;
}

#[database(entities(Todo), daos(TodoDao), version = 1)]
struct Db;

/// 비동기 CRUD 왕복 — 실행기 무관 공통 본체
async fn crud_roundtrip(dir: &tempfile::TempDir) -> roomrs::Result<()> {
    let db = Db::builder()
        .sqlite(dir.path().join("a.db"))
        .migrate(MigrationPolicy::Auto)
        .build_async()
        .await?;
    let h = db.run_async();
    let dao = h.todo_dao();

    let id = dao
        .add(&Todo {
            id: 0,
            title: "비동기".into(),
            done: false,
        })
        .await?;
    assert!(id > 0);

    let t = dao.find(id).await?.expect("존재해야 함");
    assert_eq!(t.title, "비동기");

    dao.set_done(id, true).await?;
    assert!(dao.find(id).await?.unwrap().done);
    assert_eq!(dao.by_done(false).await?.len(), 0);

    // 직접 쿼리 API (비동기 대칭)
    let cnt: i64 = h.query_scalar("SELECT COUNT(*) FROM todos", ()).await?;
    assert_eq!(cnt, 1);

    // 비동기 트랜잭션 — 동기 클로저형 (명세 A-5)
    use DbTxDaos as _;
    h.transaction(|tx| {
        tx.todo_dao().add(&Todo {
            id: 0,
            title: "tx".into(),
            done: false,
        })?;
        Ok(())
    })
    .await?;
    let cnt: i64 = h.query_scalar("SELECT COUNT(*) FROM todos", ()).await?;
    assert_eq!(cnt, 2);

    dao.remove(id).await?;
    assert!(dao.find(id).await?.is_none());
    Ok(())
}

/// 실행기 1 — tokio
#[test]
fn executor_tokio() {
    let dir = tempfile::tempdir().unwrap();
    tokio::runtime::Builder::new_multi_thread()
        .build()
        .unwrap()
        .block_on(crud_roundtrip(&dir))
        .unwrap();
}

/// 실행기 2 — smol.
/// feature `tokio`에서도 런타임 밖이면 자체 풀 폴백으로 동작(H-6) —
/// 폴백 경로는 tokio_feature_works_outside_runtime 이 검증, 여기선 순수 경로만
#[cfg(not(feature = "tokio"))]
#[test]
fn executor_smol() {
    let dir = tempfile::tempdir().unwrap();
    smol::block_on(crud_roundtrip(&dir)).unwrap();
}

/// 실행기 3 — futures::executor
#[cfg(not(feature = "tokio"))]
#[test]
fn executor_futures() {
    let dir = tempfile::tempdir().unwrap();
    futures::executor::block_on(crud_roundtrip(&dir)).unwrap();
}

/// 생성 Future가 Send — async move 블록째 tokio::spawn 가능 [명세 B-3]
#[test]
fn futures_are_send_spawnable() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread().build().unwrap();
    rt.block_on(async {
        let db = Db::builder()
            .sqlite(dir.path().join("s.db"))
            .build_async()
            .await
            .unwrap();
        let handle = tokio::spawn(async move {
            let h = db.run_async();
            h.todo_dao()
                .add(&Todo {
                    id: 0,
                    title: "spawned".into(),
                    done: false,
                })
                .await
        });
        let id = handle.await.expect("join").expect("insert");
        assert!(id > 0);
    });
}

/// 생성 DAO Future 자체가 'static이라 DAO를 먼저 drop하고 직접 spawn 가능하다.
#[cfg(feature = "tokio")]
#[test]
fn generated_dao_future_is_directly_spawnable() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let db = Db::builder()
            .sqlite(dir.path().join("direct-spawn.db"))
            .build_async()
            .await
            .unwrap();
        let dao = db.run_async().todo_dao();
        let future = dao.find(1);
        drop(dao);
        let _ = tokio::spawn(future).await.unwrap().unwrap();
    });
}

/// 패닉하는 클로저 격리 (H-5) — Err(Internal) 반환, 워커 스레드 생존, 이후 호출 정상.
/// 워커 수(기본 max(4, 코어))보다 많은 64회 패닉으로 풀 전멸이 없음을 확인
#[cfg(not(feature = "tokio"))]
#[test]
fn panicking_job_is_isolated() {
    let dir = tempfile::tempdir().unwrap();
    futures::executor::block_on(async {
        let db = Db::builder()
            .sqlite(dir.path().join("p.db"))
            .migrate(MigrationPolicy::Auto)
            .build_async()
            .await
            .unwrap();
        let h = db.run_async();

        // 패닉 클로저 — 워커에서 격리되고 호출측은 Err
        for _ in 0..64 {
            let res: roomrs::Result<i64> = h.run(|_| panic!("테스트 패닉")).await;
            assert!(matches!(res, Err(roomrs::Error::Internal(_))));
        }

        // 풀 생존 확인 — 이후 정상 동작
        let id = h
            .todo_dao()
            .add(&Todo {
                id: 0,
                title: "생존".into(),
                done: false,
            })
            .await
            .unwrap();
        assert!(id > 0);
    });
}

/// tokio feature + tokio 런타임 밖 (H-6) — 자체 풀 폴백으로 정상 동작.
/// feature 가산성 검증: 의존성이 tokio를 켜도 futures::executor 사용자가 깨지지 않는다
#[cfg(feature = "tokio")]
#[test]
fn tokio_feature_works_outside_runtime() {
    let dir = tempfile::tempdir().unwrap();
    futures::executor::block_on(crud_roundtrip(&dir)).unwrap();
}

/// tokio 경로 패닉 격리 (L-10) — spawn_blocking 안 패닉 시 JoinHandle은 이미 버려졌고
/// oneshot 송신단 drop → 호출측 Err(Internal). tokio 블로킹 풀은 패닉을 삼키므로 이후 정상
#[cfg(feature = "tokio")]
#[test]
fn tokio_panicking_job_is_isolated() {
    let dir = tempfile::tempdir().unwrap();
    tokio::runtime::Builder::new_multi_thread()
        .build()
        .unwrap()
        .block_on(async {
            let db = Db::builder()
                .sqlite(dir.path().join("tp.db"))
                .migrate(MigrationPolicy::Auto)
                .build_async()
                .await
                .unwrap();
            let h = db.run_async();

            // 패닉 클로저 — spawn_blocking 경로에서 격리되고 호출측은 Err
            for _ in 0..16 {
                let res: roomrs::Result<i64> = h.run(|_| panic!("테스트 패닉")).await;
                assert!(matches!(res, Err(roomrs::Error::Internal(_))));
            }

            // 이후 정상 동작 확인
            let id = h
                .todo_dao()
                .add(&Todo {
                    id: 0,
                    title: "생존".into(),
                    done: false,
                })
                .await
                .unwrap();
            assert!(id > 0);
        });
}

/// tokio 경로 Future 취소 안전성 (L-10) — 첫 poll 후 drop해도 쓰기는 워커에서 완료,
/// 결과만 폐기. DB 일관성 유지 + 이후 사용 정상 (cancelled_future_is_safe 대칭)
#[cfg(feature = "tokio")]
#[test]
fn tokio_cancelled_future_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    tokio::runtime::Builder::new_multi_thread()
        .build()
        .unwrap()
        .block_on(async {
            let db = Db::builder()
                .sqlite(dir.path().join("tc.db"))
                .build_async()
                .await
                .unwrap();
            let h = db.run_async();

            // poll 없이 즉시 drop — 작업 미제출, 아무 일도 안 일어남
            drop(h.todo_dao().add(&Todo {
                id: 0,
                title: "취소0".into(),
                done: false,
            }));

            // 첫 poll 후 drop — 작업은 제출됐고 워커에서 완료, 결과만 폐기
            {
                let dao = h.todo_dao();
                let todo = Todo {
                    id: 0,
                    title: "취소1".into(),
                    done: false,
                };
                let fut = dao.add(&todo);
                futures::pin_mut!(fut);
                let _ = futures::poll!(fut.as_mut());
            } // 스코프 종료 = drop(취소)

            // 취소된 쓰기도 완료된다 — count 1 도달 대기 (최대 ~2초)
            let mut landed = false;
            for _ in 0..200 {
                let cnt: i64 = h
                    .query_scalar("SELECT COUNT(*) FROM todos", ())
                    .await
                    .unwrap();
                if cnt == 1 {
                    landed = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(landed, "취소된 쓰기가 완료되지 않음");

            // 이후 정상 동작 + DB 일관성 확인
            let id = h
                .todo_dao()
                .add(&Todo {
                    id: 0,
                    title: "생존".into(),
                    done: false,
                })
                .await
                .unwrap();
            assert!(id > 0);
            let cnt: i64 = h
                .query_scalar("SELECT COUNT(*) FROM todos", ())
                .await
                .unwrap();
            assert_eq!(cnt, 2);
        });
}

/// Future 취소(drop) 안전성 — 작업은 완료되고 결과만 폐기, 이후 사용 정상
#[cfg(not(feature = "tokio"))]
#[test]
fn cancelled_future_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    smol::block_on(async {
        let db = Db::builder()
            .sqlite(dir.path().join("c.db"))
            .build_async()
            .await
            .unwrap();
        let h = db.run_async();

        // poll 없이 즉시 drop — 시작 전 취소
        drop(h.todo_dao().add(&Todo {
            id: 0,
            title: "취소1".into(),
            done: false,
        }));

        // 이후 정상 동작 확인
        let id = h
            .todo_dao()
            .add(&Todo {
                id: 0,
                title: "생존".into(),
                done: false,
            })
            .await
            .unwrap();
        assert!(id > 0);
    });
}
