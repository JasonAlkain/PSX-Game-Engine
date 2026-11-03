//! Logging setup. `RUST_LOG` overrides the default filter.

use tracing_subscriber::EnvFilter;

/// Default filter: engine at `info`, noisy graphics internals at `warn`.
pub const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

/// Installs a stderr subscriber. Safe to call more than once; later calls
/// leave the first subscriber in place.
pub fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    // `try_init` also bridges the `log` records emitted by wgpu.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
