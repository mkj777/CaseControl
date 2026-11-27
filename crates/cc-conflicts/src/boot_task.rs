//! The elevated helper's start-up task actions (ADR 0001 #42): [`Action::RegisterBootTask`] and
//! [`Action::RemoveBootTask`](crate::Action::RemoveBootTask).
//!
//! Registering a task with a boot trigger needs administrator rights, so the app asks the elevated helper (one UAC
//! prompt). The request carries no values for these actions: the helper derives every one itself.
//!
//! - **Principal**: the user signed in to the helper's own Windows session ([`HelperContext::session_id`], which
//!   [`run_elevated_request_in`](crate::run_elevated_request_in) has already compared with the request's session).
//!   It must be a real user account (never SYSTEM or a service account), the helper's own token must belong to the
//!   same user (an administrator who approved the prompt with their own credentials for a standard user is refused:
//!   the task would run that administrator's account from a folder the standard user can write; the same refusal
//!   covers Windows Administrator protection, whose elevated token is a separate account:
//!   [`other_account_message`]), the request file must be owned by that user (one owned by Administrators means
//!   CaseControl itself runs elevated: [`ADMIN_OWNED_REQUEST_MESSAGE`]), and the user's `ProfileList` entry must
//!   exist with the request file inside that profile folder.
//! - A task under our name that is not ours is left alone with the text of [`cc_system::not_ours_message`].
//! - **Executable**: the helper's own `current_exe()`, which must pass [`cc_system::check_exe_path`] for that user.
//! - **Arguments, logon type, run level**: fixed by [`cc_system::TaskSpec::boot`] (`--apply-at-boot`, S4U,
//!   least privilege).
//!
//! The task is registered with [`cc_system::register_verified`] (read back and compared; deleted again on any
//! difference) and removed with [`cc_system::delete_ours`] (only after reading it back and confirming it is ours).
//!
//! [`Action::RegisterBootTask`]: crate::Action::RegisterBootTask

use std::path::{Path, PathBuf};

use cc_system::{Ownership, TaskKind, TaskSpec, TaskStore, is_user_sid};

/// The reserved, non-catalogue `entry_id` of the start-up task actions. The catalogue never uses it, and
/// [`check_action`](crate::check_action) accepts it for these two actions only.
pub const BOOT_TASK_ENTRY: &str = "casecontrol-boot-task";

/// What the elevated helper knows about its own invocation, independent of the request's content.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HelperContext {
    /// The helper's own Windows session (`None` when unknown).
    pub session_id: Option<u32>,
    /// The validated request file the helper was started with (`None` when unknown).
    pub request_file: Option<PathBuf>,
}

impl HelperContext {
    /// The context of the running helper process started for `request_file` (already validated with
    /// [`validate_helper_request`](crate::validate_helper_request)).
    pub fn for_request(request_file: &Path) -> Self {
        Self {
            session_id: crate::current_session_id(),
            request_file: Some(request_file.to_path_buf()),
        }
    }

    /// The context of the running process without a request file (start-up task actions are then refused).
    pub fn current() -> Self {
        Self {
            session_id: crate::current_session_id(),
            request_file: None,
        }
    }
}

/// The facts and operations the start-up task actions need from the system. `WindowsBootTaskEnv` is the real one;
/// tests use fakes.
pub trait BootTaskEnv {
    /// SID of the helper process token's user.
    fn token_user_sid(&self) -> Result<String, String>;
    /// SID of the user signed in to `session`.
    fn session_user_sid(&self, session: u32) -> Result<String, String>;
    /// Profile folder of the user with `sid` (from `ProfileList`).
    fn profile_path(&self, sid: &str) -> Result<PathBuf, String>;
    /// Owner SID of `path`.
    fn file_owner_sid(&self, path: &Path) -> Result<String, String>;
    /// The helper's own executable.
    fn own_exe(&self) -> Result<PathBuf, String>;
    /// The path and ACL check of `exe` for a task running as `principal_sid`.
    fn check_exe(&self, exe: &Path, principal_sid: &str) -> Result<(), String>;
    /// The Task Scheduler.
    fn store(&self) -> &dyn TaskStore;
}

/// `S-1-5-32-544`, BUILTIN\Administrators: the owner of files an elevated process of an administrator creates.
const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

/// The refusal when the request file is owned by Administrators (CaseControl itself runs elevated), meant to be
/// shown as it is. Worded for both directions (setting the task up and removing it).
pub const ADMIN_OWNED_REQUEST_MESSAGE: &str = "The start-up task was not changed because CaseControl is running as \
     administrator (its request belongs to the Administrators group, not to you). Start CaseControl normally, not \
     as administrator, and try again.";

/// The refusal when the permission prompt was approved with another account (`token`) than the signed-in user
/// (`sid`), meant to be shown as it is. Worded for both directions (setting the task up and removing it).
pub fn other_account_message(token: &str, sid: &str) -> String {
    format!(
        "The start-up task was not changed because the Windows permission prompt was approved with another account \
         ({token}) than yours ({sid}). This happens when an administrator's name and password are typed into the \
         prompt, or when Windows Administrator protection is on. The start-up task runs as you, so it can only be \
         set up or removed when the prompt is approved with your own account."
    )
}

/// Derives the start-up task's principal (a user SID) as described in the module docs, or says why not.
pub fn derive_principal(env: &dyn BootTaskEnv, ctx: &HelperContext) -> Result<String, String> {
    let session = ctx
        .session_id
        .ok_or("the helper's own Windows session is unknown")?;
    let sid = env
        .session_user_sid(session)
        .map_err(|e| format!("the user signed in to session {session} cannot be determined: {e}"))?;
    if !is_user_sid(&sid) {
        return Err(format!(
            "the user signed in to session {session} ({sid}) is not a user account"
        ));
    }
    let token = env
        .token_user_sid()
        .map_err(|e| format!("the helper's own account cannot be determined: {e}"))?;
    if !token.eq_ignore_ascii_case(&sid) {
        return Err(other_account_message(&token, &sid));
    }
    let request = ctx.request_file.as_deref().ok_or("the request file is unknown")?;
    let owner = env
        .file_owner_sid(request)
        .map_err(|e| format!("the owner of the request file cannot be read: {e}"))?;
    if owner.eq_ignore_ascii_case(ADMINISTRATORS_SID) {
        return Err(ADMIN_OWNED_REQUEST_MESSAGE.to_owned());
    }
    if !owner.eq_ignore_ascii_case(&sid) {
        return Err(format!(
            "the request file belongs to {owner}, not to the signed-in user {sid}"
        ));
    }
    let profile = env
        .profile_path(&sid)
        .map_err(|e| format!("the signed-in user {sid} has no profile: {e}"))?;
    if !path_is_within(request, &profile) {
        return Err(format!(
            "the request file is not inside the profile folder {} of {sid}",
            profile.display()
        ));
    }
    Ok(sid)
}

/// Whether `path` lies inside folder `dir` (component-wise, ASCII case-insensitive).
fn path_is_within(path: &Path, dir: &Path) -> bool {
    let lower = |p: &Path| -> Vec<String> {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect()
    };
    let (path, dir) = (lower(path), lower(dir));
    !dir.is_empty() && path.len() > dir.len() && path[..dir.len()] == dir[..]
}

/// Registers the start-up task for the derived principal (see the module docs).
pub fn register_boot_task(env: &dyn BootTaskEnv, ctx: &HelperContext) -> Result<(), String> {
    let sid = derive_principal(env, ctx)?;
    let exe = env.own_exe()?;
    env.check_exe(&exe, &sid)?;
    let spec = TaskSpec::boot(&sid, &exe).map_err(|e| e.to_string())?;
    cc_system::register_verified(env.store(), &spec).map_err(|e| e.to_string())
}

/// Removes the derived principal's start-up task if it is ours. Succeeds when there is none.
pub fn remove_boot_task(env: &dyn BootTaskEnv, ctx: &HelperContext) -> Result<(), String> {
    let sid = derive_principal(env, ctx)?;
    let exe = env.own_exe()?;
    let exe_file_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} names no file", exe.display()))?;
    let ownership = Ownership {
        kind: TaskKind::Boot,
        user_sid: sid,
        exe_file_name,
    };
    cc_system::delete_ours(env.store(), &ownership)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The real [`BootTaskEnv`]: the process token, WTS, `ProfileList`, file security, `current_exe()` and the Task
/// Scheduler through `cc-system`.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsBootTaskEnv {
    store: cc_system::WindowsTaskStore,
}

#[cfg(windows)]
impl BootTaskEnv for WindowsBootTaskEnv {
    fn token_user_sid(&self) -> Result<String, String> {
        cc_system::current_user_sid().map_err(|e| e.to_string())
    }
    fn session_user_sid(&self, session: u32) -> Result<String, String> {
        cc_system::session_user(session)
            .map(|a| a.sid)
            .map_err(|e| e.to_string())
    }
    fn profile_path(&self, sid: &str) -> Result<PathBuf, String> {
        cc_system::profile_path(sid).map_err(|e| e.to_string())
    }
    fn file_owner_sid(&self, path: &Path) -> Result<String, String> {
        cc_system::file_owner_sid(path).map_err(|e| e.to_string())
    }
    fn own_exe(&self) -> Result<PathBuf, String> {
        std::env::current_exe().map_err(|e| format!("the helper's own path cannot be read: {e}"))
    }
    fn check_exe(&self, exe: &Path, principal_sid: &str) -> Result<(), String> {
        cc_system::check_exe_path(exe, principal_sid).map_err(|e| e.to_string())
    }
    fn store(&self) -> &dyn TaskStore {
        &self.store
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cc_system::{MemoryTaskStore, RunLevel, TaskInfo, boot_task_name};

    pub(crate) const USER: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";
    const ADMIN: &str = "S-1-5-21-1111111111-2222222222-3333333333-500";

    #[cfg(windows)]
    const PROFILE: &str = r"C:\Users\u";
    #[cfg(not(windows))]
    const PROFILE: &str = "/home/u";

    pub(crate) fn exe() -> PathBuf {
        Path::new(PROFILE)
            .join("AppData")
            .join("Local")
            .join("CaseControl")
            .join("casecontrol-desktop.exe")
    }

    pub(crate) fn request() -> PathBuf {
        Path::new(PROFILE)
            .join("AppData")
            .join("Local")
            .join("CaseControl")
            .join("elevated")
            .join("request-1-2.json")
    }

    pub(crate) fn ctx() -> HelperContext {
        HelperContext {
            session_id: Some(1),
            request_file: Some(request()),
        }
    }

    /// A configurable fake system: everything agrees on [`USER`] unless a field says otherwise.
    pub(crate) struct FakeEnv {
        pub token: String,
        pub session_user: Result<String, String>,
        pub owner: String,
        pub profile: Result<PathBuf, String>,
        pub exe_ok: bool,
        pub store: MemoryTaskStore,
    }

    impl Default for FakeEnv {
        fn default() -> Self {
            Self {
                token: USER.into(),
                session_user: Ok(USER.into()),
                owner: USER.into(),
                profile: Ok(PathBuf::from(PROFILE)),
                exe_ok: true,
                store: MemoryTaskStore::new(),
            }
        }
    }

    impl BootTaskEnv for FakeEnv {
        fn token_user_sid(&self) -> Result<String, String> {
            Ok(self.token.clone())
        }
        fn session_user_sid(&self, session: u32) -> Result<String, String> {
            assert_eq!(session, 1, "the helper's own session is used");
            self.session_user.clone()
        }
        fn profile_path(&self, _sid: &str) -> Result<PathBuf, String> {
            self.profile.clone()
        }
        fn file_owner_sid(&self, _path: &Path) -> Result<String, String> {
            Ok(self.owner.clone())
        }
        fn own_exe(&self) -> Result<PathBuf, String> {
            Ok(exe())
        }
        fn check_exe(&self, _exe: &Path, _principal_sid: &str) -> Result<(), String> {
            if self.exe_ok {
                Ok(())
            } else {
                Err("other users may write to the install folder".into())
            }
        }
        fn store(&self) -> &dyn TaskStore {
            &self.store
        }
    }

    fn registered(env: &FakeEnv) -> Option<TaskInfo> {
        env.store.get(&boot_task_name(USER))
    }

    #[test]
    fn principal_is_the_signed_in_user() {
        assert_eq!(derive_principal(&FakeEnv::default(), &ctx()), Ok(USER.to_owned()));
    }

    #[test]
    fn principal_mismatches_are_refused() {
        let cases: Vec<(&str, FakeEnv, HelperContext)> = vec![
            (
                "admin credentials typed for a standard user",
                FakeEnv {
                    token: ADMIN.into(),
                    ..FakeEnv::default()
                },
                ctx(),
            ),
            (
                "SYSTEM in the session",
                FakeEnv {
                    session_user: Ok("S-1-5-18".into()),
                    token: "S-1-5-18".into(),
                    owner: "S-1-5-18".into(),
                    ..FakeEnv::default()
                },
                ctx(),
            ),
            (
                "nobody signed in",
                FakeEnv {
                    session_user: Err("no user".into()),
                    ..FakeEnv::default()
                },
                ctx(),
            ),
            (
                "request file of another user",
                FakeEnv {
                    owner: ADMIN.into(),
                    ..FakeEnv::default()
                },
                ctx(),
            ),
            (
                "no ProfileList entry",
                FakeEnv {
                    profile: Err("missing".into()),
                    ..FakeEnv::default()
                },
                ctx(),
            ),
            (
                "request file outside the profile",
                FakeEnv {
                    profile: Ok(Path::new(PROFILE).with_file_name("other")),
                    ..FakeEnv::default()
                },
                ctx(),
            ),
            (
                "unknown session",
                FakeEnv::default(),
                HelperContext {
                    session_id: None,
                    ..ctx()
                },
            ),
            (
                "unknown request file",
                FakeEnv::default(),
                HelperContext {
                    request_file: None,
                    ..ctx()
                },
            ),
        ];
        for (what, env, ctx) in cases {
            assert!(derive_principal(&env, &ctx).is_err(), "{what}");
            assert!(register_boot_task(&env, &ctx).is_err(), "{what}");
            assert!(remove_boot_task(&env, &ctx).is_err(), "{what}");
            assert!(env.store.calls().is_empty(), "{what}: {:?}", env.store.calls());
        }
    }

    #[test]
    fn profile_containment_is_component_wise() {
        let base = Path::new(PROFILE);
        assert!(path_is_within(&request(), base));
        assert!(!path_is_within(base, base));
        let sibling = PathBuf::from(format!("{PROFILE}2")).join("x.json");
        assert!(!path_is_within(&sibling, base));
    }

    #[test]
    fn register_creates_the_fixed_s4u_task_and_verifies_it() {
        let env = FakeEnv::default();
        register_boot_task(&env, &ctx()).expect("registered");
        let task = registered(&env).expect("task");
        let spec = TaskSpec::boot(USER, &exe()).expect("spec");
        assert_eq!(task, spec.expected());
        assert_eq!(task.logon_type, cc_system::LogonType::S4U);
        assert_eq!(task.run_level, RunLevel::LeastPrivilege);
        assert_eq!(task.exec_actions[0].arguments, "--apply-at-boot");
        assert_eq!(
            env.store.calls().last().map(String::as_str),
            Some(format!("read {}", boot_task_name(USER)).as_str())
        );
    }

    #[test]
    fn an_unsafe_executable_is_never_registered() {
        let env = FakeEnv {
            exe_ok: false,
            ..FakeEnv::default()
        };
        assert!(register_boot_task(&env, &ctx()).is_err());
        assert!(env.store.calls().is_empty());
    }

    #[test]
    fn verify_after_register_mismatch_deletes_the_task() {
        let env = FakeEnv {
            store: MemoryTaskStore::with_tamper(|t| t.run_level = RunLevel::Highest),
            ..FakeEnv::default()
        };
        let err = register_boot_task(&env, &ctx()).expect_err("mismatch");
        assert!(err.contains("run level"), "{err}");
        assert_eq!(registered(&env), None);
        assert!(
            env.store
                .calls()
                .contains(&format!("delete {}", boot_task_name(USER)))
        );
    }

    #[test]
    fn remove_deletes_only_our_task() {
        let env = FakeEnv::default();
        // Nothing there: success, nothing deleted.
        remove_boot_task(&env, &ctx()).expect("nothing to remove");
        register_boot_task(&env, &ctx()).expect("registered");
        remove_boot_task(&env, &ctx()).expect("removed");
        assert_eq!(registered(&env), None);

        // A task of that name that starts another program is left alone, with a clear text.
        let env = FakeEnv::default();
        let mut foreign = TaskSpec::boot(USER, &exe()).expect("spec").expected();
        foreign.exec_actions[0].path = "C:\\Tools\\other.exe".into();
        env.store.insert(&boot_task_name(USER), foreign.clone());
        let err = remove_boot_task(&env, &ctx()).expect_err("not ours");
        assert!(err.contains("not created by CaseControl"), "{err}");
        let err = register_boot_task(&env, &ctx()).expect_err("not ours");
        assert!(err.contains("not created by CaseControl"), "{err}");
        assert!(err.contains("Task Scheduler"), "{err}");
        assert_eq!(registered(&env), Some(foreign));
    }

    /// Our own existing start-up task is updated in place, never deleted first.
    #[test]
    fn re_registering_updates_in_place() {
        let env = FakeEnv::default();
        register_boot_task(&env, &ctx()).expect("registered");
        register_boot_task(&env, &ctx()).expect("re-registered");
        let calls = env.store.calls();
        assert!(!calls.iter().any(|c| c.starts_with("delete")), "{calls:?}");
        assert!(
            calls.contains(&format!("update {}", boot_task_name(USER))),
            "{calls:?}"
        );
    }

    #[test]
    fn refusals_the_app_shows_are_specific() {
        let env = FakeEnv {
            token: ADMIN.into(),
            ..FakeEnv::default()
        };
        let err = register_boot_task(&env, &ctx()).expect_err("other account");
        assert_eq!(err, other_account_message(ADMIN, USER));
        assert!(err.contains("Administrator protection"), "{err}");
        assert!(err.contains(ADMIN) && err.contains(USER), "{err}");
        assert_eq!(
            remove_boot_task(&env, &ctx()),
            Err(other_account_message(ADMIN, USER))
        );
        // The same sentence is shown for setting up and for removing, so it names neither as what did not happen.
        for text in [err.as_str(), ADMIN_OWNED_REQUEST_MESSAGE] {
            assert!(
                text.starts_with("The start-up task was not changed because"),
                "{text}"
            );
            assert!(!text.contains("not set up"), "{text}");
            assert!(!text.contains("not removed"), "{text}");
        }

        let env = FakeEnv {
            owner: ADMINISTRATORS_SID.into(),
            ..FakeEnv::default()
        };
        assert_eq!(
            register_boot_task(&env, &ctx()),
            Err(ADMIN_OWNED_REQUEST_MESSAGE.to_owned())
        );
        assert_eq!(
            remove_boot_task(&env, &ctx()),
            Err(ADMIN_OWNED_REQUEST_MESSAGE.to_owned())
        );
        assert!(env.store.calls().is_empty());
    }
}
