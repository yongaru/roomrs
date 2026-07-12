// [명세 A-4] 필드 속성 표기 충돌 실측:
//   #[pk] · #[json] · #[column(ignore)] 가 내장 속성/derive 헬퍼와 충돌하지 않아야 한다.
//   #[entity] 는 #[derive] 보다 먼저 전개되어 보조 속성을 소비한다.
use roomrs::entity;

#[entity(table = "users")]
#[derive(Debug, Clone, Default)]
struct User {
    #[pk(autoincrement)]
    id: i64,
    #[json]
    prefs: String,
    #[column(ignore)]
    transient: Option<String>,
}

// 필드 속성이 제거되고 derive가 정상 동작하는지 확인
fn main() {
    let u = User { id: 1, prefs: String::new(), transient: None };
    let d = User::default();
    let _ = format!("{u:?} {d:?}");
    let _ = u.clone();
    // 엔티티 메타 생성 확인
    assert_eq!(<User as roomrs::Entity>::TABLE, "users");
}
