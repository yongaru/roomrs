//! 모바일 FFI 패턴 예제 (명세 §13) — extern "C" 표면.
//!
//! Android(JNI)/iOS(Swift)에서 이 cdylib을 로드해 사용한다.
//! 규약: 반환 0 = 성공, 음수 = 에러. 문자열은 UTF-8 NUL 종료, 소유권은 넘기지 않는다.
//! FFI 경계라 `unsafe`가 불가피 — 각 지점에 SAFETY 근거 명시(지침 §4 예외).

use roomrs::{dao, database, entity};
use std::ffi::{CStr, CString, c_char};
use std::sync::Mutex;

#[entity(table = "notes")]
#[derive(Debug, Clone)]
struct Note {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[dao]
trait NoteDao {
    #[insert]
    fn add(&self, n: &Note) -> roomrs::Result<i64>;

    #[query("SELECT COUNT(*) FROM notes")]
    fn count(&self) -> roomrs::Result<i64>;
}

#[database(entities(Note), daos(NoteDao), version = 1)]
struct Db;

/// 전역 DB 핸들 — 모바일 프로세스당 1개 가정 (다중 DB는 핸들 테이블로 확장)
static DB: Mutex<Option<Db>> = Mutex::new(None);

/// DB 락 획득 — extern "C" 경계에서 panic = abort이므로 절대 panic하지 않는다.
/// poison은 무해(보호 대상이 단순 Option 상태뿐)라 into_inner로 복구해 계속 진행 (M-23).
fn db_lock() -> std::sync::MutexGuard<'static, Option<Db>> {
    DB.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// DB 열기 — path는 UTF-8 NUL 종료 경로. 0=성공.
#[unsafe(no_mangle)]
/// # Safety
/// path는 유효한 NUL 종료 UTF-8 포인터여야 한다
pub unsafe extern "C" fn roomrs_open(path: *const c_char) -> i32 {
    if path.is_null() {
        return -1;
    }
    // SAFETY: 호출자(FFI 규약)가 유효한 NUL 종료 포인터를 보장한다
    let cstr = unsafe { CStr::from_ptr(path) };
    let Ok(path) = cstr.to_str() else { return -2 };
    match Db::builder().sqlite(path).build() {
        Ok(db) => {
            *db_lock() = Some(db);
            0
        }
        Err(_) => -3,
    }
}

/// 노트 추가 — 새 rowid 반환(양수), 에러는 음수
#[unsafe(no_mangle)]
/// # Safety
/// body는 유효한 NUL 종료 UTF-8 포인터여야 한다
pub unsafe extern "C" fn roomrs_add_note(body: *const c_char) -> i64 {
    if body.is_null() {
        return -1;
    }
    // SAFETY: 호출자(FFI 규약)가 유효한 NUL 종료 포인터를 보장한다
    let cstr = unsafe { CStr::from_ptr(body) };
    let Ok(body) = cstr.to_str() else { return -2 };
    let guard = db_lock();
    let Some(db) = guard.as_ref() else { return -3 };
    db.run_sync()
        .note_dao()
        .add(&Note {
            id: 0,
            body: body.to_string(),
        })
        .unwrap_or(-4)
}

/// 노트 개수
#[unsafe(no_mangle)]
pub extern "C" fn roomrs_note_count() -> i64 {
    let guard = db_lock();
    let Some(db) = guard.as_ref() else { return -3 };
    db.run_sync().note_dao().count().unwrap_or(-4)
}

/// 마지막 에러 메시지 예시 — 실제 앱은 스레드로컬 에러 버퍼 권장.
/// 반환 문자열은 호출자가 roomrs_string_free로 해제.
#[unsafe(no_mangle)]
pub extern "C" fn roomrs_version() -> *mut c_char {
    CString::new(env!("CARGO_PKG_VERSION"))
        .expect("버전 문자열에 NUL 없음")
        .into_raw()
}

/// roomrs_version 등이 반환한 문자열 해제
#[unsafe(no_mangle)]
/// # Safety
/// s는 roomrs_version이 반환한 포인터여야 하며 1회만 해제한다
pub unsafe extern "C" fn roomrs_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: roomrs_version이 CString::into_raw로 만든 포인터만 전달된다는 FFI 규약
    unsafe {
        drop(CString::from_raw(s));
    }
}

/// DB 닫기
#[unsafe(no_mangle)]
pub extern "C" fn roomrs_close() {
    *db_lock() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FFI 왕복 — open/add/count/close
    #[test]
    fn ffi_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = CString::new(dir.path().join("m.db").to_str().unwrap()).unwrap();
        // SAFETY: 유효한 CString 포인터
        assert_eq!(unsafe { roomrs_open(path.as_ptr()) }, 0);

        let body = CString::new("모바일 노트").unwrap();
        // SAFETY: 유효한 CString 포인터
        assert!(unsafe { roomrs_add_note(body.as_ptr()) } > 0);
        assert_eq!(roomrs_note_count(), 1);

        let v = roomrs_version();
        assert!(!v.is_null());
        // SAFETY: 방금 roomrs_version이 반환한 포인터
        unsafe { roomrs_string_free(v) };
        roomrs_close();
    }
}
