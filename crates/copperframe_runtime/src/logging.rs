//! Logging setup. `RUST_LOG` overrides the default filter.
//!
//! Every event goes to stderr (coloured when it is a terminal) and, without
//! colour codes, to [`copperframe_core::report::record_log`], which keeps the
//! tail for crash reports and writes the log file once
//! [`copperframe_core::report::install`] has run.

use std::io::Write;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// Default filter: engine at `info`, noisy graphics internals at `warn`.
pub const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

/// Installs the subscriber. Safe to call more than once; later calls leave
/// the first subscriber in place. Call `report::install` first so the log
/// file is open from the first line.
pub fn init_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let stderr = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr);
    let report = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(false)
        .with_writer(ReportWriter);
    // `try_init` also bridges the `log` records emitted by wgpu.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(report)
        .try_init();
}

/// Collects one formatted event and hands it to the report log on drop.
#[derive(Default)]
struct EventBuffer(Vec<u8>);

impl Write for EventBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for EventBuffer {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            copperframe_core::report::record_log(&String::from_utf8_lossy(&self.0));
        }
    }
}

struct ReportWriter;

impl<'a> MakeWriter<'a> for ReportWriter {
    type Writer = EventBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        EventBuffer::default()
    }
}
