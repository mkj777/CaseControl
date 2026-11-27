//! The only module of this crate that contains `unsafe` code: thin, safe wrappers around the Win32 and COM calls
//! for the Task Scheduler (`ITaskService`), window subclassing, the named mutex, token / session / account queries
//! and file security descriptors.
//!
//! Every function here is safe to call. Kernel handles, security descriptors and WTS buffers are owned by RAII
//! guards and released on drop; COM interfaces release themselves and are dropped before COM is uninitialised.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
    GetLastError, HANDLE, HLOCAL, HWND, LPARAM, LRESULT, LocalFree, RPC_E_CHANGED_MODE, VARIANT_BOOL,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
    GetNamedSecurityInfoW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, DACL_SECURITY_INFORMATION, GetAce,
    GetAclInformation, GetTokenInformation, LookupAccountNameW, LookupAccountSidW,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SID_NAME_USE,
    TOKEN_ELEVATION_TYPE, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
    TokenElevationType, TokenIntegrityLevel, TokenUser,
};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTS_CURRENT_SERVER_HANDLE, WTS_INFO_CLASS, WTSDomainName, WTSFreeMemory,
    WTSQuerySessionInformationW, WTSUserName,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ILogonTrigger, ITaskFolder, ITaskService, TASK_ACTION_EXEC, TASK_ACTION_TYPE, TASK_CREATE,
    TASK_CREATE_OR_UPDATE, TASK_LOGON_TYPE, TASK_RUNLEVEL_TYPE, TASK_TRIGGER_LOGON, TASK_TRIGGER_TYPE2,
    TaskScheduler,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, GetCurrentProcess, GetCurrentThreadId, OpenProcessToken, ReleaseMutex,
    SetEvent, WaitForSingleObject,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GetClassNameW, GetSystemMetrics, GetWindowThreadProcessId, IsWindow, SM_SHUTTINGDOWN,
    WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION,
};
use windows::core::{BOOL, BSTR, HSTRING, Interface, PCWSTR, PWSTR};

use crate::Error;
use crate::acl::{Ace, AceKind, ObjectSecurity};
use crate::identity::Account;
use crate::session_end::{Installed, SessionMessage};
use crate::task::{ExecAction, LogonType, RunLevel, TaskInfo, TriggerKind};

/// Owned kernel handle, closed on drop.
#[derive(Debug)]
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned valid by the creating call and is owned exclusively by this guard, so it
        // is closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Memory the system allocated with `LocalAlloc` (security descriptors, SID strings), freed on drop.
struct LocalMem(*mut c_void);

impl Drop for LocalMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer was allocated by the system with LocalAlloc for us to free, and this guard owns
            // it exclusively, so it is freed exactly once.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}

/// A WTS buffer, freed with `WTSFreeMemory` on drop.
struct WtsMem(PWSTR);

impl Drop for WtsMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the buffer was allocated by WTSQuerySessionInformationW and is owned exclusively by this guard.
            unsafe { WTSFreeMemory(self.0.0.cast()) };
        }
    }
}

fn hresult_is(e: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    e.code() == code.to_hresult()
}

// ---------------------------------------------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------------------------------------------

/// Windows session of `pid`.
pub fn process_session(pid: u32) -> Result<u32, Error> {
    let mut session = 0u32;
    // SAFETY: plain FFI call; `session` is a writable u32 for the duration of the call.
    unsafe { ProcessIdToSessionId(pid, &mut session) }
        .map_err(|e| Error::win32("ProcessIdToSessionId", &e))?;
    Ok(session)
}

/// The string form of `sid`.
///
/// `sid` must point to a valid SID for the duration of the call (callers pass SIDs inside buffers they own).
fn sid_to_string(sid: PSID) -> Result<String, Error> {
    let mut text = PWSTR::null();
    // SAFETY: `sid` points to a valid SID per this function's contract; `text` receives a LocalAlloc'd string that
    // is owned by the guard below.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }
        .map_err(|e| Error::win32("ConvertSidToStringSidW", &e))?;
    let _guard = LocalMem(text.0.cast());
    // SAFETY: on success `text` is a NUL-terminated UTF-16 string that stays alive until `_guard` drops.
    unsafe { text.to_string() }.map_err(|e| Error::Invalid(format!("SID string is not UTF-16: {e}")))
}

/// SID of the current process token's user.
pub fn token_user_sid() -> Result<String, Error> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing; `token` receives a new handle that is
    // owned below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| Error::win32("OpenProcessToken", &e))?;
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    // SAFETY: a null buffer of length 0 is the documented way to ask for the required size; the call fails with
    // ERROR_INSUFFICIENT_BUFFER, which is expected and ignored.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    let size = u32::try_from(std::mem::size_of_val(buf.as_slice()))
        .map_err(|_| Error::Invalid("token information is too large".into()))?;
    // SAFETY: `buf` is 8-byte aligned (enough for TOKEN_USER) and `size` bytes long, which is at least `needed`.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            size,
            &mut needed,
        )
    }
    .map_err(|e| Error::win32("GetTokenInformation", &e))?;
    // SAFETY: the call succeeded, so the start of `buf` holds a TOKEN_USER whose SID pointer points into `buf`,
    // which stays alive and unchanged until the end of this function.
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };
    sid_to_string(user.User.Sid)
}

/// The current process token's information of `class`, in an 8-byte aligned buffer.
fn own_token_info(class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>, Error> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing; `token` receives a new handle that is
    // owned below.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| Error::win32("OpenProcessToken", &e))?;
    let token = OwnedHandle(token);
    let mut needed = 0u32;
    // SAFETY: a null buffer of length 0 is the documented way to ask for the required size; the call fails with
    // ERROR_INSUFFICIENT_BUFFER (or succeeds for fixed-size classes), which is expected and ignored.
    let _ = unsafe { GetTokenInformation(token.0, class, None, 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    let size = u32::try_from(std::mem::size_of_val(buf.as_slice()))
        .map_err(|_| Error::Invalid("token information is too large".into()))?;
    // SAFETY: `buf` is 8-byte aligned (enough for every TOKEN_* structure) and `size` bytes long, at least `needed`.
    unsafe { GetTokenInformation(token.0, class, Some(buf.as_mut_ptr().cast()), size, &mut needed) }
        .map_err(|e| Error::win32("GetTokenInformation", &e))?;
    Ok(buf)
}

/// The current process token's elevation type (`TOKEN_ELEVATION_TYPE`: 1 default, 2 full, 3 limited) and the SID of
/// its integrity level (`S-1-16-<rid>`).
pub fn token_elevation() -> Result<(i32, String), Error> {
    let elevation = own_token_info(TokenElevationType)?;
    // SAFETY: the call succeeded, so the buffer starts with a TOKEN_ELEVATION_TYPE (an i32) and is aligned for it.
    let elevation = unsafe { *elevation.as_ptr().cast::<TOKEN_ELEVATION_TYPE>() }.0;
    let label = own_token_info(TokenIntegrityLevel)?;
    // SAFETY: the call succeeded, so the start of `label` holds a TOKEN_MANDATORY_LABEL whose SID pointer points into
    // `label`, which stays alive and unchanged until the end of this function.
    let mandatory = unsafe { &*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
    Ok((elevation, sid_to_string(mandatory.Label.Sid)?))
}

fn wts_string(session: u32, class: WTS_INFO_CLASS, what: &'static str) -> Result<String, Error> {
    let mut buffer = PWSTR::null();
    let mut bytes = 0u32;
    // SAFETY: the current-server pseudo handle needs no opening or closing; `buffer` receives a WTS allocation
    // that is owned by the guard below.
    unsafe {
        WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            session,
            class,
            &mut buffer,
            &mut bytes,
        )
    }
    .map_err(|e| Error::win32(what, &e))?;
    let guard = WtsMem(buffer);
    if guard.0.is_null() {
        return Ok(String::new());
    }
    // SAFETY: for the user and domain classes the buffer is a NUL-terminated UTF-16 string, alive until `guard`
    // drops at the end of this function.
    unsafe { guard.0.to_string() }.map_err(|e| Error::Invalid(format!("{what}: {e}")))
}

/// The SID of account `name` (`DOMAIN\user`, `user` or a SID string returned as is).
pub fn account_sid(name: &str) -> Result<String, Error> {
    if name.starts_with("S-1-") {
        return Ok(name.to_owned());
    }
    let wide = HSTRING::from(name);
    let mut sid_len = 0u32;
    let mut domain_len = 0u32;
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: null buffers with zero lengths ask for the required sizes; the expected failure is ignored.
    let _ = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &wide,
            None,
            &mut sid_len,
            None,
            &mut domain_len,
            &mut use_,
        )
    };
    if sid_len == 0 {
        return Err(Error::Invalid(format!("account {name:?} is not known")));
    }
    let mut sid = vec![0u64; (sid_len as usize).div_ceil(8)];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    // SAFETY: `sid` holds at least `sid_len` bytes (8-byte aligned) and `domain` `domain_len` UTF-16 units, the
    // sizes the first call asked for; the lengths are valid in/out pointers.
    unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &wide,
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut use_,
        )
    }
    .map_err(|e| Error::win32("LookupAccountNameW", &e))?;
    sid_to_string(PSID(sid.as_mut_ptr().cast()))
}

/// The account name of `sid` as `DOMAIN\user` (the form the Task Scheduler stores a logon trigger's user in), via
/// `LookupAccountSidW`. Fails for a malformed SID or one that cannot be resolved (for example an offline domain).
pub fn account_name(sid: &str) -> Result<String, Error> {
    let wide = HSTRING::from(sid);
    let mut psid = PSID::default();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string alive for the call; `psid` receives a LocalAlloc'd SID that is
    // owned by the guard below.
    unsafe { ConvertStringSidToSidW(&wide, &mut psid) }
        .map_err(|e| Error::win32("ConvertStringSidToSidW", &e))?;
    let _guard = LocalMem(psid.0);
    let mut name_len = 0u32;
    let mut domain_len = 0u32;
    let mut use_ = SID_NAME_USE::default();
    // SAFETY: `psid` is a valid SID (owned by `_guard`); null buffers with zero lengths ask for the required sizes,
    // and the expected failure is ignored.
    let _ = unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            psid,
            None,
            &mut name_len,
            None,
            &mut domain_len,
            &mut use_,
        )
    };
    if name_len == 0 {
        return Err(Error::Invalid(format!("SID {sid:?} has no known account")));
    }
    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    // SAFETY: `name` holds `name_len` and `domain` at least `domain_len` UTF-16 units, the sizes the first call asked
    // for; the lengths are valid in/out pointers and `psid` is still valid.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            psid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut use_,
        )
    }
    .map_err(|e| Error::win32("LookupAccountSidW", &e))?;
    // On success the lengths exclude the terminating NUL.
    let name = String::from_utf16_lossy(&name[..(name_len as usize).min(name.len())]);
    let domain = String::from_utf16_lossy(&domain[..(domain_len as usize).min(domain.len())]);
    Ok(if domain.is_empty() {
        name
    } else {
        format!("{domain}\\{name}")
    })
}

/// The user signed in to `session`.
pub fn session_user(session: u32) -> Result<Account, Error> {
    let name = wts_string(session, WTSUserName, "WTSQuerySessionInformationW(WTSUserName)")?;
    let domain = wts_string(
        session,
        WTSDomainName,
        "WTSQuerySessionInformationW(WTSDomainName)",
    )?;
    if name.is_empty() {
        return Err(Error::Invalid(format!(
            "nobody is signed in to session {session}"
        )));
    }
    let qualified = if domain.is_empty() {
        name.clone()
    } else {
        format!("{domain}\\{name}")
    };
    Ok(Account {
        sid: account_sid(&qualified)?,
        domain,
        name,
    })
}

/// `ProfileImagePath` of `sid` (read only), with environment variables expanded.
pub fn profile_path(sid: &str) -> Result<PathBuf, Error> {
    let key = format!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}");
    let raw = windows_registry::LOCAL_MACHINE
        .open(&key)
        .and_then(|k| k.get_hstring("ProfileImagePath"))
        .map_err(|e| Error::Invalid(format!("no profile for {sid}: {}", e.message())))?;
    let raw = raw.to_string_lossy();
    if !raw.contains('%') {
        return Ok(PathBuf::from(raw));
    }
    let src = HSTRING::from(raw.as_str());
    // SAFETY: `src` is a NUL-terminated string that outlives the call; no output buffer asks for the size.
    let needed = unsafe { ExpandEnvironmentStringsW(&src, None) };
    if needed == 0 {
        return Err(Error::Invalid(format!("cannot expand {raw:?}")));
    }
    let mut buf = vec![0u16; needed as usize];
    // SAFETY: `buf` is writable for `needed` UTF-16 units, the size the first call asked for.
    let written = unsafe { ExpandEnvironmentStringsW(&src, Some(&mut buf)) } as usize;
    if written == 0 || written > buf.len() {
        return Err(Error::Invalid(format!("cannot expand {raw:?}")));
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

// ---------------------------------------------------------------------------------------------------------------
// File security
// ---------------------------------------------------------------------------------------------------------------

/// Owner and DACL of file or folder `path`.
pub fn object_security(path: &Path) -> Result<ObjectSecurity, Error> {
    let wide = HSTRING::from(path);
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `wide` is a NUL-terminated path that outlives the call; the out pointers are valid; the returned
    // security descriptor is owned by the guard below and `owner` / `dacl` point into it.
    let status = unsafe {
        GetNamedSecurityInfoW(
            &wide,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut sd,
        )
    };
    let _guard = LocalMem(sd.0);
    status
        .ok()
        .map_err(|e| Error::win32("GetNamedSecurityInfoW", &e))?;
    let owner = if owner.0.is_null() {
        None
    } else {
        Some(sid_to_string(owner)?)
    };
    let dacl = if dacl.is_null() {
        None
    } else {
        Some(read_acl(dacl)?)
    };
    Ok(ObjectSecurity { owner, dacl })
}

/// The entries of `acl`, which must point to a valid ACL that outlives the call.
fn read_acl(acl: *const ACL) -> Result<Vec<Ace>, Error> {
    let mut info = ACL_SIZE_INFORMATION::default();
    // SAFETY: `acl` is a valid ACL (inside a security descriptor the caller keeps alive); `info` is a writable
    // ACL_SIZE_INFORMATION of the size passed.
    unsafe {
        GetAclInformation(
            acl,
            (&raw mut info).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    }
    .map_err(|e| Error::win32("GetAclInformation", &e))?;
    let mut out = Vec::with_capacity(info.AceCount as usize);
    for i in 0..info.AceCount {
        let mut ace: *mut c_void = std::ptr::null_mut();
        // SAFETY: `i` is below the ACE count of the valid ACL; `ace` receives a pointer into the ACL.
        unsafe { GetAce(acl, i, &mut ace) }.map_err(|e| Error::win32("GetAce", &e))?;
        // SAFETY: every ACE starts with an ACE_HEADER; the pointer is valid while the ACL lives. ACEs are DWORD
        // aligned, enough for ACE_HEADER (alignment 2).
        let header = unsafe { *ace.cast::<ACE_HEADER>() };
        let kind = match header.AceType {
            0 => AceKind::Allow,
            1 | 6 | 10 | 12 => AceKind::Deny,
            4 | 5 | 9 | 11 => AceKind::OtherAllow,
            _ => AceKind::Other,
        };
        let (mask, sid) = if matches!(kind, AceKind::Allow | AceKind::Deny) && header.AceType <= 1 {
            // ACCESS_ALLOWED_ACE and ACCESS_DENIED_ACE share the layout: header, mask, SID.
            // SAFETY: the ACE type says the layout is ACCESS_ALLOWED_ACE / ACCESS_DENIED_ACE (header, u32 mask,
            // SID starting at SidStart); ACEs are DWORD aligned; the memory lives as long as the ACL.
            let allowed = ace.cast::<windows::Win32::Security::ACCESS_ALLOWED_ACE>();
            // SAFETY: see above; reading the mask field of a valid ACE.
            let mask = unsafe { (*allowed).Mask };
            // SAFETY: see above; SidStart is the first DWORD of the SID stored inline in the ACE.
            let sid_ptr = unsafe { &raw const (*allowed).SidStart };
            (mask, Some(sid_to_string(PSID(sid_ptr.cast_mut().cast()))?))
        } else if !matches!(kind, AceKind::Other) {
            // Object and callback ACEs: the mask follows the header in every layout.
            // SAFETY: all access ACE layouts put a u32 mask right after the 4-byte header; the ACE is DWORD aligned.
            let mask = unsafe { *ace.cast::<u8>().add(4).cast::<u32>() };
            (mask, None)
        } else {
            (0, None)
        };
        out.push(Ace {
            kind,
            sid,
            mask,
            flags: header.AceFlags,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// Named mutex
// ---------------------------------------------------------------------------------------------------------------

/// An open mutex handle. On drop it releases the mutex (harmless when this thread does not own it) and closes the
/// handle.
#[derive(Debug)]
pub struct MutexHandle(OwnedHandle);

impl Drop for MutexHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is a live mutex handle owned by this guard; ReleaseMutex fails harmlessly when the
        // calling thread does not own the mutex.
        let _ = unsafe { ReleaseMutex(self.0.0) };
    }
}

/// Result of creating or opening a named kernel object.
pub enum NamedOpen<T> {
    /// Opened or created; `existed` when the object was there before (then its security descriptor is not ours by
    /// construction and must be checked).
    Handle {
        /// The handle.
        handle: T,
        /// Whether the object already existed.
        existed: bool,
    },
    /// It exists and this user may not open it.
    AccessDenied,
}

/// Runs `create` with security attributes built from `sddl` (a self-relative descriptor alive for the call).
fn with_sddl<T>(sddl: &str, create: impl FnOnce(&SECURITY_ATTRIBUTES) -> T) -> Result<T, Error> {
    let sddl = HSTRING::from(sddl);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` is a NUL-terminated string that outlives the call; the descriptor is LocalAlloc'd and owned by
    // the guard below.
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut sd, None) }
        .map_err(|e| Error::win32("ConvertStringSecurityDescriptorToSecurityDescriptorW", &e))?;
    let _guard = LocalMem(sd.0);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    Ok(create(&attributes))
}

/// Whether the last error of this thread is `ERROR_ALREADY_EXISTS`. Call it directly after a successful
/// `Create*W` of a named object.
fn already_existed() -> bool {
    // SAFETY: plain FFI call reading this thread's last-error value.
    unsafe { GetLastError() == ERROR_ALREADY_EXISTS }
}

/// Owner and DACL of the kernel object behind `handle` (which must carry `READ_CONTROL`).
fn handle_security(handle: HANDLE) -> Result<ObjectSecurity, Error> {
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `handle` is a live handle owned by the caller; the out pointers are valid; the returned security
    // descriptor is owned by the guard below and `owner` / `dacl` point into it.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut sd),
        )
    };
    let _guard = LocalMem(sd.0);
    status.ok().map_err(|e| Error::win32("GetSecurityInfo", &e))?;
    let owner = if owner.0.is_null() {
        None
    } else {
        Some(sid_to_string(owner)?)
    };
    let dacl = if dacl.is_null() {
        None
    } else {
        Some(read_acl(dacl)?)
    };
    Ok(ObjectSecurity { owner, dacl })
}

/// Creates (or opens) mutex `name` with security descriptor `sddl` for a new object.
pub fn create_mutex(name: &str, sddl: &str) -> Result<NamedOpen<MutexHandle>, Error> {
    let name = HSTRING::from(name);
    let result = with_sddl(sddl, |attributes| {
        // SAFETY: `attributes` points to a valid self-relative security descriptor alive for the call; `name` is
        // NUL-terminated and outlives the call; the handle is owned below. The last error is read right after the
        // call.
        unsafe { CreateMutexW(Some(attributes), false, &name) }.map(|h| (h, already_existed()))
    })?;
    match result {
        Ok((handle, existed)) => Ok(NamedOpen::Handle {
            handle: MutexHandle(OwnedHandle(handle)),
            existed,
        }),
        Err(e) if hresult_is(&e, ERROR_ACCESS_DENIED) => Ok(NamedOpen::AccessDenied),
        Err(e) => Err(Error::win32("CreateMutexW", &e)),
    }
}

/// Owner and DACL of an open mutex.
pub fn mutex_security(mutex: &MutexHandle) -> Result<ObjectSecurity, Error> {
    handle_security(mutex.0.0)
}

/// An open manual-reset event, closed on drop.
#[derive(Debug)]
pub struct EventHandle(OwnedHandle);

// SAFETY: an event handle may be used and closed from any thread; the handle is owned exclusively by this value and
// every operation on it (SetEvent, WaitForSingleObject, CloseHandle) is thread-safe in the kernel.
unsafe impl Send for EventHandle {}
// SAFETY: see above; `&EventHandle` only allows SetEvent and waits, which are thread-safe.
unsafe impl Sync for EventHandle {}

/// Creates (or opens) the manual-reset event `name` (initially not set) with security descriptor `sddl` for a new
/// object.
pub fn create_event(name: &str, sddl: &str) -> Result<NamedOpen<EventHandle>, Error> {
    let name = HSTRING::from(name);
    let result = with_sddl(sddl, |attributes| {
        // SAFETY: as in `create_mutex`.
        unsafe { CreateEventW(Some(attributes), true, false, &name) }.map(|h| (h, already_existed()))
    })?;
    match result {
        Ok((handle, existed)) => Ok(NamedOpen::Handle {
            handle: EventHandle(OwnedHandle(handle)),
            existed,
        }),
        Err(e) if hresult_is(&e, ERROR_ACCESS_DENIED) => Ok(NamedOpen::AccessDenied),
        Err(e) => Err(Error::win32("CreateEventW", &e)),
    }
}

/// Owner and DACL of an open event.
pub fn event_security(event: &EventHandle) -> Result<ObjectSecurity, Error> {
    handle_security(event.0.0)
}

/// Sets the event.
pub fn set_event(event: &EventHandle) -> Result<(), Error> {
    // SAFETY: the handle is a live event handle owned by `event`.
    unsafe { SetEvent(event.0.0) }.map_err(|e| Error::win32("SetEvent", &e))
}

/// Waits up to `timeout` for the event; whether it is set.
pub fn wait_event(event: &EventHandle, timeout: Duration) -> Result<bool, Error> {
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: the handle is a live event handle owned by `event`.
    match unsafe { WaitForSingleObject(event.0.0, millis) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        other => Err(Error::Win32 {
            call: "WaitForSingleObject",
            code: other.0,
            message: "the wait failed".into(),
        }),
    }
}

/// How a wait on the mutex ended.
pub enum WaitOutcome {
    /// Acquired.
    Acquired,
    /// Acquired; the previous owner ended without releasing it.
    Abandoned,
    /// Still held by another thread.
    Timeout,
}

/// Waits up to `timeout` for the mutex.
pub fn wait_mutex(mutex: &MutexHandle, timeout: Duration) -> Result<WaitOutcome, Error> {
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
    // SAFETY: the handle is a live mutex handle owned by `mutex`.
    let result = unsafe { WaitForSingleObject(mutex.0.0, millis) };
    match result {
        WAIT_OBJECT_0 => Ok(WaitOutcome::Acquired),
        WAIT_ABANDONED => Ok(WaitOutcome::Abandoned),
        WAIT_TIMEOUT => Ok(WaitOutcome::Timeout),
        other => Err(Error::Win32 {
            call: "WaitForSingleObject",
            code: other.0,
            message: "the wait failed".into(),
        }),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Session end
// ---------------------------------------------------------------------------------------------------------------

/// Arbitrary id of CaseControl's subclass ("CCSE").
const SUBCLASS_ID: usize = 0x4343_5345;
const TAO_TARGET_CLASS: &str = "Tao Thread Event Target";

/// `GetDriveTypeW` of drive root `root` (for example `C:\\`).
pub fn drive_type(root: &str) -> u32 {
    let root = HSTRING::from(root);
    // SAFETY: `root` is a NUL-terminated string that outlives the call.
    unsafe { GetDriveTypeW(&root) }
}

/// `GetSystemMetrics(SM_SHUTTINGDOWN)`.
pub fn shutting_down() -> bool {
    // SAFETY: plain FFI call with a constant index.
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

/// The subclass procedure: records session messages, always passes the message on.
///
/// # Safety
/// Called by the system for windows subclassed with [`SetWindowSubclass`] on this thread.
unsafe extern "system" fn session_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_QUERYENDSESSION => crate::session_end::record(SessionMessage::Query {
            flags: lparam.0 as u32,
        }),
        WM_ENDSESSION => crate::session_end::record(SessionMessage::End {
            ending: wparam.0 != 0,
            flags: lparam.0 as u32,
        }),
        WM_NCDESTROY => {
            // SAFETY: `hwnd` is the window being destroyed on its own thread; removing our subclass there is what
            // the documentation recommends.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(session_subclass), SUBCLASS_ID) };
        }
        _ => {}
    }
    // SAFETY: called from within a subclass procedure with the arguments the system passed in.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Collects the windows `EnumThreadWindows` reports into the `Vec<HWND>` behind `lparam`.
///
/// # Safety
/// `lparam` must be a pointer to a live `Vec<HWND>` that nothing else accesses during the enumeration.
unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: per this function's contract `lparam` points to a live, exclusively used Vec<HWND>.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    true.into()
}

/// Whether `hwnd` is a window of the calling thread.
fn is_own_window(hwnd: HWND) -> bool {
    // SAFETY: IsWindow accepts any value and only reports whether it names an existing window.
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return false;
    }
    // SAFETY: plain FFI calls; `hwnd` names an existing window (it may be destroyed concurrently only by its own
    // thread, which is this one).
    unsafe { GetWindowThreadProcessId(hwnd, None) == GetCurrentThreadId() }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: `hwnd` is a window of this thread and `buf` a writable buffer of the given length.
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..usize::try_from(len).unwrap_or(0).min(buf.len())])
}

fn subclass(hwnd: HWND) -> Result<(), Error> {
    // SAFETY: `hwnd` is a window of the calling thread (checked by the callers), which SetWindowSubclass requires;
    // the procedure is a static function and the reference data is unused.
    let ok = unsafe { SetWindowSubclass(hwnd, Some(session_subclass), SUBCLASS_ID, 0) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(Error::Invalid("SetWindowSubclass failed".into()))
    }
}

/// Subclasses `hwnd` and tao's event-target window of the calling thread.
pub fn install_session_hooks(hwnd: isize) -> Result<Installed, Error> {
    let main = HWND(hwnd as *mut c_void);
    if hwnd == 0 || !is_own_window(main) {
        return Err(Error::Invalid(format!(
            "window {hwnd:#x} is not a window of the calling thread"
        )));
    }
    subclass(main)?;
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `windows`, which lives for the whole call and is not touched otherwise.
    let _ = unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect_window),
            LPARAM((&raw mut windows) as isize),
        )
    };
    let mut tao_target = false;
    for w in windows {
        if w != main && class_name(w) == TAO_TARGET_CLASS && subclass(w).is_ok() {
            tao_target = true;
        }
    }
    if !tao_target {
        tracing::warn!(
            "tao's event-target window was not found; session end is seen through the main window only"
        );
    }
    Ok(Installed {
        main_window: true,
        tao_target,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Task Scheduler
// ---------------------------------------------------------------------------------------------------------------

/// COM for the calling thread: initialised (multithreaded) unless the thread is already in another apartment, then
/// uninitialised on drop if this guard initialised it.
struct ComGuard {
    owns: bool,
}

impl ComGuard {
    fn init() -> Result<Self, Error> {
        // SAFETY: plain FFI call; a matching CoUninitialize runs on drop only when this call succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self { owns: false });
        }
        hr.ok().map_err(|e| Error::win32("CoInitializeEx", &e))?;
        Ok(Self { owns: true })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owns {
            // SAFETY: balances the successful CoInitializeEx of this guard on the same thread; every COM object
            // created under it was dropped before the guard (see `with_root`).
            unsafe { CoUninitialize() };
        }
    }
}

/// Runs `f` with the Task Scheduler's root folder.
fn with_root<T>(f: impl FnOnce(&ITaskFolder) -> Result<T, Error>) -> Result<T, Error> {
    let _com = ComGuard::init()?;
    {
        // SAFETY: COM is initialised on this thread; TaskScheduler is the documented CLSID of ITaskService.
        let service: ITaskService = unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| Error::win32("CoCreateInstance(TaskScheduler)", &e))?;
        let empty = VARIANT::default();
        // SAFETY: empty VARIANTs connect to the local machine as the current user.
        unsafe { service.Connect(&empty, &empty, &empty, &empty) }
            .map_err(|e| Error::win32("ITaskService::Connect", &e))?;
        // SAFETY: valid interface; the BSTR lives for the call.
        let root = unsafe { service.GetFolder(&BSTR::from("\\")) }
            .map_err(|e| Error::win32("ITaskService::GetFolder", &e))?;
        f(&root)
    }
}

/// Registers task `name` from `xml` with principal `user_sid` and logon type `logon_type`: `TASK_CREATE` (fails
/// when the name exists) or, with `update`, `TASK_CREATE_OR_UPDATE` (replaces it in place).
pub fn task_register(
    name: &str,
    xml: &str,
    user_sid: &str,
    logon_type: i32,
    update: bool,
) -> Result<(), Error> {
    let flags = if update {
        TASK_CREATE_OR_UPDATE.0
    } else {
        TASK_CREATE.0
    };
    with_root(|root| {
        let empty = VARIANT::default();
        // SAFETY: valid interface; every BSTR and VARIANT lives for the call; no password and no SDDL are passed.
        unsafe {
            root.RegisterTask(
                &BSTR::from(name),
                &BSTR::from(xml),
                flags,
                &VARIANT::from(user_sid),
                &empty,
                TASK_LOGON_TYPE(logon_type),
                &empty,
            )
        }
        .map(|_| ())
        .map_err(|e| Error::win32("ITaskFolder::RegisterTask", &e))
    })
}

/// Deletes task `name`.
pub fn task_delete(name: &str) -> Result<(), Error> {
    with_root(|root| {
        // SAFETY: valid interface; the BSTR lives for the call.
        unsafe { root.DeleteTask(&BSTR::from(name), 0) }
            .map_err(|e| Error::win32("ITaskFolder::DeleteTask", &e))
    })
}

/// Resolves a principal's user id to a SID: a SID string as it is, an account name through
/// `LookupAccountNameW` (unresolvable names are returned as they are, and then never equal a SID).
fn resolve_user(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    account_sid(text).unwrap_or_else(|_| text.to_owned())
}

/// Reads task `name`; `None` if it does not exist.
pub fn task_read(name: &str) -> Result<Option<TaskInfo>, Error> {
    with_root(|root| {
        // SAFETY: valid interface; the BSTR lives for the call.
        let task = match unsafe { root.GetTask(&BSTR::from(name)) } {
            Ok(task) => task,
            Err(e) if hresult_is(&e, ERROR_FILE_NOT_FOUND) || hresult_is(&e, ERROR_PATH_NOT_FOUND) => {
                return Ok(None);
            }
            Err(e) => return Err(Error::win32("ITaskFolder::GetTask", &e)),
        };
        let com = |call: &'static str| move |e: windows::core::Error| Error::win32(call, &e);
        // SAFETY: valid COM interfaces obtained above; out pointers are writable locals (the same holds for every
        // call below).
        let xml = unsafe { task.Xml() }
            .map_err(com("IRegisteredTask::Xml"))?
            .to_string();
        // SAFETY: see above.
        let task_enabled = unsafe { task.Enabled() }.map_err(com("IRegisteredTask::Enabled"))?;
        let mut enabled = task_enabled.as_bool();
        // SAFETY: see above.
        let definition = unsafe { task.Definition() }.map_err(com("IRegisteredTask::Definition"))?;
        // SAFETY: see above.
        let principal = unsafe { definition.Principal() }.map_err(com("ITaskDefinition::Principal"))?;
        // The principal as the XML states it (a SID for tasks registered with one); `IPrincipal::UserId` returns a
        // bare account name, which is ambiguous for domain and Microsoft Entra accounts, so it is only the fallback.
        let user = match crate::task::principal_from_xml(&xml) {
            Some(user) => user,
            None => {
                let mut user = BSTR::default();
                // SAFETY: see above.
                unsafe { principal.UserId(&mut user) }.map_err(com("IPrincipal::UserId"))?;
                user.to_string()
            }
        };
        let mut xml_logon_users = crate::task::logon_trigger_users_from_xml(&xml).into_iter();
        let mut logon = TASK_LOGON_TYPE::default();
        // SAFETY: see above.
        unsafe { principal.LogonType(&mut logon) }.map_err(com("IPrincipal::LogonType"))?;
        let mut level = TASK_RUNLEVEL_TYPE::default();
        // SAFETY: see above.
        unsafe { principal.RunLevel(&mut level) }.map_err(com("IPrincipal::RunLevel"))?;

        // SAFETY: see above.
        let triggers = unsafe { definition.Triggers() }.map_err(com("ITaskDefinition::Triggers"))?;
        let mut count = 0i32;
        // SAFETY: see above.
        unsafe { triggers.Count(&mut count) }.map_err(com("ITriggerCollection::Count"))?;
        let mut kinds = Vec::new();
        let mut trigger_users = Vec::new();
        for i in 1..=count {
            // SAFETY: see above; collections are 1-based and `i` is within the count.
            let trigger = unsafe { triggers.get_Item(i) }.map_err(com("ITriggerCollection::Item"))?;
            let mut kind = TASK_TRIGGER_TYPE2::default();
            // SAFETY: see above.
            unsafe { trigger.Type(&mut kind) }.map_err(com("ITrigger::Type"))?;
            let mut trigger_enabled = VARIANT_BOOL::default();
            // SAFETY: see above.
            unsafe { trigger.Enabled(&mut trigger_enabled) }.map_err(com("ITrigger::Enabled"))?;
            enabled &= trigger_enabled.as_bool();
            let mut trigger_user = None;
            if kind == TASK_TRIGGER_LOGON {
                // The k-th logon trigger of the collection is the k-th <LogonTrigger> of the XML.
                let id = match xml_logon_users.next() {
                    Some(from_xml) => from_xml.unwrap_or_default(),
                    None => {
                        let logon_trigger: ILogonTrigger = trigger.cast().map_err(com("ILogonTrigger"))?;
                        let mut id = BSTR::default();
                        // SAFETY: see above.
                        unsafe { logon_trigger.UserId(&mut id) }.map_err(com("ILogonTrigger::UserId"))?;
                        id.to_string()
                    }
                };
                // Kept as the Task Scheduler stores it (normally `DOMAIN\user`, never resolved to a SID here:
                // `LookupAccountNameW` fails for Microsoft Entra users and offline domains). `TaskSpec::verify_for`
                // compares it with the SID and the SID's account name.
                let id = id.trim().to_owned();
                trigger_user = (!id.is_empty()).then_some(id);
            }
            kinds.push(TriggerKind::from_raw(kind.0));
            trigger_users.push(trigger_user);
        }

        // SAFETY: see above.
        let actions = unsafe { definition.Actions() }.map_err(com("ITaskDefinition::Actions"))?;
        let mut count = 0i32;
        // SAFETY: see above.
        unsafe { actions.Count(&mut count) }.map_err(com("IActionCollection::Count"))?;
        let mut exec_actions = Vec::new();
        let mut other_actions = 0usize;
        for i in 1..=count {
            // SAFETY: see above; collections are 1-based and `i` is within the count.
            let action = unsafe { actions.get_Item(i) }.map_err(com("IActionCollection::Item"))?;
            let mut kind = TASK_ACTION_TYPE::default();
            // SAFETY: see above.
            unsafe { action.Type(&mut kind) }.map_err(com("IAction::Type"))?;
            if kind != TASK_ACTION_EXEC {
                other_actions += 1;
                continue;
            }
            let exec: IExecAction = action.cast().map_err(com("IExecAction"))?;
            let (mut path, mut arguments, mut dir) = (BSTR::default(), BSTR::default(), BSTR::default());
            // SAFETY: see above.
            unsafe { exec.Path(&mut path) }.map_err(com("IExecAction::Path"))?;
            // SAFETY: see above.
            unsafe { exec.Arguments(&mut arguments) }.map_err(com("IExecAction::Arguments"))?;
            // SAFETY: see above.
            unsafe { exec.WorkingDirectory(&mut dir) }.map_err(com("IExecAction::WorkingDirectory"))?;
            exec_actions.push(ExecAction {
                path: path.to_string(),
                arguments: arguments.to_string(),
                working_directory: dir.to_string(),
            });
        }

        Ok(Some(TaskInfo {
            user_sid: resolve_user(&user),
            logon_type: LogonType::from_raw(logon.0),
            run_level: RunLevel::from_raw(level.0),
            triggers: kinds,
            trigger_users,
            exec_actions,
            other_actions,
            enabled,
        }))
    })
}
