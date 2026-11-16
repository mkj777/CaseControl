//! The only module of this crate that contains `unsafe` code: thin, safe wrappers around the Win32 calls for
//! process enumeration (Toolhelp), the service control manager (enumeration, stop, start type), process
//! termination and `ShellExecuteW` / `ShellExecuteExW` (UAC).
//!
//! Every function here is safe to call. Handles are owned by RAII guards and closed on drop. Buffers that the
//! system fills with structs are allocated as `Vec<u64>` so they are 8-byte aligned.

use std::mem::size_of;
use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA,
    ERROR_NO_MORE_FILES, ERROR_SERVICE_NOT_ACTIVE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW,
    ENUM_SERVICE_TYPE, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW,
    QueryServiceConfigW, QueryServiceStatus, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP,
    SERVICE_DEMAND_START, SERVICE_DRIVER, SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_WIN32,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, PWSTR};

use crate::actions::{ActionError, terminate_process_with};
use crate::elevation::ElevationError;
use crate::sources::{ScanError, ServiceInfo, ServiceKind, ServiceState, StartType};

fn win32(call: &'static str, e: &windows::core::Error) -> ScanError {
    ScanError::Win32 {
        call,
        message: e.message(),
    }
}

/// Owned kernel handle, closed on drop.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned valid by the creating call and is owned exclusively by this guard, so it
        // is closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Owned service control manager / service handle, closed on drop.
struct OwnedScHandle(SC_HANDLE);

impl Drop for OwnedScHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned valid by OpenSCManagerW / OpenServiceW and is owned exclusively by this
        // guard, so it is closed exactly once.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

/// Converts a NUL-terminated (or full) UTF-16 buffer to a `String`, lossily.
fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Reads a NUL-terminated wide string the system placed in a buffer we own. Null yields `None`.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string that stays valid for the duration of the call.
unsafe fn read_pwstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: `p` is non-null and, per this function's contract, points to a live NUL-terminated UTF-16 string.
    unsafe { p.to_string() }.ok()
}

/// All processes as `(pid, exe name)` from a Toolhelp snapshot.
pub fn process_list() -> Result<Vec<(u32, String)>, ScanError> {
    // SAFETY: plain FFI call with constant arguments; the returned handle is checked by the wrapper and owned below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| win32("CreateToolhelp32Snapshot", &e))?;
    let snapshot = OwnedHandle(snapshot);

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut out = Vec::new();
    // SAFETY: `snapshot.0` is a valid snapshot handle and `entry` is a properly sized PROCESSENTRY32W with dwSize
    // set, as the API requires.
    let mut result = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    while result.is_ok() {
        out.push((entry.th32ProcessID, from_wide(&entry.szExeFile)));
        // SAFETY: same invariants as for Process32FirstW above.
        result = unsafe { Process32NextW(snapshot.0, &mut entry) };
    }
    if let Err(e) = &result
        && e.code() != ERROR_NO_MORE_FILES.to_hresult()
    {
        tracing::debug!(
            code = e.code().0,
            message = %e.message(),
            listed = out.len(),
            "process enumeration stopped early"
        );
    }
    Ok(out)
}

/// Full image path of `pid`, `None` if the process cannot be opened (protected, other session, exited).
pub fn process_image_path(pid: u32) -> Option<String> {
    // SAFETY: plain FFI call; failure is reported through the Result and a success handle is owned below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let handle = OwnedHandle(handle);
    image_path(&handle).ok()
}

/// Full Win32 image path of the process behind `handle`, which must carry `PROCESS_QUERY_LIMITED_INFORMATION`.
fn image_path(handle: &OwnedHandle) -> windows::core::Result<String> {
    let mut buf = vec![0u16; 32 * 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `handle` is a live process handle opened with PROCESS_QUERY_LIMITED_INFORMATION; `buf` is writable
    // for `len` UTF-16 units, and `len` is a valid in/out pointer.
    unsafe { QueryFullProcessImageNameW(handle.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }?;
    let len = (len as usize).min(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

/// Terminates `pid` with exit code 1 if its image file name is `expected_name` (case-insensitive).
///
/// The process is opened once with `PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION`; the name check and
/// `TerminateProcess` both use that handle. An open handle keeps the process object alive, so the pid cannot be
/// reused by another process between the check and the kill. Same-user processes only; no privilege is enabled.
pub fn terminate_checked(pid: u32, expected_name: &str, reason: &str) -> Result<(), ActionError> {
    // SAFETY: plain FFI call; failure is reported through the Result and a success handle is owned below.
    let handle =
        match unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(handle) => OwnedHandle(handle),
            // OpenProcess reports a pid that does not exist as ERROR_INVALID_PARAMETER.
            Err(e) if e.code() == ERROR_INVALID_PARAMETER.to_hresult() => {
                return Err(ActionError::NotFound { pid });
            }
            Err(e) => {
                return Err(ActionError::Failed(format!(
                    "OpenProcess failed: {}",
                    e.message()
                )));
            }
        };
    terminate_process_with(
        pid,
        expected_name,
        reason,
        |_| {
            let path = image_path(&handle).map_err(|e| e.message())?;
            Ok(Some(file_name(&path).to_owned()))
        },
        |_| {
            // SAFETY: `handle` is a live process handle opened with PROCESS_TERMINATE and still owned by the guard.
            unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| e.message())
        },
    )
}

/// The file-name component of a Windows path (`C:\dir\tool.exe` -> `tool.exe`).
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Starts `file` with `parameters` through `ShellExecuteW` with the `runas` verb (shows UAC). Returns immediately.
///
/// `file` should be an absolute path: an unqualified name is resolved through a search path that includes the
/// current directory.
pub fn shell_execute_runas(file: &Path, parameters: &str) -> Result<(), String> {
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(file);
    let parameters = HSTRING::from(parameters);
    // SAFETY: all string arguments are NUL-terminated HSTRINGs that outlive the call; a null directory and no
    // owner window are allowed by the API.
    let result = unsafe { ShellExecuteW(None, &verb, &file, &parameters, PCWSTR::null(), SW_HIDE) };
    // ShellExecuteW returns a value greater than 32 on success, an error code otherwise.
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed with code {code}"))
    }
}

/// Allocates an 8-byte aligned buffer of at least `bytes` bytes.
fn aligned(bytes: usize) -> Vec<u64> {
    vec![0u64; bytes.div_ceil(8).max(1)]
}

/// Views an aligned buffer as bytes.
fn as_bytes(buf: &mut [u64]) -> &mut [u8] {
    let len = std::mem::size_of_val(buf);
    // SAFETY: `buf` is a live, exclusively borrowed slice of `len` bytes; u8 has no alignment or validity
    // requirements, and the returned slice borrows `buf` mutably so no aliasing is possible.
    unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), len) }
}

/// All Win32 services and driver services, with start type and binary path where readable.
pub fn service_list() -> Result<Vec<ServiceInfo>, ScanError> {
    // SAFETY: null machine and database names select the local active database; the handle is checked by the
    // wrapper and owned below.
    let scm = unsafe {
        OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
        )
    }
    .map_err(|e| win32("OpenSCManagerW", &e))?;
    let scm = OwnedScHandle(scm);

    let mut out = Vec::new();
    let mut resume: u32 = 0;
    let mut buf = aligned(64 * 1024);
    loop {
        let mut needed: u32 = 0;
        let mut returned: u32 = 0;
        // SAFETY: `scm` is a valid SCM handle with ENUMERATE_SERVICE; the buffer slice, the out counters and the
        // resume handle are valid writable locations for the duration of the call.
        let result = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32 | SERVICE_DRIVER,
                SERVICE_STATE_ALL,
                Some(as_bytes(&mut buf)),
                &mut needed,
                &mut returned,
                Some(&raw mut resume),
                PCWSTR::null(),
            )
        };
        let more = match &result {
            Ok(()) => false,
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() => true,
            Err(e) => return Err(win32("EnumServicesStatusExW", e)),
        };

        let count =
            (returned as usize).min(size_of_val(buf.as_slice()) / size_of::<ENUM_SERVICE_STATUS_PROCESSW>());
        // SAFETY: the call wrote `returned` ENUM_SERVICE_STATUS_PROCESSW records at the start of `buf`, which is
        // 8-byte aligned (sufficient for the struct) and large enough (`count` is clamped to the buffer size).
        let records =
            unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(), count) };
        for record in records {
            // SAFETY: the name pointers of each record point to NUL-terminated strings the system wrote into
            // `buf`, which is not modified or freed while `records` is in use.
            let (name, display_name) =
                unsafe { (read_pwstr(record.lpServiceName), read_pwstr(record.lpDisplayName)) };
            let Some(name) = name else { continue };
            let display_name = display_name.unwrap_or_default();
            let status = &record.ServiceStatusProcess;
            let kind = if status.dwServiceType.0 & SERVICE_DRIVER.0 != 0 {
                ServiceKind::Driver
            } else {
                ServiceKind::Win32
            };
            let state = match status.dwCurrentState {
                SERVICE_RUNNING => ServiceState::Running,
                SERVICE_STOPPED => ServiceState::Stopped,
                other => ServiceState::Other(other.0),
            };
            let (start_type, binary_path) = service_config(&scm, &name).unwrap_or((None, None));
            out.push(ServiceInfo {
                name,
                display_name,
                kind,
                state,
                start_type,
                binary_path,
            });
        }

        if !more {
            break;
        }
        if returned == 0 {
            // Nothing fit: grow to what the system asked for and retry from the same resume point.
            buf = aligned((needed as usize).max(size_of_val(buf.as_slice()) * 2));
        }
    }
    Ok(out)
}

/// Start type and binary path of one service; `None` when access is denied.
fn service_config(scm: &OwnedScHandle, name: &str) -> Option<(Option<StartType>, Option<String>)> {
    let wide = HSTRING::from(name);
    // SAFETY: `scm` is a valid SCM handle and `wide` a NUL-terminated name that outlives the call.
    let service = unsafe { OpenServiceW(scm.0, &wide, SERVICE_QUERY_CONFIG) }.ok()?;
    let service = OwnedScHandle(service);

    let mut needed: u32 = 0;
    // SAFETY: a null buffer with size 0 is the documented way to query the required size.
    let probe = unsafe { QueryServiceConfigW(service.0, None, 0, &mut needed) };
    match probe {
        Err(e) if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {}
        _ => return None,
    }
    let mut buf = aligned(needed as usize);
    let size = u32::try_from(size_of_val(buf.as_slice())).ok()?;
    let config = buf.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: `buf` is 8-byte aligned and at least `needed` bytes long, which is what the system asked for.
    unsafe { QueryServiceConfigW(service.0, Some(config), size, &mut needed) }.ok()?;
    // SAFETY: the call succeeded, so the start of `buf` holds an initialized QUERY_SERVICE_CONFIGW whose string
    // pointers point into `buf`, which stays alive until the end of this function.
    let config = unsafe { &*config };
    let start_type = match config.dwStartType.0 {
        0 => Some(StartType::Boot),
        1 => Some(StartType::System),
        2 => Some(StartType::Auto),
        3 => Some(StartType::Manual),
        4 => Some(StartType::Disabled),
        _ => None,
    };
    // SAFETY: lpBinaryPathName is null or points to a NUL-terminated string inside `buf`, which is still alive.
    let path = unsafe { read_pwstr(config.lpBinaryPathName) }.filter(|p| !p.is_empty());
    Some((start_type, path))
}

/// Opens service `name` on the local SCM with `access`.
fn open_service(name: &str, access: u32) -> Result<OwnedScHandle, String> {
    // SAFETY: null machine and database names select the local active database; the handle is checked by the
    // wrapper and owned below.
    let scm = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
        .map_err(|e| format!("OpenSCManagerW failed: {}", e.message()))?;
    let scm = OwnedScHandle(scm);
    let wide = HSTRING::from(name);
    // SAFETY: `scm` is a valid SCM handle and `wide` a NUL-terminated name that outlives the call.
    let service = unsafe { OpenServiceW(scm.0, &wide, access) }
        .map_err(|e| format!("OpenServiceW({name}) failed: {}", e.message()))?;
    Ok(OwnedScHandle(service))
}

/// Current status of the service behind `service` (opened with `SERVICE_QUERY_STATUS`).
fn service_status(service: &OwnedScHandle) -> Result<SERVICE_STATUS, String> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is a live service handle with SERVICE_QUERY_STATUS and `status` a writable struct.
    unsafe { QueryServiceStatus(service.0, &mut status) }
        .map_err(|e| format!("QueryServiceStatus failed: {}", e.message()))?;
    Ok(status)
}

/// Asks service `name` to stop and waits up to `wait` for it to reach the stopped state. A service that is not
/// running counts as stopped. Needs administrator rights for most services.
pub fn stop_service(name: &str, wait: std::time::Duration) -> Result<(), String> {
    let service = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is a live service handle opened with SERVICE_STOP and `status` a writable struct.
    match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_SERVICE_NOT_ACTIVE.to_hresult() => return Ok(()),
        Err(e) => return Err(format!("ControlService(stop {name}) failed: {}", e.message())),
    }
    let deadline = std::time::Instant::now() + wait;
    loop {
        if service_status(&service)?.dwCurrentState == SERVICE_STOPPED {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "service {name} was asked to stop but is still stopping after {} s",
                wait.as_secs()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Sets the start type of service `name` to Auto (`auto = true`) or Manual; the rest of its configuration is left
/// unchanged (`SERVICE_NO_CHANGE`), so a delayed-auto flag survives a Manual / Auto round trip.
pub fn set_service_start_type(name: &str, auto: bool) -> Result<(), String> {
    let service = open_service(name, SERVICE_CHANGE_CONFIG)?;
    let start = if auto {
        SERVICE_AUTO_START
    } else {
        SERVICE_DEMAND_START
    };
    // SAFETY: `service` is a live service handle opened with SERVICE_CHANGE_CONFIG; every other field is passed as
    // SERVICE_NO_CHANGE or null, which the API documents as "leave unchanged".
    unsafe {
        ChangeServiceConfigW(
            service.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            start,
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(|e| format!("ChangeServiceConfigW({name}) failed: {}", e.message()))
}

/// Starts `file` with `parameters` through `ShellExecuteExW` with the `runas` verb (one UAC prompt) and waits up to
/// `timeout` for it to exit. Returns its exit code.
pub fn run_as_admin_and_wait(
    file: &Path,
    parameters: &str,
    timeout: std::time::Duration,
) -> Result<u32, ElevationError> {
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(file);
    let parameters = HSTRING::from(parameters);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` is a properly sized SHELLEXECUTEINFOW whose string pointers point to HSTRINGs that outlive the
    // call; the system writes hProcess into it.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            ElevationError::Declined
        } else {
            ElevationError::Failed(format!("ShellExecuteExW failed: {}", e.message()))
        });
    }
    if info.hProcess.is_invalid() {
        return Err(ElevationError::Failed(
            "ShellExecuteExW returned no process handle".to_owned(),
        ));
    }
    let process = OwnedHandle(info.hProcess);
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: `process` is the live process handle ShellExecuteExW returned (SEE_MASK_NOCLOSEPROCESS).
    let waited = unsafe { WaitForSingleObject(process.0, millis) };
    if waited == WAIT_TIMEOUT {
        return Err(ElevationError::Timeout(timeout.as_secs()));
    }
    if waited != WAIT_OBJECT_0 {
        return Err(ElevationError::Failed(format!(
            "waiting for the elevated helper failed ({})",
            waited.0
        )));
    }
    let mut code = 0u32;
    // SAFETY: `process` is a live process handle and `code` a writable u32.
    unsafe { GetExitCodeProcess(process.0, &mut code) }
        .map_err(|e| ElevationError::Failed(format!("GetExitCodeProcess failed: {}", e.message())))?;
    Ok(code)
}
