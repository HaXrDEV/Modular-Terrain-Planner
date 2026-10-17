//! Logging and crash reports. Nothing is sent anywhere: logs and reports
//! stay in the app's data folder, where About and bug reports point to.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

use crate::settings::{APP_NAME, VERSION};

/// Keeps the log writer alive; dropping it flushes the log.
pub struct LogGuard(#[allow(dead_code)] tracing_appender::non_blocking::WorkerGuard);

/// Logs to a daily file in `dir` (the last 7 are kept) and to stderr when
/// there is one. `RUST_LOG` overrides the level.
pub fn init_logging(dir: &Path) -> Option<LogGuard> {
    std::fs::create_dir_all(dir).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("app")
        .filename_suffix("log")
        .max_log_files(7)
        .build(dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn,eframe=warn,egui_wgpu=warn"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(writer).with_ansi(false))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init()
        .ok()?;
    tracing::info!(
        "{APP_NAME} {VERSION} on {} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::var("WGPU_BACKEND").unwrap_or_else(|_| "automatic backend".into())
    );
    Some(LogGuard(guard))
}

/// Where a crash report goes, and where the next start looks for one.
pub fn crash_report_path(logs: &Path) -> PathBuf {
    logs.join("crash-report.txt")
}

/// On a panic, logs it, and for the main thread writes a crash report the
/// next start offers to show, before the default hook prints it.
pub fn install_panic_hook(logs: PathBuf) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown".into());
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        tracing::error!("panic in thread {name} at {location}: {message}");
        // Loader threads catch their panics and report the file as skipped;
        // only a panic on the main thread ends the app
        if name != "main" {
            default(info);
            return;
        }
        let backtrace = std::backtrace::Backtrace::force_capture();
        if let Ok(mut f) = std::fs::File::create(crash_report_path(&logs)) {
            let _ = writeln!(
                f,
                "{APP_NAME} {VERSION} ({} {})\nPanicked at {location}:\n{message}\n\n{backtrace}",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
        }
        default(info);
    }));
}

/// A crash report from the last run, removed once read.
pub fn take_crash_report(logs: &Path) -> Option<String> {
    let path = crash_report_path(logs);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::rename(&path, logs.join("crash-report-previous.txt"));
    Some(text)
}
