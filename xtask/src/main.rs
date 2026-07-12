//! xtask — 개발 태스크 러너.
//!
//! 크로스 빌드 (zig/NDK):
//!   cargo xtask cross-linux     # x86_64/aarch64-unknown-linux-gnu + x86_64-musl (cargo-zigbuild)
//!   cargo xtask cross-android   # arm64-v8a/armeabi-v7a/x86_64 .so (cargo-ndk)
//!   cargo xtask cross-all       # 위 전부
//!
//! 요구 도구: zig, cargo-zigbuild, cargo-ndk, Android NDK(ANDROID_HOME 또는 ANDROID_NDK_HOME)
#![deny(unsafe_code)]

use std::process::{Command, ExitCode};

/// 리눅스 zig 타깃 (gnu 2종 + 정적 musl)
const LINUX_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
];

/// Android ABI (cargo-ndk 표기)
const ANDROID_ABIS: &[&str] = &["arm64-v8a", "armeabi-v7a", "x86_64"];

/// 진입점 — 서브커맨드 분기
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("cross-linux") => cross_linux(),
        Some("cross-android") => cross_android(),
        Some("cross-all") => {
            let a = cross_linux();
            if a != ExitCode::SUCCESS {
                return a;
            }
            cross_android()
        }
        _ => {
            eprintln!(
                "사용법: cargo xtask <cross-linux | cross-android | cross-all>\n\
                 요구 도구: zig + cargo-zigbuild (리눅스), cargo-ndk + NDK (안드로이드)"
            );
            ExitCode::from(2)
        }
    }
}

/// 명령 실행 — 실패 시 즉시 에러 코드
fn run(desc: &str, cmd: &mut Command) -> ExitCode {
    eprintln!("[xtask] {desc}");
    match cmd.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => {
            eprintln!("[xtask] 실패: {desc} (exit {s})");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("[xtask] 실행 불가: {desc} — {e} (도구 설치 확인)");
            ExitCode::FAILURE
        }
    }
}

/// 리눅스 크로스 빌드 — cargo-zigbuild (zig를 cc/링커로 사용, bundled SQLite 포함)
fn cross_linux() -> ExitCode {
    for target in LINUX_TARGETS {
        // musl은 cdylib 대신 CLI 정적 바이너리 검증에 사용
        let packages: &[&str] = if target.contains("musl") {
            &["roomrs", "roomrs-cli"]
        } else {
            &["roomrs", "roomrs-mobile-ffi-example"]
        };
        let mut cmd = Command::new("cargo");
        cmd.args(["zigbuild", "--target", target]);
        for p in packages {
            cmd.args(["-p", p]);
        }
        let code = run(&format!("zigbuild {target}"), &mut cmd);
        if code != ExitCode::SUCCESS {
            return code;
        }
    }
    eprintln!("[xtask] 리눅스 크로스 빌드 완료: {LINUX_TARGETS:?}");
    ExitCode::SUCCESS
}

/// Android 크로스 빌드 — cargo-ndk (bionic 헤더가 필요해 zig 단독 불가)
fn cross_android() -> ExitCode {
    let mut cmd = Command::new("cargo");
    cmd.arg("ndk");
    for abi in ANDROID_ABIS {
        cmd.args(["-t", abi]);
    }
    cmd.args(["build", "-p", "roomrs-mobile-ffi-example"]);
    let code = run(&format!("cargo ndk {ANDROID_ABIS:?}"), &mut cmd);
    if code == ExitCode::SUCCESS {
        eprintln!("[xtask] Android .so 빌드 완료: {ANDROID_ABIS:?}");
    }
    code
}
