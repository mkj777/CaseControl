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

/// File name prefix of the start-up task's log files (`boot-apply-YYYY-MM-DD.log`, ADR 0001 #42).
pub const BOOT_LOG_PREFIX: &str = "boot-apply-";

/// The name of the start-up task's log file for the UTC day `days` days after 1970-01-01:
/// `boot-apply-YYYY-MM-DD.log`.
pub fn boot_log_name(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{BOOT_LOG_PREFIX}{y:04}-{m:02}-{d:02}.log")
}

/// The proleptic Gregorian date of the day `days` after 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Logging of the start-up task's instance (`--apply-at-boot`, ADR 0001 #42): `tracing` to its own file
/// `boot-apply-YYYY-MM-DD.log` (UTC day, appended) in `dir`, created if needed; older files beyond the newest
/// 14 (as for the app log) are deleted. Returns the file's path, or why there is no file (then nothing is logged to a
/// file). `pending` lines (things decided before the file was known) are logged first.
pub fn init_boot(dir: &std::path::Path, pending: &[String]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0);
    let path = dir.join(boot_log_name(days));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    prune_boot_logs(dir, MAX_LOG_FILES);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(Mutex::new(file));
    let stderr_layer = cfg!(debug_assertions).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .boxed()
    });
    let _ = tracing_subscriber::registry()
        .with(file_layer)
        .with(stderr_layer)
        .with(env_filter())
        .try_init();
    for line in pending {
        tracing::info!("{line}");
    }
    Ok(path)
}

/// Deletes the oldest `boot-apply-*.log` files in `dir` beyond the newest `keep` (by name, which sorts by date).
fn prune_boot_logs(dir: &std::path::Path, keep: usize) {
    let Ok(items) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = items
        .flatten()
        .map(|i| i.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(BOOT_LOG_PREFIX) && n.ends_with(".log"))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for name in &names[..excess] {
        let _ = std::fs::remove_file(dir.join(name));
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
    fn boot_log_names_carry_the_utc_date() {
        assert_eq!(boot_log_name(0), "boot-apply-1970-01-01.log");
        assert_eq!(boot_log_name(20_784), "boot-apply-2026-11-27.log");
        assert_eq!(boot_log_name(19_782), "boot-apply-2024-02-29.log");
        assert_eq!(boot_log_name(19_783), "boot-apply-2024-03-01.log");
    }

    #[test]
    fn old_boot_logs_are_pruned_by_date() {
        let dir = std::env::temp_dir().join(format!("casecontrol-bootlog-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        for day in 0..5 {
            std::fs::write(dir.join(boot_log_name(day)), "x").expect("write");
        }
        std::fs::write(dir.join("casecontrol.2026-01-01.log"), "x").expect("write");
        prune_boot_logs(&dir, 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .expect("dir")
            .flatten()
            .map(|i| i.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "boot-apply-1970-01-04.log",
                "boot-apply-1970-01-05.log",
                "casecontrol.2026-01-01.log"
            ]
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
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
