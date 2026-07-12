//! roomrs CLI — `roomrs migrate diff` / `roomrs migrate check` (명세 §8).
//!
//! 파일 기반으로 동작한다. 코드(엔티티) 대조는 런타임 헬퍼
//! `write_schema_snapshot` / `check_schema_snapshot` 이 담당 — CI에서는
//! `cargo test` 로 check 헬퍼를 실행하는 흐름을 권장(문서 참조).
#![deny(unsafe_code)]

use roomrs_migrate::{SchemaSnapshot, diff_plan, diff_sql, list_snapshot_versions};
use std::path::Path;
use std::process::ExitCode;

/// 진입점 — 서브커맨드 분기
fn main() -> ExitCode {
    // args()는 비 UTF-8 인자에서 panic — args_os로 받아 한국어 에러로 처리 (L-18)
    let mut args: Vec<String> = Vec::new();
    for a in std::env::args_os().skip(1) {
        match a.into_string() {
            Ok(s) => args.push(s),
            Err(bad) => {
                eprintln!("인자가 유효한 UTF-8이 아닙니다: {}", bad.to_string_lossy());
                return ExitCode::from(2);
            }
        }
    }
    // --strict: 경고(버전 갭·파괴적 변경)를 실패(exit 1)로 승격 (M-16)
    let strict = args.iter().any(|a| a == "--strict");
    let strs: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--strict")
        .collect();

    match strs.as_slice() {
        ["migrate", "diff", old, new] => cmd_diff(old, new, None),
        ["migrate", "diff", old, new, out] => cmd_diff(old, new, Some(out)),
        ["migrate", "check", a, b] => cmd_check(a, b),
        ["migrate", "check-dir", dir, db] => cmd_check_dir(dir, db, strict),
        _ => {
            eprintln!(
                "사용법:\n  roomrs migrate diff <old.json> <new.json> [out.sql]      # 초안 생성\n  roomrs migrate check <a.json> <b.json>                     # 스냅샷 해시 비교\n  roomrs migrate check-dir <schema_dir> <db이름> [--strict]  # 버전 파일 스캔 검증\n                                                             #   --strict: 경고(버전 갭·파괴적 변경)를 exit 1로 승격"
            );
            ExitCode::from(2)
        }
    }
}

/// 스냅샷 로드 (에러 = 한국어 메시지 출력)
fn load(path: &str) -> Result<SchemaSnapshot, ExitCode> {
    SchemaSnapshot::read_from(Path::new(path)).map_err(|e| {
        eprintln!("스냅샷을 읽을 수 없습니다 ({path}): {e}");
        ExitCode::FAILURE
    })
}

/// diff 초안 생성 — stdout 또는 파일
fn cmd_diff(old: &str, new: &str, out: Option<&str>) -> ExitCode {
    let (old, new) = match (load(old), load(new)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(c), _) | (_, Err(c)) => return c,
    };
    let sql = diff_sql(&old, &new);
    match out {
        None => {
            print!("{sql}");
            ExitCode::SUCCESS
        }
        // create_new = 원자적 존재 검사 — 검토 중인 기존 초안을 절대 덮어쓰지 않는다 (L-18)
        Some(path) => match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut f) => match std::io::Write::write_all(&mut f, sql.as_bytes()) {
                Ok(()) => {
                    eprintln!("초안 저장: {path}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("초안 저장 실패 ({path}): {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                eprintln!(
                    "초안 파일이 이미 존재합니다 ({path}) — 덮어쓰지 않습니다. 확인 후 삭제하고 다시 실행하세요"
                );
                ExitCode::FAILURE
            }
            Err(e) => {
                eprintln!("초안 저장 실패 ({path}): {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// 버전 파일 디렉토리 검사 (명세 §8.4) — `{db}.{N}.json` 스캔 후
/// 파스·버전 정합성 검증 + 버전 갭·연속 쌍 diff의 파괴적 항목을 경고로 보고.
/// 성공=0(경고는 실패 아님), 스냅샷 없음/파손=1.
/// strict = true 면 경고 1건 이상일 때 exit 1 (M-16)
fn cmd_check_dir(dir: &str, db: &str, strict: bool) -> ExitCode {
    let files = match list_snapshot_versions(Path::new(dir), db) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("스냅샷 디렉토리를 읽을 수 없습니다 ({dir}): {e}");
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        eprintln!("스냅샷이 없습니다: {dir} 에 {db}.<버전>.json 파일 없음");
        return ExitCode::FAILURE;
    }

    // 전 파일 파스 + 파일명 버전 ↔ 내부 version 정합성
    let mut snaps: Vec<SchemaSnapshot> = Vec::with_capacity(files.len());
    for (ver, path) in &files {
        let s = match SchemaSnapshot::read_from(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("스냅샷을 읽을 수 없습니다 ({}): {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        if s.version != *ver {
            eprintln!(
                "스냅샷 내부 version({})이 파일명 버전({ver})과 다릅니다: {}",
                s.version,
                path.display()
            );
            return ExitCode::FAILURE;
        }
        snaps.push(s);
    }

    // 연속 쌍 diff — 버전 갭·파괴적 항목·경고는 stderr 경고 (기본은 실패 아님)
    let mut warn_count: usize = 0;
    for pair in snaps.windows(2) {
        // 버전 갭 — v1 다음 v3 처럼 중간 스냅샷이 빠진 구간 (M-16).
        // 내장 자동 마이그레이션은 연속 스냅샷 diff로 동작하므로 갭 구간은 커버 불가
        if pair[1].version - pair[0].version > 1 {
            eprintln!(
                "경고: 버전 갭: v{} 다음 v{} — 중간 스냅샷 누락(내장 자동 마이그레이션 불가 구간)",
                pair[0].version, pair[1].version
            );
            warn_count += 1;
        }
        let plan = diff_plan(&pair[0], &pair[1]);
        for d in &plan.destructive {
            eprintln!(
                "경고: v{} -> v{} 파괴적 변경: {d}",
                pair[0].version, pair[1].version
            );
            warn_count += 1;
        }
        for w in &plan.warnings {
            eprintln!("경고: v{} -> v{}: {w}", pair[0].version, pair[1].version);
            warn_count += 1;
        }
    }

    // 요약 — 파일명 규칙상 버전 오름차순·중복 불가
    eprintln!(
        "스냅샷 {}개 확인: v{}..v{} ({db})",
        files.len(),
        files.first().map(|(v, _)| *v).unwrap_or(0),
        files.last().map(|(v, _)| *v).unwrap_or(0)
    );
    // --strict: 경고를 실패로 승격 (M-16)
    if strict && warn_count > 0 {
        eprintln!("--strict: 경고 {warn_count}건 — 실패로 처리합니다");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// 두 스냅샷 해시 비교 — 일치=0, 불일치=1
fn cmd_check(a: &str, b: &str) -> ExitCode {
    let (a, b) = match (load(a), load(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(c), _) | (_, Err(c)) => return c,
    };
    if a.hash() == b.hash() {
        eprintln!("일치");
        ExitCode::SUCCESS
    } else {
        eprintln!("불일치 — 스냅샷 재생성 또는 마이그레이션 필요");
        ExitCode::FAILURE
    }
}
