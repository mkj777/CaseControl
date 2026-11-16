//! The elevated helper: one UAC prompt per Proceed (ADR 0001 #31).
//!
//! App side ([`run_admin_batch`]): writes the actions that need administrator rights as an
//! [`ElevatedRequest`] (JSON) to a new file under `%LOCALAPPDATA%\CaseControl\elevated`, starts its own executable
//! elevated with `--elevated-actions "<file>"`, waits for it (at most [`HELPER_TIMEOUT`]), reads the
//! [`ElevatedResponse`] from the result file next to it and deletes both files.
//!
//! Helper side ([`helper_main`], called from `main` before Tauri, the single-instance guard or the engine start):
//! reads the request (at most [`MAX_REQUEST_BYTES`]), runs a fresh conflict scan, and lets
//! [`cc_conflicts::run_elevated_request`] validate every action against it (fixed action enum, catalogue-matched
//! names only, HKCU writes refused) and run what passes through `WindowsOps`. No shell is started. The response is
//! written to a new file (never over an existing one), then the helper exits.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cc_conflicts::{
    ActionOutcome, ELEVATED_PROTOCOL_VERSION, ElevatedRequest, ElevatedResponse, HELPER_TIMEOUT,
    PlannedAction, helper_request_path, is_helper_invocation, result_path,
};

/// Largest request file the helper reads.
pub const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Helper exit code: the response was written.
pub const EXIT_OK: i32 = 0;
/// Helper exit code: the command line was malformed.
pub const EXIT_BAD_ARGS: i32 = 2;
/// Helper exit code: the request could not be read or parsed.
pub const EXIT_BAD_REQUEST: i32 = 3;
/// Helper exit code: the response could not be written.
pub const EXIT_WRITE_FAILED: i32 = 4;

/// Entry point for `main`: `None` for a normal start, `Some(exit code)` when the command line asks for helper mode
/// (the helper then already ran).
pub fn helper_main(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let args: Vec<OsString> = args.into_iter().collect();
    if !is_helper_invocation(args.iter().cloned()) {
        return None;
    }
    crate::logging::init();
    let Some(request) = helper_request_path(args) else {
        tracing::error!("elevated helper started with a malformed command line; doing nothing");
        return Some(EXIT_BAD_ARGS);
    };
    Some(run_helper(&request))
}

/// Reads the request file, refusing anything larger than [`MAX_REQUEST_BYTES`].
fn read_request(path: &Path) -> Result<ElevatedRequest, String> {
    let len = std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .len();
    if len > MAX_REQUEST_BYTES {
        return Err(format!(
            "request file is {len} bytes, more than {MAX_REQUEST_BYTES}"
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_request(&text)
}

/// Parses a request (JSON).
pub fn parse_request(text: &str) -> Result<ElevatedRequest, String> {
    serde_json::from_str(text).map_err(|e| format!("malformed request: {e}"))
}

/// Writes `response` to `path`, which must not exist yet.
fn write_new(path: &Path, response: &ElevatedResponse) -> std::io::Result<()> {
    let text = serde_json::to_vec(response).map_err(std::io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&text)?;
    file.sync_all()
}

/// The helper: validate and run the request in `request`, write the response next to it.
pub fn run_helper(request: &Path) -> i32 {
    tracing::info!(request = %request.display(), "elevated helper started");
    let parsed = match read_request(request) {
        Ok(r) => r,
        Err(err) => {
            tracing::error!(%err, "elevated helper: bad request; doing nothing");
            return EXIT_BAD_REQUEST;
        }
    };
    let response = execute_request(&parsed);
    match write_new(&result_path(request), &response) {
        Ok(()) => {
            tracing::info!(
                actions = response.outcomes.len(),
                failed = response.outcomes.iter().filter(|o| !o.ok).count(),
                "elevated helper finished"
            );
            EXIT_OK
        }
        Err(err) => {
            tracing::error!(%err, "elevated helper: cannot write the result");
            EXIT_WRITE_FAILED
        }
    }
}

#[cfg(windows)]
fn execute_request(request: &ElevatedRequest) -> ElevatedResponse {
    let report = cc_conflicts::Scanner::windows().scan(&cc_conflicts::ScanOptions::default());
    cc_conflicts::run_elevated_request(request, &report, &cc_conflicts::WindowsOps)
}

#[cfg(not(windows))]
fn execute_request(request: &ElevatedRequest) -> ElevatedResponse {
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

/// Runs `actions` (all needing administrator rights, or terminations that failed without them) in one elevated
/// helper invocation. Shows exactly one UAC prompt. Never fails as a whole: every problem becomes a failed outcome.
pub fn run_admin_batch(actions: &[PlannedAction]) -> Vec<ActionOutcome> {
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
    let outcomes = run_helper_for(&request_file, &result_file, actions);
    for file in [&request_file, &result_file] {
        if let Err(err) = std::fs::remove_file(file)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(file = %file.display(), %err, "cannot delete elevated helper file");
        }
    }
    outcomes
}

fn run_helper_for(request_file: &Path, result_file: &Path, actions: &[PlannedAction]) -> Vec<ActionOutcome> {
    let request = ElevatedRequest::new(actions.to_vec());
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
        return all_failed(actions, &format!("cannot write the elevated request: {err}"));
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => return all_failed(actions, &format!("cannot locate CaseControl's executable: {err}")),
    };
    match cc_conflicts::run_elevated_helper(&exe, request_file, HELPER_TIMEOUT) {
        Ok(code) => {
            let response = std::fs::read_to_string(result_file)
                .ok()
                .and_then(|text| serde_json::from_str::<ElevatedResponse>(&text).ok());
            if response.is_none() {
                tracing::warn!(code, "elevated helper exited without a readable result");
            }
            merge_response(actions, response)
        }
        Err(err) => {
            tracing::warn!(%err, "elevated helper did not run");
            all_failed(actions, &err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use cc_conflicts::{Action, AutostartLocation, StartType};

    use super::*;

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
        let request = ElevatedRequest::new(vec![PlannedAction {
            entry_id: "signalrgb".into(),
            action: Action::SetStartupApproved {
                location: AutostartLocation::HklmRun32,
                name: "SignalRgb".into(),
                enabled: false,
            },
        }]);
        let json = serde_json::to_value(&request).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({
                "version": 1,
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
