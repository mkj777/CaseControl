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

/// How long a service stop waits for the stopped state before it is reported as still stopping.
pub const SERVICE_STOP_WAIT: Duration = Duration::from_secs(10);

/// Base time the app gives the helper after the UAC prompt was accepted (its fresh scan and start-up).
pub const HELPER_BASE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the app waits for a helper running `actions`: [`HELPER_BASE_TIMEOUT`] plus [`SERVICE_STOP_WAIT`] and
/// 2 s per service stop and 2 s per other action. On timeout the outcomes are reported as unknown, not failed.
pub fn helper_timeout(actions: &[crate::PlannedAction]) -> Duration {
    actions.iter().fold(HELPER_BASE_TIMEOUT, |t, a| match a.action {
        crate::Action::StopService { .. } => t + SERVICE_STOP_WAIT + Duration::from_secs(2),
        _ => t + Duration::from_secs(2),
    })
}

/// Whether `path` has the shape of a request file: `...\CaseControl\elevated\request-<digits>-<digits>.json`.
/// Pure; [`validate_helper_request`] adds the file system checks.
pub fn request_path_shape_ok(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let [.., app, dir, file] = parts.as_slice() else {
        return false;
    };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let name_ok = file
        .strip_prefix("request-")
        .and_then(|rest| rest.strip_suffix(".json"))
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(a, b)| digits(a) && digits(b));
    path.is_absolute() && app == "casecontrol" && dir == "elevated" && name_ok
}

/// The helper's check of its request path: the shape of [`request_path_shape_ok`], an existing regular file, no
/// symbolic link, junction or other reparse point in any component, and a canonical path equal to the given one
/// (so nothing in it redirects elsewhere). Returns the path to use.
pub fn validate_helper_request(path: &Path) -> Result<PathBuf, String> {
    if !request_path_shape_ok(path) {
        return Err(format!("{} is not a CaseControl request file", path.display()));
    }
    for ancestor in path.ancestors() {
        if ancestor.parent().is_none() {
            break; // the drive root
        }
        let meta = std::fs::symlink_metadata(ancestor).map_err(|e| format!("{}: {e}", ancestor.display()))?;
        if is_reparse_point(&meta) {
            return Err(format!("{} is a link or reparse point", ancestor.display()));
        }
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let canonical = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let canonical_text = canonical.to_string_lossy();
    let canonical_text = canonical_text.strip_prefix(r"\\?\").unwrap_or(&canonical_text);
    if !canonical_text.eq_ignore_ascii_case(&path.to_string_lossy()) {
        return Err(format!("{} resolves to {canonical_text}", path.display()));
    }
    Ok(path.to_path_buf())
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

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

/// Starts `exe` elevated in helper mode for `request` (one UAC prompt, owned by window `owner` when given so it comes
/// to the front) and waits up to `timeout` for it to exit. Returns the helper's exit code.
pub fn run_elevated_helper(
    exe: &Path,
    request: &Path,
    timeout: Duration,
    owner: Option<isize>,
) -> Result<u32, ElevationError> {
    let parameters = helper_parameters(request)?;
    if !exe.is_absolute() {
        return Err(ElevationError::InvalidPath(exe.display().to_string()));
    }
    #[cfg(windows)]
    {
        tracing::warn!(exe = %exe.display(), request = %request.display(), "starting the elevated helper on user request");
        crate::sys::run_as_admin_and_wait(exe, &parameters, timeout, owner)
    }
    #[cfg(not(windows))]
    {
        let _ = (parameters, timeout, owner);
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
    fn request_path_shape() {
        assert!(request_path_shape_ok(Path::new(REQ)));
        for bad in [
            REQ.replace("request-1-2", "request-1"),
            REQ.replace("request-1-2", "request-a-2"),
            REQ.replace("request-1-2", "other-1-2"),
            REQ.replace("elevated", "logs"),
            REQ.replace("CaseControl", "Other"),
            "request-1-2.json".to_owned(),
        ] {
            assert!(!request_path_shape_ok(Path::new(&bad)), "{bad}");
        }
    }

    #[test]
    fn helper_request_validation_on_disk() {
        let root = std::env::temp_dir().join(format!("cc-validate-{}", std::process::id()));
        let dir = root.join("CaseControl").join("elevated");
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("request-1-2.json");
        std::fs::write(&file, "{}").expect("write");
        // The temp folder itself may sit behind a link on some machines; compare against its canonical form.
        let canonical_root = std::fs::canonicalize(&root).expect("canonical");
        let text = canonical_root.to_string_lossy().into_owned();
        let plain = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
        let good = plain
            .join("CaseControl")
            .join("elevated")
            .join("request-1-2.json");
        assert_eq!(validate_helper_request(&good), Ok(good.clone()));
        // Missing file, a directory with the right name, and a wrong shape are refused.
        assert!(validate_helper_request(&good.with_file_name("request-1-3.json")).is_err());
        std::fs::create_dir_all(good.with_file_name("request-9-9.json")).expect("dir");
        assert!(validate_helper_request(&good.with_file_name("request-9-9.json")).is_err());
        assert!(validate_helper_request(&good.with_file_name("x.json")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn timeout_grows_with_service_stops() {
        use crate::{Action, PlannedAction};
        let p = |action| PlannedAction {
            entry_id: "x".into(),
            action,
        };
        assert_eq!(helper_timeout(&[]), HELPER_BASE_TIMEOUT);
        let t = helper_timeout(&[
            p(Action::StopService { name: "a".into() }),
            p(Action::StopService { name: "b".into() }),
            p(Action::TerminateProcess {
                pid: 1,
                name: "a.exe".into(),
            }),
        ]);
        assert_eq!(
            t,
            HELPER_BASE_TIMEOUT + 2 * (SERVICE_STOP_WAIT + Duration::from_secs(2)) + Duration::from_secs(2)
        );
    }

    #[test]
    fn relative_executable_is_refused() {
        assert!(matches!(
            run_elevated_helper(
                Path::new("casecontrol.exe"),
                Path::new(REQ),
                HELPER_BASE_TIMEOUT,
                None
            ),
            Err(ElevationError::InvalidPath(_))
        ));
    }
}
