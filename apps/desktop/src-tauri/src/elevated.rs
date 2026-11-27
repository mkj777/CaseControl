//! The elevated helper: one UAC prompt per Proceed (ADR 0001 #31).
//!
//! App side ([`run_admin_batch`]): writes the actions that need administrator rights as an [`ElevatedRequest`]
//! (JSON, with this app's Windows session) to a new file `request-<pid>-<nanos>.json` under
//! `%LOCALAPPDATA%\CaseControl\elevated`, starts its own executable elevated with `--elevated-actions "<file>"`
//! (the main window owns the UAC prompt), waits for it ([`cc_conflicts::helper_timeout`], sized from the batch),
//! reads the [`ElevatedResponse`] from the result file next to it and deletes both files. If the helper is still
//! running at the timeout, the outcomes are reported as not known yet and the files are left for the helper;
//! [`sweep_stale_files`] removes leftovers on a later start.
//!
//! Helper side ([`helper_main`], called from `main` before Tauri, the single-instance guard or the engine start, and
//! without any logging setup): checks the request path ([`cc_conflicts::validate_helper_request`]: the request file
//! shape, no links or reparse points, canonical path unchanged), reads the request through a bounded reader (at most
//! [`MAX_REQUEST_BYTES`]), runs a fresh conflict scan, and lets [`cc_conflicts::run_elevated_request_in`] validate
//! every action against it (fixed action enum, catalogue-matched names only, HKCU writes refused, processes only in
//! the requesting session, the request's session equal to the helper's) and run what passes through `WindowsOps`;
//! the start-up task actions of ADR 0001 #42 (`RegisterBootTask` / `RemoveBootTask`) derive their user from the
//! validated request file ([`cc_conflicts::HelperContext::for_request`]). No shell is started. The response is
//! written to a new file next to the request after the request path was checked again
//! ([`cc_conflicts::write_helper_result`], never over an existing file), then the helper exits.

use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cc_conflicts::{
    ActionOutcome, ELEVATED_PROTOCOL_VERSION, ElevatedRequest, ElevatedResponse, ElevationError,
    PlannedAction, helper_request_path, helper_timeout, is_helper_invocation, result_path,
    validate_helper_request,
};

/// Largest request file the helper reads.
pub const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Request and result files older than this are removed by [`sweep_stale_files`].
pub const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// Helper exit code: the response was written.
pub const EXIT_OK: i32 = 0;
/// Helper exit code: the command line or the request path was refused.
pub const EXIT_BAD_ARGS: i32 = 2;
/// Helper exit code: the request could not be read or parsed.
pub const EXIT_BAD_REQUEST: i32 = 3;
/// Helper exit code: the response could not be written.
pub const EXIT_WRITE_FAILED: i32 = 4;

/// Entry point for `main`: `None` for a normal start, `Some(exit code)` when the command line asks for helper mode
/// (the helper then already ran). Sets up no logging: the elevated process creates, writes and prunes no log files.
pub fn helper_main(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args: Vec<OsString> = args.into_iter().collect();
    if !is_helper_invocation(args.iter().cloned()) {
        return None;
    }
    let Some(request) = helper_request_path(args) else {
        return Some(EXIT_BAD_ARGS);
    };
    match validate_helper_request(&request) {
        Ok(request) => Some(run_helper(&request)),
        Err(_) => Some(EXIT_BAD_ARGS),
    }
}

/// Reads the request file through a reader bounded to [`MAX_REQUEST_BYTES`] + 1, refusing anything larger.
fn read_request(path: &Path) -> Result<ElevatedRequest, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_REQUEST_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if text.len() as u64 > MAX_REQUEST_BYTES {
        return Err(format!("request file is larger than {MAX_REQUEST_BYTES} bytes"));
    }
    parse_request(&text)
}

/// Parses a request (JSON).
pub fn parse_request(text: &str) -> Result<ElevatedRequest, String> {
    serde_json::from_str(text).map_err(|e| format!("malformed request: {e}"))
}

/// Writes `response` to `path`, which must not exist yet.
#[cfg(test)]
fn write_new(path: &Path, response: &ElevatedResponse) -> std::io::Result<()> {
    let text = serde_json::to_vec(response).map_err(std::io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&text)?;
    file.sync_all()
}

/// The helper: run the request in `request` (an already validated path), write the response next to it.
pub fn run_helper(request: &Path) -> i32 {
    let Ok(parsed) = read_request(request) else {
        return EXIT_BAD_REQUEST;
    };
    let response = execute_request(&parsed, request);
    let Ok(bytes) = serde_json::to_vec(&response) else {
        return EXIT_WRITE_FAILED;
    };
    match cc_conflicts::write_helper_result(request, &bytes) {
        Ok(()) => EXIT_OK,
        Err(_) => EXIT_WRITE_FAILED,
    }
}

/// The helper's scanner: [`cc_conflicts::Scanner::windows_helper`], whose autostart source checks the owner of the
/// common Startup folder before it reports (and so lets the helper act on) entries there. The unelevated
/// `Scanner::windows` must not be used in this module outside tests (see the test
/// `the_helper_scans_with_the_helper_scanner`).
#[cfg(windows)]
fn helper_scanner() -> cc_conflicts::Scanner {
    cc_conflicts::Scanner::windows_helper()
}

#[cfg(windows)]
fn execute_request(request: &ElevatedRequest, request_file: &Path) -> ElevatedResponse {
    let report = helper_scanner().scan(&cc_conflicts::ScanOptions::default());
    cc_conflicts::run_elevated_request_in(
        request,
        &report,
        &cc_conflicts::WindowsOps,
        &cc_conflicts::HelperContext::for_request(request_file),
    )
}

#[cfg(not(windows))]
fn execute_request(request: &ElevatedRequest, _request_file: &Path) -> ElevatedResponse {
    ElevatedResponse {
        version: ELEVATED_PROTOCOL_VERSION,
        outcomes: request
            .actions
            .iter()
            .map(|a| ActionOutcome::failure(a, "not supported on this platform"))
            .collect(),
    }
}

/// Every action failed with `reason`.
pub fn all_failed(actions: &[PlannedAction], reason: &str) -> Vec<ActionOutcome> {
    actions
        .iter()
        .map(|a| ActionOutcome::failure(a, reason))
        .collect()
}

/// Every action's result is not known yet (the helper is still running).
pub fn all_unknown(actions: &[PlannedAction]) -> Vec<ActionOutcome> {
    actions
        .iter()
        .map(|a| {
            ActionOutcome::not_known(
                a,
                "not confirmed yet: the elevated helper is still working; scan again in a moment",
            )
        })
        .collect()
}

/// Pairs the helper's response with the actions that were sent: an outcome counts only if it is at the same
/// position and names the same entry and action; anything missing or mismatched is reported as failed.
pub fn merge_response(actions: &[PlannedAction], response: Option<ElevatedResponse>) -> Vec<ActionOutcome> {
    let outcomes = match response {
        Some(r) if r.version == ELEVATED_PROTOCOL_VERSION => r.outcomes,
        Some(_) => {
            return all_failed(
                actions,
                "the elevated helper answered with another protocol version",
            );
        }
        None => return all_failed(actions, "the elevated helper returned no result"),
    };
    actions
        .iter()
        .enumerate()
        .map(|(i, planned)| match outcomes.get(i) {
            Some(o) if o.entry_id == planned.entry_id && o.action == planned.action => o.clone(),
            _ => ActionOutcome::failure(planned, "the elevated helper returned no result for this action"),
        })
        .collect()
}

/// `%LOCALAPPDATA%\CaseControl\elevated`.
fn request_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.data_local_dir().join("CaseControl").join("elevated"))
}

/// Removes request and result files older than [`STALE_AFTER`] (left behind by a helper that outlived its
/// timeout). Called off the main thread at start; problems are only logged.
pub fn sweep_stale_files() {
    if let Some(dir) = request_dir() {
        sweep_dir(&dir, SystemTime::now(), STALE_AFTER);
    }
}

/// [`sweep_stale_files`] for `dir` at time `now`: deletes `request-*.json` files (results included) whose last
/// change is older than `age`. Returns how many it removed.
fn sweep_dir(dir: &Path, now: SystemTime, age: Duration) -> usize {
    let Ok(items) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for item in items.flatten() {
        let name = item.file_name().to_string_lossy().to_ascii_lowercase();
        if !(name.starts_with("request-") && name.ends_with(".json")) {
            continue;
        }
        let Ok(meta) = item.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|d| d > age);
        if meta.is_file() && old {
            match std::fs::remove_file(item.path()) {
                Ok(()) => removed += 1,
                Err(err) => {
                    tracing::warn!(file = %item.path().display(), %err, "cannot remove stale helper file")
                }
            }
        }
    }
    removed
}

/// Runs `actions` (all needing administrator rights, terminations of programs with admin work, or terminations
/// that failed without rights) in one elevated helper invocation; `session` limits terminations to this app's
/// Windows session and `owner` (the main window) brings the UAC prompt to the front. Shows exactly one UAC prompt.
/// Never fails as a whole: every problem becomes a failed (or, on timeout, not-yet-known) outcome.
pub fn run_admin_batch(
    actions: &[PlannedAction],
    session: Option<u32>,
    owner: Option<isize>,
) -> Vec<ActionOutcome> {
    if actions.is_empty() {
        return Vec::new();
    }
    let Some(dir) = request_dir() else {
        return all_failed(actions, "cannot determine the local data folder");
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return all_failed(actions, &format!("cannot create {}: {err}", dir.display()));
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let request_file = dir.join(format!("request-{}-{nanos}.json", std::process::id()));
    let result_file = result_path(&request_file);
    let (outcomes, helper_done) = run_helper_for(&request_file, &result_file, actions, session, owner);
    if helper_done {
        for file in [&request_file, &result_file] {
            if let Err(err) = std::fs::remove_file(file)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(file = %file.display(), %err, "cannot delete elevated helper file");
            }
        }
    }
    outcomes
}

/// Runs the helper; the flag says whether it is finished (or never started), i.e. whether its files may go.
fn run_helper_for(
    request_file: &Path,
    result_file: &Path,
    actions: &[PlannedAction],
    session: Option<u32>,
    owner: Option<isize>,
) -> (Vec<ActionOutcome>, bool) {
    let request = ElevatedRequest::new(actions.to_vec(), session);
    let written = serde_json::to_vec(&request)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(request_file)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes).map_err(|e| e.to_string())
        });
    if let Err(err) = written {
        return (
            all_failed(actions, &format!("cannot write the elevated request: {err}")),
            true,
        );
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            return (
                all_failed(actions, &format!("cannot locate CaseControl's executable: {err}")),
                true,
            );
        }
    };
    match cc_conflicts::run_elevated_helper(&exe, request_file, helper_timeout(actions), owner) {
        Ok(code) => {
            let response = std::fs::read_to_string(result_file)
                .ok()
                .and_then(|text| serde_json::from_str::<ElevatedResponse>(&text).ok());
            if response.is_none() {
                tracing::warn!(code, "elevated helper exited without a readable result");
            }
            (merge_response(actions, response), true)
        }
        Err(ElevationError::Timeout(secs)) => {
            tracing::warn!(
                secs,
                "elevated helper still running at the timeout; results unknown"
            );
            (all_unknown(actions), false)
        }
        Err(err) => {
            tracing::warn!(%err, "elevated helper did not run");
            (all_failed(actions, &err.to_string()), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use cc_conflicts::{Action, AutostartLocation, StartType};

    use super::*;

    /// Guard for the helper's scanner: the module's code (everything before this test module) scans only with
    /// `Scanner::windows_helper`, so the owner check on the common Startup folder applies in the elevated helper.
    #[test]
    fn the_helper_scans_with_the_helper_scanner() {
        let source = include_str!("elevated.rs");
        let code = source
            .split_once("mod tests {")
            .map(|(code, _)| code)
            .expect("test module");
        assert!(
            code.contains("cc_conflicts::Scanner::windows_helper()"),
            "the helper builds its scanner with Scanner::windows_helper"
        );
        assert!(
            !code.contains("Scanner::windows()"),
            "the helper must not scan with the unelevated Scanner::windows"
        );
        let scans: Vec<_> = code.match_indices(".scan(").collect();
        assert_eq!(scans.len(), 1, "one scan, through helper_scanner()");
        assert!(
            code[..scans[0].0].ends_with("helper_scanner()"),
            "the scan uses helper_scanner()"
        );
    }

    fn actions() -> Vec<PlannedAction> {
        vec![
            PlannedAction {
                entry_id: "openrgb".into(),
                action: Action::StopService {
                    name: "OpenRGB".into(),
                },
            },
            PlannedAction {
                entry_id: "openrgb".into(),
                action: Action::SetServiceStartType {
                    name: "OpenRGB".into(),
                    start_type: StartType::Manual,
                },
            },
        ]
    }

    #[test]
    fn request_json_shape_round_trips() {
        let request = ElevatedRequest::new(
            vec![PlannedAction {
                entry_id: "signalrgb".into(),
                action: Action::SetStartupApproved {
                    location: AutostartLocation::HklmRun32,
                    name: "SignalRgb".into(),
                    enabled: false,
                },
            }],
            Some(1),
        );
        let json = serde_json::to_value(&request).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "version": 1,
                "session_id": 1,
                "actions": [{
                    "entry_id": "signalrgb",
                    "action": { "kind": "set_startup_approved", "location": "hklm_run32", "name": "SignalRgb", "enabled": false }
                }]
            })
        );
        assert_eq!(parse_request(&json.to_string()), Ok(request));
    }

    #[test]
    fn requests_outside_the_fixed_enum_do_not_parse() {
        for text in [
            r#"{"version":1,"actions":[{"entry_id":"x","action":{"kind":"run_command","command":"cmd /c del"}}]}"#,
            r#"{"version":1,"actions":[{"entry_id":"x","action":{"kind":"set_service_start_type","name":"a","start_type":"delayed"}}]}"#,
            r#"{"version":1,"actions":[{"entry_id":"x","action":{"kind":"set_startup_approved","location":"hkcu_policies","name":"a","enabled":true}}]}"#,
            r#"{"version":1}"#,
            "not json",
        ] {
            assert!(parse_request(text).is_err(), "{text}");
        }
    }

    #[test]
    fn helper_mode_is_only_entered_with_the_flag() {
        assert_eq!(helper_main(["cc.exe", "--hidden"].map(OsString::from)), None);
        assert_eq!(helper_main(["cc.exe"].map(OsString::from)), None);
    }

    #[test]
    fn response_is_merged_by_position_and_identity() {
        let sent = actions();
        let ok = ElevatedResponse {
            version: ELEVATED_PROTOCOL_VERSION,
            outcomes: sent.iter().map(ActionOutcome::success).collect(),
        };
        assert!(merge_response(&sent, Some(ok.clone())).iter().all(|o| o.ok));

        // Swapped, truncated or foreign outcomes do not count.
        let mut swapped = ok.clone();
        swapped.outcomes.reverse();
        assert!(merge_response(&sent, Some(swapped)).iter().all(|o| !o.ok));
        let mut short = ok.clone();
        short.outcomes.truncate(1);
        let merged = merge_response(&sent, Some(short));
        assert_eq!(merged.iter().map(|o| o.ok).collect::<Vec<_>>(), [true, false]);
        let mut old = ok;
        old.version = 0;
        assert!(merge_response(&sent, Some(old)).iter().all(|o| !o.ok));
        let none = merge_response(&sent, None);
        assert_eq!(none.len(), 2);
        assert!(none.iter().all(|o| !o.ok && o.error.is_some()));
    }

    #[test]
    fn helper_refuses_oversized_or_missing_requests() {
        let dir = std::env::temp_dir().join(format!("cc-elevated-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let big = dir.join("big.json");
        std::fs::write(&big, vec![b' '; (MAX_REQUEST_BYTES + 1) as usize]).expect("write");
        assert!(read_request(&big).is_err());
        assert!(read_request(&dir.join("missing.json")).is_err());
        // A malformed request ends the helper before any scan or action, and writes no result.
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{").expect("write");
        assert_eq!(run_helper(&bad), EXIT_BAD_REQUEST);
        assert!(!result_path(&bad).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_reports_unknown_not_failed() {
        let outcomes = all_unknown(&actions());
        assert!(outcomes.iter().all(|o| !o.ok && o.unknown && o.error.is_some()));
    }

    #[test]
    fn stale_files_are_swept_by_age_and_name() {
        let dir = std::env::temp_dir().join(format!("cc-elevated-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        for name in ["request-1-2.json", "request-1-2.result.json", "keep.txt"] {
            std::fs::write(dir.join(name), "x").expect("write");
        }
        // Nothing is old yet.
        assert_eq!(sweep_dir(&dir, SystemTime::now(), STALE_AFTER), 0);
        // An hour and a bit later both helper files go, other files stay.
        let later = SystemTime::now() + STALE_AFTER + Duration::from_secs(60);
        assert_eq!(sweep_dir(&dir, later, STALE_AFTER), 2);
        assert!(dir.join("keep.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helper_refuses_paths_outside_the_request_folder() {
        let args = |p: &str| {
            [
                OsString::from("cc.exe"),
                OsString::from("--elevated-actions"),
                OsString::from(p),
            ]
        };
        let outside = std::env::temp_dir().join("evil.json");
        assert_eq!(helper_main(args(&outside.to_string_lossy())), Some(EXIT_BAD_ARGS));
        assert_eq!(helper_main(args("relative.json")), Some(EXIT_BAD_ARGS));
        // The right shape, but no such file: refused before anything is read or scanned.
        let missing = std::env::temp_dir()
            .join("CaseControl")
            .join("elevated")
            .join("request-0-0.json");
        assert_eq!(helper_main(args(&missing.to_string_lossy())), Some(EXIT_BAD_ARGS));
    }

    #[test]
    fn result_is_never_written_over_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("cc-elevated-test-w-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let target = dir.join("r.result.json");
        std::fs::write(&target, "keep").expect("write");
        let response = ElevatedResponse {
            version: ELEVATED_PROTOCOL_VERSION,
            outcomes: vec![],
        };
        assert!(write_new(&target, &response).is_err());
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "keep");
        let fresh = dir.join("n.result.json");
        write_new(&fresh, &response).expect("new file");
        assert!(
            std::fs::read_to_string(&fresh)
                .expect("read")
                .contains("\"version\":1")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
