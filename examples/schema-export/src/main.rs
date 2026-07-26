//! Schema snapshot export entry point (decision 47).
//!
//! Run: `cargo roomrs schema export -p roomrs-schema-export`
//!
//! Creates `migrations/schema/{db}.{version}.json` only when missing.
//! Never overwrites an existing same-version file (decision 39).
//! Do not invoke from `build.rs` (Cargo re-entrancy).

use roomrs::{database, entity, run_registered_schema_export};

#[entity(table = "notes", trigger = "migrations/triggers/note_audit.sql")]
struct Note {
    #[pk(autoincrement)]
    id: i64,
    body: String,
}

#[database(entities(Note), version = 1)]
struct AppDb;

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let _ = tracing_log::LogTracer::init();
    // #[database] inventory 등록 — AppDb 참조로 링크 보장
    let _ = AppDb::builder;

    let manifest = env!("CARGO_MANIFEST_DIR");
    if std::env::var("ROOMRS_SCHEMA_CHECK").ok().as_deref() == Some("1") {
        if let Err(e) = roomrs::run_registered_schema_check(manifest) {
            log::error!("schema check failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    match run_registered_schema_export(manifest) {
        Ok(paths) => {
            for path in paths {
                log::info!("schema snapshot ready: {}", path.display());
                println!("{}", path.display());
            }
        }
        Err(e) => {
            log::error!("schema export failed: {e}");
            std::process::exit(1);
        }
    }
}
