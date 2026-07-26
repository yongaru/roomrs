//! roomrs CLI — `cargo roomrs schema export|check` · `cargo roomrs migrate …` (명세 §7.4/§8, 결정 47·58).
//!
//! 설치형 바이너리 이름: `cargo-roomrs` (`cargo roomrs …`).
#![deny(unsafe_code)]

use roomrs_migrate::{SchemaSnapshot, diff_plan, diff_sql, list_snapshot_versions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const SCHEMA_ENTRYPOINT_MARKER: &str = "__ROOMRS_SCHEMA_ENTRYPOINT__";

/// `log` 기반 라이브러리 레코드를 `tracing` 포맷으로 출력한다.
fn init_tracing() -> Result<(), String> {
    tracing_log::LogTracer::init().map_err(|e| format!("CLI log bridge initialization failed: {e}"))?;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,roomrs_core=debug,roomrs_migrate=debug,roomrs_cli=debug"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter).with_target(true).finish();
    tracing::subscriber::set_global_default(subscriber).map_err(|e| format!("CLI tracing subscriber initialization failed: {e}"))
}

/// UTF-8 인자 수집 (비 UTF-8 = 한국어 에러)
fn collect_args() -> Result<Vec<String>, ExitCode> {
    let mut args: Vec<String> = Vec::new();
    for a in std::env::args_os().skip(1) {
        match a.into_string() {
            Ok(s) => args.push(s),
            Err(bad) => {
                log::error!("argument is not valid UTF-8: {}", bad.to_string_lossy());
                return Err(ExitCode::from(2));
            }
        }
    }
    // cargo 가 `cargo-roomrs schema …` 로 호출하면 첫 인자가 서브커맨드 이름일 수 있다
    if args.first().is_some_and(|a| a == "roomrs" || a == "cargo-roomrs") {
        args.remove(0);
    }
    Ok(args)
}

/// schema와 migration 명령을 실행하는 단일 CLI 진입점.
fn run_cli() -> ExitCode {
    if let Err(e) = init_tracing() {
        eprintln!("{e}");
        return ExitCode::from(1);
    }
    tracing::info!("roomrs CLI started");
    let args = match collect_args() {
        Ok(a) => a,
        Err(c) => return c,
    };
    let strict = args.iter().any(|a| a == "--strict");
    let strs: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--strict").collect();
    tracing::debug!(argument_count = strs.len(), strict, "CLI arguments parsed");

    match strs.as_slice() {
        ["schema", "export", rest @ ..] => {
            if rest.iter().any(|a| *a == "--help" || *a == "-h") {
                print_schema_help();
                ExitCode::SUCCESS
            } else {
                cmd_schema_export(rest)
            }
        }
        ["schema", "check", rest @ ..] => {
            if rest.iter().any(|a| *a == "--help" || *a == "-h") {
                print_schema_check_help();
                ExitCode::SUCCESS
            } else {
                cmd_schema_check(rest)
            }
        }
        ["schema", "--help"] | ["schema", "-h"] => {
            print_schema_help();
            ExitCode::SUCCESS
        }
        ["migrate", "diff", old, new] => cmd_diff(old, new, None),
        ["migrate", "diff", old, new, out] => cmd_diff(old, new, Some(out)),
        ["migrate", "check", a, b] => cmd_check(a, b),
        ["migrate", "check-dir", dir, db] => cmd_check_dir(dir, db, strict),
        ["--help"] | ["-h"] | ["help"] => {
            print_help();
            ExitCode::SUCCESS
        }
        _ => {
            log::error!("usage: cargo roomrs migrate diff <old.json> <new.json> [out.sql]; cargo roomrs migrate check <a.json> <b.json>; cargo roomrs migrate check-dir <schema_dir> <db-name> [--strict]; cargo roomrs schema export [--package <name>] [--workspace]");
            ExitCode::from(2)
        }
    }
}

/// `cargo-roomrs` binary 진입점.
fn main() -> ExitCode {
    run_cli()
}

/// 사용법 출력
fn print_help() {
    println!(
        "\
cargo-roomrs

Usage:
  cargo roomrs schema export [--package <name>] [--workspace]
  cargo roomrs schema check   (see 040)
  cargo roomrs migrate diff <old.json> <new.json> [out.sql]
  cargo roomrs migrate check <a.json> <b.json>
  cargo roomrs migrate check-dir <schema_dir> <db-name> [--strict]

Notes:
  - snapshot JSON is only written by `schema export` (decision 39)
  - cargo test / cargo build never write schema or migration files
  - after export, run `cargo build` so macros re-embed snapshots
"
    );
}

/// schema export 도움말
fn print_schema_help() {
    println!(
        "\
cargo roomrs schema export

Discovers every #[database] registered in package lib/bin targets via inventory,
preflights all databases, then writes missing/current-version snapshots under
migrations/schema/[db].[version].json. version=auto may also write a forward SQL draft.

Options:
  --package <name>   Export a specific workspace member (default: current package)
  --workspace        Export workspace members that contain #[database]
  --help             Show this help

Consumer source changes are not required. Application main() is not run.
"
    );
}

/// schema check 도움말
fn print_schema_check_help() {
    println!(
        "\
cargo roomrs schema check

Read-only verification of every registered #[database] against on-disk snapshots.
Never creates or modifies schema JSON or migration SQL.

Options:
  --package <name>   Check a specific workspace member
  --workspace        Check workspace members that contain #[database]
  --help             Show this help

CI order: cargo roomrs schema check → cargo test --workspace → cargo build --workspace
"
    );
}

/// `schema check` — custom cfg harness 읽기 전용 실행
fn cmd_schema_check(args: &[&str]) -> ExitCode {
    let mut package: Option<String> = None;
    let mut workspace = false;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--package" | "-p" => {
                i += 1;
                let Some(name) = args.get(i) else {
                    log::error!("--package 뒤에 패키지 이름이 필요합니다");
                    return ExitCode::from(2);
                };
                package = Some((*name).to_string());
            }
            "--workspace" => workspace = true,
            other => {
                log::error!("unknown schema check argument: {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    if workspace && package.is_some() {
        log::error!("--workspace 와 --package 는 함께 쓸 수 없습니다");
        return ExitCode::from(2);
    }
    let meta = match cargo_metadata() {
        Ok(m) => m,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let targets = match select_packages(&meta, package.as_deref(), workspace) {
        Ok(t) => t,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut any_fail = false;
    for pkg in targets {
        match check_one_package(&pkg) {
            Ok(true) => log::info!("schema check ok: package={}", pkg.name),
            Ok(false) if workspace => log::debug!("schema check skipped: package={}, reason=no_database", pkg.name),
            Ok(false) => {
                log::error!("schema check failed: package={}: #[database]를 찾지 못했습니다", pkg.name);
                any_fail = true;
            }
            Err(e) => {
                log::error!("schema check failed: package={}: {e}", pkg.name);
                any_fail = true;
            }
        }
    }
    if any_fail { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// 한 패키지 읽기 전용 check — custom cfg harness 실행
fn check_one_package(pkg: &PkgInfo) -> Result<bool, String> {
    run_schema_action(pkg, SchemaAction::Check)
}

/// `schema export` — package 선택 후 lib/bin custom cfg harness 실행
fn cmd_schema_export(args: &[&str]) -> ExitCode {
    let mut package: Option<String> = None;
    let mut workspace = false;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--package" | "-p" => {
                i += 1;
                let Some(name) = args.get(i) else {
                    log::error!("--package 뒤에 패키지 이름이 필요합니다");
                    return ExitCode::from(2);
                };
                package = Some((*name).to_string());
            }
            "--workspace" => workspace = true,
            "--help" | "-h" => {
                print_schema_help();
                return ExitCode::SUCCESS;
            }
            other => {
                log::error!("unknown schema export argument: {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    if workspace && package.is_some() {
        log::error!("--workspace 와 --package 는 함께 쓸 수 없습니다");
        return ExitCode::from(2);
    }

    let meta = match cargo_metadata() {
        Ok(m) => m,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let targets = match select_packages(&meta, package.as_deref(), workspace) {
        Ok(t) => t,
        Err(e) => {
            log::error!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut any_fail = false;
    for pkg in targets {
        match export_one_package(&pkg) {
            Ok(true) => log::info!("schema export ok: package={}", pkg.name),
            Ok(false) if workspace => log::debug!("schema export skipped: package={}, reason=no_database", pkg.name),
            Ok(false) => {
                log::error!("schema export failed: package={}: #[database]를 찾지 못했습니다", pkg.name);
                any_fail = true;
            }
            Err(e) => {
                log::error!("schema export failed: package={}: {e}", pkg.name);
                any_fail = true;
            }
        }
    }
    if any_fail { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// cargo metadata JSON 한 패키지 요약
#[derive(Clone)]
struct PkgInfo {
    name: String,
    manifest_dir: PathBuf,
    schema_targets: Vec<SchemaTarget>,
}

/// schema harness로 컴파일할 production target.
#[derive(Clone)]
enum SchemaTarget {
    Lib,
    Bin(String),
}

/// schema harness 작업.
#[derive(Clone, Copy)]
enum SchemaAction {
    Export,
    Check,
}

impl SchemaAction {
    /// generated harness에 전달할 환경값.
    fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Check => "check",
        }
    }
}

/// cargo metadata 실행
fn cargo_metadata() -> Result<serde_json::Value, String> {
    let out = Command::new("cargo").args(["metadata", "--format-version", "1", "--no-deps"]).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("cargo metadata 실행 실패: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata 실패: {}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata JSON 파스 실패: {e}"))
}

/// package 선택 — 현재 디렉터리 패키지 / --package / --workspace
fn select_packages(meta: &serde_json::Value, package: Option<&str>, workspace: bool) -> Result<Vec<PkgInfo>, String> {
    let packages = meta.get("packages").and_then(|p| p.as_array()).ok_or_else(|| "cargo metadata: packages 없음".to_string())?;
    let workspace_root = meta.get("workspace_root").and_then(|v| v.as_str()).unwrap_or(".");
    let members: Vec<&str> = meta.get("workspace_members").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();

    let mut out = Vec::new();
    for p in packages {
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !members.contains(&id) {
            continue;
        }
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let manifest_path = p.get("manifest_path").and_then(|v| v.as_str()).unwrap_or("");
        let manifest_dir = Path::new(manifest_path).parent().unwrap_or(Path::new(".")).to_path_buf();
        let targets = p.get("targets").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        let mut schema_targets = Vec::new();
        for target in &targets {
            if !target.get("test").and_then(serde_json::Value::as_bool).unwrap_or(true) {
                continue;
            }
            let kinds = target.get("kind").and_then(serde_json::Value::as_array);
            if kinds.is_some_and(|values| values.iter().any(|value| value.as_str() == Some("lib"))) {
                schema_targets.push(SchemaTarget::Lib);
                continue;
            }
            if kinds.is_some_and(|values| values.iter().any(|value| value.as_str() == Some("bin"))) {
                if let Some(target_name) = target.get("name").and_then(serde_json::Value::as_str) {
                    schema_targets.push(SchemaTarget::Bin(target_name.to_string()));
                }
            }
        }
        out.push(PkgInfo { name, manifest_dir, schema_targets });
    }

    if workspace {
        return Ok(out);
    }
    if let Some(name) = package {
        return out.into_iter().find(|p| p.name == name).map(|p| vec![p]).ok_or_else(|| format!("패키지를 찾지 못했습니다: {name}"));
    }
    // 기본: CWD 가 속한 패키지, 없으면 workspace root 패키지
    let cwd = std::env::current_dir().map_err(|e| format!("cwd 실패: {e}"))?;
    if let Some(pkg) = out.iter().find(|p| cwd.starts_with(&p.manifest_dir)).cloned() {
        return Ok(vec![pkg]);
    }
    let root = PathBuf::from(workspace_root);
    if let Some(pkg) = out.iter().find(|p| p.manifest_dir == root).cloned() {
        return Ok(vec![pkg]);
    }
    out.into_iter().next().map(|p| vec![p]).ok_or_else(|| "workspace 멤버 패키지가 없습니다".into())
}

/// 한 패키지 export — lib/bin custom cfg harness 실행
fn export_one_package(pkg: &PkgInfo) -> Result<bool, String> {
    let found = run_schema_action(pkg, SchemaAction::Export)?;
    if found {
        log::info!("export finished for {} — run `cargo build -p {}` to re-embed snapshots", pkg.name, pkg.name);
    }
    Ok(found)
}

/// package의 production target 전체에서 generated schema entrypoint를 실행한다.
fn run_schema_action(pkg: &PkgInfo, action: SchemaAction) -> Result<bool, String> {
    if pkg.schema_targets.is_empty() {
        return Ok(false);
    }
    let mut found = false;
    for target in &pkg.schema_targets {
        found |= run_schema_target(pkg, target, action)?;
    }
    Ok(found)
}

/// target 하나를 stable test harness로 컴파일·실행한다.
fn run_schema_target(pkg: &PkgInfo, target: &SchemaTarget, action: SchemaAction) -> Result<bool, String> {
    let mut command = Command::new("cargo");
    command.args(["test", "-q", "-p", &pkg.name]);
    match target {
        SchemaTarget::Lib => {
            command.arg("--lib");
        }
        SchemaTarget::Bin(name) => {
            command.args(["--bin", name]);
        }
    }
    command.args(["__roomrs_export_", "--", "--nocapture", "--test-threads=1"]);
    command.env("ROOMRS_SCHEMA_ACTION", action.as_str()).current_dir(&pkg.manifest_dir);
    add_roomrs_export_cfg(&mut command);
    let output = command.output().map_err(|e| format!("schema harness 실행 실패(package={}): {e}", pkg.name))?;
    std::io::stdout().write_all(&output.stdout).map_err(|e| format!("schema harness stdout 전달 실패: {e}"))?;
    std::io::stderr().write_all(&output.stderr).map_err(|e| format!("schema harness stderr 전달 실패: {e}"))?;
    if !output.status.success() {
        return Err(format!("schema {} harness 실패(package={}, target={}): {}", action.as_str(), pkg.name, target.label(), output.status));
    }
    Ok(contains_marker(&output.stdout) || contains_marker(&output.stderr))
}

impl SchemaTarget {
    /// 사용자 진단용 target 이름.
    fn label(&self) -> &str {
        match self {
            Self::Lib => "lib",
            Self::Bin(name) => name,
        }
    }
}

/// 기존 rustflags를 보존하면서 custom cfg와 check-cfg를 추가한다.
fn add_roomrs_export_cfg(command: &mut Command) {
    if let Some(mut encoded) = std::env::var_os("CARGO_ENCODED_RUSTFLAGS") {
        if !encoded.is_empty() {
            encoded.push("\u{1f}");
        }
        encoded.push("--cfg\u{1f}roomrs_export\u{1f}--check-cfg=cfg(roomrs_export)");
        command.env("CARGO_ENCODED_RUSTFLAGS", encoded);
        return;
    }
    let mut flags = std::env::var_os("RUSTFLAGS").unwrap_or_default();
    if !flags.is_empty() {
        flags.push(" ");
    }
    flags.push("--cfg roomrs_export --check-cfg=cfg(roomrs_export)");
    command.env("RUSTFLAGS", flags);
}

/// harness 출력에 generated 진입점 marker가 있는지 검사한다.
fn contains_marker(bytes: &[u8]) -> bool {
    bytes.windows(SCHEMA_ENTRYPOINT_MARKER.len()).any(|window| window == SCHEMA_ENTRYPOINT_MARKER.as_bytes())
}

/// 스냅샷 로드 (오류는 로그 기록)
fn load(path: &str) -> Result<SchemaSnapshot, ExitCode> {
    SchemaSnapshot::read_from(Path::new(path)).map_err(|e| {
        log::error!("failed to read snapshot ({path}): {e}");
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
        Some(path) => match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut f) => match std::io::Write::write_all(&mut f, sql.as_bytes()) {
                Ok(()) => {
                    log::info!("migration draft saved: {path}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    log::error!("failed to save migration draft ({path}): {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                log::error!("migration draft already exists and will not be overwritten: {path}");
                ExitCode::FAILURE
            }
            Err(e) => {
                log::error!("failed to save migration draft ({path}): {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// 해시 비교
fn cmd_check(a: &str, b: &str) -> ExitCode {
    let (a, b) = match (load(a), load(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(c), _) | (_, Err(c)) => return c,
    };
    if a.hash() == b.hash() {
        log::info!("snapshots match");
        ExitCode::SUCCESS
    } else {
        log::error!("snapshots differ (hash {} vs {})", a.hash(), b.hash());
        ExitCode::FAILURE
    }
}

/// 버전 파일 디렉토리 검사
fn cmd_check_dir(dir: &str, db: &str, strict: bool) -> ExitCode {
    let files = match list_snapshot_versions(Path::new(dir), db) {
        Ok(f) => f,
        Err(e) => {
            log::error!("failed to read snapshot directory ({dir}): {e}");
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        log::error!("no snapshots found in {dir} for database {db}");
        return ExitCode::FAILURE;
    }

    let mut snaps: Vec<SchemaSnapshot> = Vec::with_capacity(files.len());
    for (ver, path) in &files {
        let s = match SchemaSnapshot::read_from(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("failed to read snapshot ({}): {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        if s.version != *ver {
            log::error!("snapshot internal version {} differs from filename version {ver}: {}", s.version, path.display());
            return ExitCode::FAILURE;
        }
        snaps.push(s);
    }

    let mut warn_count: usize = 0;
    for pair in snaps.windows(2) {
        if pair[1].version - pair[0].version > 1 {
            log::warn!("snapshot version gap: v{} followed by v{}", pair[0].version, pair[1].version);
            warn_count += 1;
        }
        let plan = diff_plan(&pair[0], &pair[1]);
        for d in &plan.destructive {
            log::warn!("destructive change from v{} to v{}: {d}", pair[0].version, pair[1].version);
            warn_count += 1;
        }
        for w in &plan.warnings {
            log::warn!("migration warning from v{} to v{}: {w}", pair[0].version, pair[1].version);
            warn_count += 1;
        }
    }
    log::info!("checked {} snapshots: v{}..v{} ({db})", files.len(), files.first().map(|(v, _)| *v).unwrap_or(0), files.last().map(|(v, _)| *v).unwrap_or(0));
    if strict && warn_count > 0 {
        log::error!("strict mode: {warn_count} warning(s)");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
