//! Logging setup: `tracing` to a daily rolling file under `%LOCALAPPDATA%\CaseControl\logs`, plus stderr in
//! debug builds. The filter comes from `CASECONTROL_LOG` (same syntax as `RUST_LOG`), default `info`.

use std::path::PathBuf;
use std::sync::Mutex;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Environment variable that holds the log filter directives.
pub const LOG_ENV_VAR: &str = "CASECONTROL_LOG";

/// File name prefix of the rolling log files (`casecontrol.YYYY-MM-DD.log`).
const LOG_FILE_PREFIX: &str = "casecontrol";

/// How many daily log files are kept before the oldest is deleted.
const MAX_LOG_FILES: usize = 14;

/// Directory for log files: `<data_local_dir>\CaseControl\logs`, i.e. `%LOCALAPPDATA%\CaseControl\logs` on
/// Windows. `cc-config` owns the canonical path definitions from Phase 2 on and must produce the same path.
/// Returns `None` if no home directory can be determined.
pub fn log_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.data_local_dir().join("CaseControl").join("logs"))
}

/// Builds the filter from `CASECONTROL_LOG`; invalid directives are ignored, the default level is `info`.
fn env_filter() -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .with_env_var(LOG_ENV_VAR)
        .from_env_lossy()
}

/// Installs the global subscriber. Call once, before anything logs.
///
/// Failing to create the log directory is not fatal: the app keeps running and logs only to stderr
/// (debug builds) and reports the problem there. File writes are synchronous, so nothing is lost when the
/// process exits through `std::process::exit` (which Tauri's event loop does) and log volume stays low.
pub fn init() {
    let mut file_error = None;
    let file_layer = match log_dir() {
        Some(dir) => match open_appender(&dir) {
            Ok(appender) => Some(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(Mutex::new(appender))
                    .boxed(),
            ),
            Err(err) => {
                file_error = Some(format!("cannot open log directory {}: {err}", dir.display()));
                None
            }
        },
        None => {
            file_error = Some("cannot determine the local data directory".to_owned());
            None
        }
    };

    let stderr_layer = cfg!(debug_assertions).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .boxed()
    });

    let result = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .with(env_filter())
        .try_init();
    if result.is_err() {
        // A subscriber is already installed (e.g. by a test harness); keep using it.
        return;
    }
    if let Some(message) = file_error {
        tracing::warn!("file logging disabled: {message}");
    }
}

fn open_appender(dir: &std::path::Path) -> Result<RollingFileAppender, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(dir)?;
    Ok(appender)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_ends_with_casecontrol_logs() {
        let dir = log_dir().expect("CI runners have a home directory");
        assert!(dir.ends_with(std::path::Path::new("CaseControl").join("logs")));
    }

    #[cfg(windows)]
    #[test]
    fn log_dir_is_under_local_app_data() {
        let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set on Windows");
        let dir = log_dir().expect("CI runners have a home directory");
        assert_eq!(dir, PathBuf::from(local).join("CaseControl").join("logs"));
    }

    #[test]
    fn appender_writes_into_given_directory() {
        let dir = std::env::temp_dir().join(format!("casecontrol-log-test-{}", std::process::id()));
        let mut appender = open_appender(&dir).expect("temp dir is writable");
        std::io::Write::write_all(&mut appender, b"hello\n").expect("write succeeds");
        let entries: Vec<_> = std::fs::read_dir(&dir).expect("dir exists").collect();
        assert_eq!(entries.len(), 1);
        drop(appender);
        std::fs::remove_dir_all(&dir).expect("cleanup succeeds");
    }
}
