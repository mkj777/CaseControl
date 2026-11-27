//! Windows system glue for CaseControl (ADR 0001 #42).
//!
//! What this crate does:
//! - [`task`]: the per-user Task Scheduler tasks. Pure XML builders ([`TaskSpec::to_xml`]) for the logon task
//!   (created by the user, no admin) and the start-up task (created only by the elevated helper), fixed task names
//!   derived from the user's SID, and the operations behind the [`TaskStore`] trait: register and verify by reading
//!   back ([`register_verified`], an own existing task is updated in place), delete only a task that is ours
//!   ([`delete_ours`]), and inspect ([`task_state`], including a disabled task).
//!   `WindowsTaskStore` talks to the Task Scheduler through COM (`ITaskService`); [`MemoryTaskStore`] is the fake.
//! - [`session_end`]: telling a Windows sign-out, shutdown / restart and a Restart Manager close apart from a normal
//!   quit, by subclassing the app's windows and recording the `WM_QUERYENDSESSION` / `WM_ENDSESSION` flags.
//! - [`lock`]: a `Global\` named mutex per user SID ([`DeviceLock`]) so the start-up instance (session 0) and the
//!   signed-in instance never drive devices at the same time, and a per-user yield event ([`YieldSignal`]) with
//!   which the signed-in instance asks the start-up instance to finish now.
//! - [`identity`]: the current user's SID, the interactive user of a session, the profile path from `ProfileList`
//!   and file owners.
//! - [`acl`]: the path and ACL check for the start-up task's executable ([`check_exe_path`]), with the pure DACL
//!   evaluation in [`acl::check_object`].
//!
//! What it deliberately does not do:
//! - No `schtasks.exe`, no shell, no `std::process::Command`, no network.
//! - Never registers a task for SYSTEM, a service account, with highest privileges or with a stored password.
//! - No registry writes; the registry is only read (`ProfileList`).
//! - It does not decide policy (when to create or remove a task, what to do at session end): the desktop app and
//!   the elevated helper in `cc-conflicts` do.
//!
//! All `unsafe` code lives in the private `sys` module (Windows only). On other platforms every system call returns
//! [`Error::Unsupported`] so the workspace still builds and the pure logic is tested everywhere.

pub mod acl;
pub mod identity;
pub mod lock;
pub mod session_end;
#[cfg(windows)]
#[allow(unsafe_code)]
mod sys;
pub mod task;

pub use acl::check_exe_path;
pub use identity::{
    Account, TokenElevation, current_session_id, current_user_sid, file_owner_sid, integrity_name,
    interactive_user, is_user_sid, profile_path, session_user, token_elevation,
};
pub use lock::{
    DeviceLock, LockStatus, SignalStatus, YieldSignal, device_lock_name, device_lock_sddl, request_yield,
    yield_signal_name,
};
pub use session_end::SessionEnd;
#[cfg(windows)]
pub use task::WindowsTaskStore;
pub use task::{
    BOOT_TASK_ARGUMENT, ExecAction, LOGON_TASK_ARGUMENT, LogonType, MemoryTaskStore, Ownership, RegisterMode,
    RunLevel, TaskInfo, TaskKind, TaskSpec, TaskState, TaskStore, TriggerKind, boot_task_name, delete_ours,
    is_apply_at_boot_invocation, logon_task_name, not_ours_message, register_verified, task_state,
};

/// Errors of this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A Windows call failed.
    #[error("{call} failed: {message} (0x{code:08X})")]
    Win32 {
        /// The function that failed.
        call: &'static str,
        /// HRESULT or Win32 error code.
        code: u32,
        /// System message.
        message: String,
    },
    /// An input was refused before anything was changed.
    #[error("{0}")]
    Invalid(String),
    /// A task read back after registering differs from what was intended (it was deleted again).
    #[error("the registered task differs from the intended one: {}", .0.join("; "))]
    Mismatch(Vec<String>),
    /// A task with our name exists but is not ours; it was left alone.
    #[error("{0}")]
    NotOurs(String),
    /// Not available on this platform.
    #[error("not supported on this platform")]
    Unsupported,
}

#[cfg(windows)]
impl Error {
    pub(crate) fn win32(call: &'static str, e: &windows::core::Error) -> Self {
        Error::Win32 {
            call,
            code: e.code().0 as u32,
            message: e.message(),
        }
    }
}
