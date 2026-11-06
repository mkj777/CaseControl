//! Opt-in actions. Call these only from an explicit user click in the conflict view.
//!
//! Neither action changes how software starts: CaseControl never disables autostart, deletes services or
//! uninstalls anything (ADR 0001 #12). A terminated process or stopped service may come back at the next logon.

/// Why an action was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    /// No process with that pid exists any more.
    #[error("process {pid} no longer exists")]
    NotFound {
        /// The pid that was requested.
        pid: u32,
    },
    /// The pid now belongs to a different executable (pid reuse); nothing was done.
    #[error("process {pid} is `{actual}`, expected `{expected}`; refusing to terminate")]
    NameMismatch {
        /// The pid that was requested.
        pid: u32,
        /// Name the caller expected.
        expected: String,
        /// Name found now.
        actual: String,
    },
    /// The service name contains characters outside `[A-Za-z0-9_.\- ]`.
    #[error("invalid service name `{0}`")]
    InvalidServiceName(String),
    /// Reading the process list failed.
    #[error("process lookup failed: {0}")]
    Lookup(String),
    /// The system call failed (for example access denied for another user's process).
    #[error("{0}")]
    Failed(String),
    /// Not available on this platform.
    #[error("not supported on this platform")]
    Unsupported,
}

/// Terminates `pid` after re-checking that its image file name is still `expected_name` (case-insensitive).
///
/// The process is opened once and both the name check and the termination use that handle, so a pid reused
/// by another process between scan and click is refused with [`ActionError::NameMismatch`] instead of being
/// killed. Same-user processes only: no privilege is enabled and nothing is elevated, so another user's or a
/// protected process fails with [`ActionError::Failed`]. `reason` (typically the finding's entry id) is logged
/// together with the pid and name before anything happens.
pub fn terminate_process(pid: u32, expected_name: &str, reason: &str) -> Result<(), ActionError> {
    #[cfg(windows)]
    {
        crate::sys::terminate_checked(pid, expected_name, reason)
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, expected_name, reason);
        Err(ActionError::Unsupported)
    }
}

/// The name check and logging of [`terminate_process`] with an injectable name lookup and kill function.
///
/// `lookup` returns the current exe file name of `pid` (`None` if it does not exist); `kill` terminates it.
/// The Windows implementation passes closures over one open process handle; tests pass fakes.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn terminate_process_with(
    pid: u32,
    expected_name: &str,
    reason: &str,
    lookup: impl FnOnce(u32) -> Result<Option<String>, String>,
    kill: impl FnOnce(u32) -> Result<(), String>,
) -> Result<(), ActionError> {
    let actual = lookup(pid)
        .map_err(ActionError::Lookup)?
        .ok_or(ActionError::NotFound { pid })?;
    if !actual.eq_ignore_ascii_case(expected_name) {
        tracing::warn!(pid, expected = expected_name, actual = %actual, reason, "refusing to terminate: name mismatch");
        return Err(ActionError::NameMismatch {
            pid,
            expected: expected_name.to_owned(),
            actual,
        });
    }
    tracing::warn!(pid, name = %actual, reason, "terminating process on user request");
    kill(pid).map_err(ActionError::Failed)
}

/// Whether `name` is safe to pass to `sc.exe stop "<name>"`: non-empty, only `[A-Za-z0-9_.\- ]`.
pub(crate) fn is_valid_service_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ' '))
}

/// Asks Windows to stop service `name`: runs `%SystemRoot%\System32\sc.exe stop "<name>"` through `ShellExecuteW`
/// with the `runas` verb, which shows the UAC prompt. Returns as soon as the request was handed to the shell; it
/// does not wait for UAC or for the service to stop, so the caller should rescan afterwards.
///
/// `sc.exe` is started by absolute path, never by bare name, so a planted `sc.exe` in the current directory or on
/// `PATH` cannot be run elevated. Stopping does not disable autostart; the service starts again at the next boot
/// (ADR 0001 #12 forbids changing the start type). `reason` is logged with the name.
pub fn request_stop_service(name: &str, reason: &str) -> Result<(), ActionError> {
    let parameters = stop_parameters(name)?;
    #[cfg(windows)]
    {
        let sc = sc_exe_path(std::env::var_os("SystemRoot"))?;
        tracing::warn!(
            service = name,
            reason,
            sc = %sc.display(),
            "requesting elevated service stop on user request"
        );
        crate::sys::shell_execute_runas(&sc, &parameters).map_err(ActionError::Failed)
    }
    #[cfg(not(windows))]
    {
        let _ = (parameters, reason);
        Err(ActionError::Unsupported)
    }
}

/// `<system_root>\System32\sc.exe`; fails when the system root is unknown or not absolute.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn sc_exe_path(system_root: Option<std::ffi::OsString>) -> Result<std::path::PathBuf, ActionError> {
    let root = system_root
        .filter(|r| !r.is_empty())
        .ok_or_else(|| ActionError::Failed("SystemRoot is not set; cannot locate sc.exe".to_owned()))?;
    let root = std::path::PathBuf::from(root);
    if !root.is_absolute() {
        return Err(ActionError::Failed(format!(
            "SystemRoot `{}` is not an absolute path; cannot locate sc.exe",
            root.display()
        )));
    }
    Ok(root.join("System32").join("sc.exe"))
}

/// Builds the `sc.exe` parameter string after validating `name`.
fn stop_parameters(name: &str) -> Result<String, ActionError> {
    if !is_valid_service_name(name) {
        return Err(ActionError::InvalidServiceName(name.to_owned()));
    }
    Ok(format!("stop \"{name}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminate_refuses_on_name_mismatch_without_killing() {
        let result = terminate_process_with(
            42,
            "SignalRgb.exe",
            "signalrgb",
            |_| Ok(Some("explorer.exe".to_owned())),
            |_| panic!("must not kill on mismatch"),
        );
        assert_eq!(
            result,
            Err(ActionError::NameMismatch {
                pid: 42,
                expected: "SignalRgb.exe".to_owned(),
                actual: "explorer.exe".to_owned()
            })
        );
    }

    #[test]
    fn terminate_refuses_missing_process() {
        let result = terminate_process_with(7, "a.exe", "t", |_| Ok(None), |_| panic!("must not kill"));
        assert_eq!(result, Err(ActionError::NotFound { pid: 7 }));
    }

    #[test]
    fn terminate_calls_kill_on_case_insensitive_match() {
        let mut killed = None;
        let result = terminate_process_with(
            9,
            "fancontrol.EXE",
            "fancontrol",
            |_| Ok(Some("FanControl.exe".to_owned())),
            |pid| {
                killed = Some(pid);
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(killed, Some(9));
    }

    #[test]
    fn service_name_validation() {
        assert!(is_valid_service_name("SignalRgb.Service"));
        assert!(is_valid_service_name("Razer Game Manager Service"));
        assert!(is_valid_service_name("logi_lamparray_service"));
        assert!(is_valid_service_name("a-b"));
        assert!(!is_valid_service_name(""));
        assert!(!is_valid_service_name("   "));
        assert!(!is_valid_service_name("x\" & del"));
        assert!(!is_valid_service_name("x;y"));
        assert!(!is_valid_service_name("a/b"));
        assert!(!is_valid_service_name("dienst\u{e4}"));
        assert_eq!(
            stop_parameters("SignalRgb.Service").as_deref(),
            Ok("stop \"SignalRgb.Service\"")
        );
        assert_eq!(
            stop_parameters("bad\"name"),
            Err(ActionError::InvalidServiceName("bad\"name".to_owned()))
        );
        assert_eq!(
            stop_parameters("x\" & del"),
            Err(ActionError::InvalidServiceName("x\" & del".to_owned()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn sc_exe_is_absolute_under_system_root() {
        let root = std::env::var_os("SystemRoot").expect("SystemRoot is set on Windows");
        let path = sc_exe_path(Some(root.clone())).expect("sc.exe path");
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.starts_with(&root), "{}", path.display());
        assert!(path.ends_with(r"System32\sc.exe"), "{}", path.display());
        assert_eq!(path, std::path::Path::new(&root).join(r"System32\sc.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn sc_exe_path_rejects_missing_or_relative_root() {
        assert!(matches!(sc_exe_path(None), Err(ActionError::Failed(_))));
        assert!(matches!(
            sc_exe_path(Some("".into())),
            Err(ActionError::Failed(_))
        ));
        assert!(matches!(
            sc_exe_path(Some("Windows".into())),
            Err(ActionError::Failed(_))
        ));
        assert_eq!(
            sc_exe_path(Some(r"C:\Windows".into())),
            Ok(std::path::PathBuf::from(r"C:\Windows\System32\sc.exe"))
        );
    }
}
