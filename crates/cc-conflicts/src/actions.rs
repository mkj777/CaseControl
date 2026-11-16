//! Checked process termination (the building block of [`Action::TerminateProcess`](crate::Action)) and service
//! name validation. Call these only for an explicit user click; everything else goes through the `resolve` module
//! (ADR 0001 #31).

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

/// Whether `name` is an acceptable service name: non-empty, only `[A-Za-z0-9_.\- ]`.
pub(crate) fn is_valid_service_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ' '))
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
    }
}
