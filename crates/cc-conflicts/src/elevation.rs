//! Launching CaseControl's own executable as the elevated helper (one UAC prompt per batch) and recognising the
//! helper's command line.
//!
//! Protocol: the app writes an [`ElevatedRequest`](crate::ElevatedRequest) as JSON to a fresh file whose name ends in
//! `.json`, then starts `<own exe> --elevated-actions "<absolute request path>"` with the `runas` verb and waits for
//! the process to exit (with a timeout). The helper validates and runs the request and writes an
//! [`ElevatedResponse`](crate::ElevatedResponse) to [`result_path`] of the request, creating that file new (it never
//! overwrites an existing file). The app reads the result and deletes both files.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The helper-mode flag. `main` checks for it before anything else starts.
pub const ELEVATED_ACTIONS_FLAG: &str = "--elevated-actions";

/// How long the app waits for the helper after the UAC prompt was accepted.
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(60);

/// Why the helper did not run to completion.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ElevationError {
    /// The user declined the UAC prompt.
    #[error("the Windows permission prompt was declined")]
    Declined,
    /// The helper did not exit within the timeout.
    #[error("the elevated helper did not finish within {0} s")]
    Timeout(u64),
    /// The request path cannot be passed safely.
    #[error("invalid request path: {0}")]
    InvalidPath(String),
    /// Starting or waiting failed.
    #[error("{0}")]
    Failed(String),
    /// Not available on this platform.
    #[error("not supported on this platform")]
    Unsupported,
}

/// Whether `path` is acceptable as a request file: absolute, a `.json` file name, and no `"` (it is passed quoted).
fn valid_request_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    path.is_absolute()
        && !text.contains('"')
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".json"))
}

/// The helper's parameter string: `--elevated-actions "<path>"`.
pub fn helper_parameters(request: &Path) -> Result<String, ElevationError> {
    if !valid_request_path(request) {
        return Err(ElevationError::InvalidPath(request.display().to_string()));
    }
    Ok(format!("{ELEVATED_ACTIONS_FLAG} \"{}\"", request.display()))
}

/// The request path if the command line (including the program name) is exactly
/// `<program> --elevated-actions <absolute .json path>`; `None` otherwise (normal start).
pub fn helper_request_path<I, S>(args: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    match args.as_slice() {
        [_, flag, path] if flag == ELEVATED_ACTIONS_FLAG => {
            let path = PathBuf::from(path);
            valid_request_path(&path).then_some(path)
        }
        _ => None,
    }
}

/// Whether the command line asks for helper mode at all (also with wrong arguments, which the helper then refuses
/// instead of starting the app).
pub fn is_helper_invocation<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    args.into_iter()
        .skip(1)
        .any(|a| a.into() == ELEVATED_ACTIONS_FLAG)
}

/// Where the helper writes its response: the request path with `.json` replaced by `.result.json`.
pub fn result_path(request: &Path) -> PathBuf {
    request.with_extension("result.json")
}

/// Starts `exe` elevated in helper mode for `request` (one UAC prompt) and waits up to `timeout` for it to exit.
/// Returns the helper's exit code.
pub fn run_elevated_helper(exe: &Path, request: &Path, timeout: Duration) -> Result<u32, ElevationError> {
    let parameters = helper_parameters(request)?;
    if !exe.is_absolute() {
        return Err(ElevationError::InvalidPath(exe.display().to_string()));
    }
    #[cfg(windows)]
    {
        tracing::warn!(exe = %exe.display(), request = %request.display(), "starting the elevated helper on user request");
        crate::sys::run_as_admin_and_wait(exe, &parameters, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (parameters, timeout);
        Err(ElevationError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const REQ: &str = r"C:\Users\u\AppData\Local\CaseControl\elevated\request-1-2.json";
    #[cfg(not(windows))]
    const REQ: &str = "/tmp/CaseControl/elevated/request-1-2.json";

    #[test]
    fn parameters_quote_the_path() {
        assert_eq!(
            helper_parameters(Path::new(REQ)),
            Ok(format!("--elevated-actions \"{REQ}\""))
        );
        assert!(helper_parameters(Path::new("relative.json")).is_err());
        assert!(helper_parameters(Path::new(&REQ.replace(".json", ".txt"))).is_err());
        assert!(helper_parameters(Path::new(&REQ.replace("request", "a\"b"))).is_err());
    }

    #[test]
    fn helper_command_line_is_recognised_exactly() {
        assert_eq!(
            helper_request_path(["cc.exe", ELEVATED_ACTIONS_FLAG, REQ]),
            Some(PathBuf::from(REQ))
        );
        assert_eq!(helper_request_path(["cc.exe"]), None);
        assert_eq!(helper_request_path(["cc.exe", "--hidden"]), None);
        assert_eq!(helper_request_path(["cc.exe", ELEVATED_ACTIONS_FLAG]), None);
        assert_eq!(
            helper_request_path(["cc.exe", ELEVATED_ACTIONS_FLAG, REQ, "x"]),
            None
        );
        assert_eq!(
            helper_request_path(["cc.exe", ELEVATED_ACTIONS_FLAG, "relative.json"]),
            None
        );
        assert!(is_helper_invocation(["cc.exe", ELEVATED_ACTIONS_FLAG]));
        assert!(!is_helper_invocation(["cc.exe", "--hidden"]));
        assert!(!is_helper_invocation([ELEVATED_ACTIONS_FLAG]));
    }

    #[test]
    fn result_path_sits_next_to_the_request() {
        let result = result_path(Path::new(REQ));
        assert_eq!(result.parent(), Path::new(REQ).parent());
        assert_eq!(
            result.file_name().and_then(|n| n.to_str()),
            Some("request-1-2.result.json")
        );
    }

    #[test]
    fn relative_executable_is_refused() {
        assert!(matches!(
            run_elevated_helper(Path::new("casecontrol.exe"), Path::new(REQ), HELPER_TIMEOUT),
            Err(ElevationError::InvalidPath(_))
        ));
    }
}
