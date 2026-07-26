//! 예제 공통 로깅 초기화.

/// roomrs의 `log` 레코드를 `tracing` 포맷으로 출력한다.
pub(crate) fn init_tracing() {
    if let Err(e) = tracing_log::LogTracer::init() {
        log::error!("example log bridge initialization skipped: {e}");
        return;
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,roomrs_core=debug,roomrs_async=debug,roomrs_migrate=debug"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter).with_target(true).finish();
    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        log::error!("example tracing subscriber initialization skipped: {e}");
    }
}
