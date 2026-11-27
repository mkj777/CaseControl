//! CaseControl's two Task Scheduler tasks (ADR 0001 #42).
//!
//! - The **logon task** ([`TaskKind::Logon`]) replaces the HKCU Run entry. The user registers it without admin
//!   rights: a logon trigger for the user's SID with no delay, principal = that SID with `InteractiveToken` and
//!   `LeastPrivilege`, priority 4 (normal process class), runs on battery, no time limit, `IgnoreNew`; the action is
//!   the app's executable with [`LOGON_TASK_ARGUMENT`] in the executable's folder.
//! - The **start-up task** ([`TaskKind::Boot`]) runs CaseControl headless before sign-in. Only the elevated helper
//!   registers it (a boot trigger needs administrator rights): boot trigger, principal = the user's SID with logon
//!   type `S4U` and run level `LeastPrivilege` (never SYSTEM, a service account, highest privileges, a password or
//!   `InteractiveToken`), a 2 minute time limit, runs on battery, priority 4, `IgnoreNew`; the action is the
//!   executable with exactly [`BOOT_TASK_ARGUMENT`].
//!
//! Both live in the Task Scheduler's root folder `\`, which Windows owns (a user cannot create or take over a folder
//! there), under names that contain the user's SID ([`logon_task_name`], [`boot_task_name`]), so two users never
//! collide and removing one user's task can never hit another user's. An existing task of that name is read back
//! first and only replaced when it is ours ([`Ownership`]); it is then updated in place
//! ([`RegisterMode::CreateOrUpdate`], so there is no moment without a task), while a free name is registered with
//! "create" only ([`RegisterMode::Create`], which fails if someone took the name in between). After registering,
//! the task is read back and compared with the intent ([`TaskSpec::verify`]); on any difference it is deleted and
//! the call fails.
//!
//! The principal is read back from the task's XML ([`principal_from_xml`]), where the Task Scheduler keeps the SID it
//! was registered with. A logon trigger's user is stored as `DOMAIN\user` instead, never as a SID
//! ([`logon_trigger_users_from_xml`]); it is not turned into a SID (that lookup fails for Microsoft Entra users and
//! offline domains) but compared with the expected SID and with that SID's account name
//! ([`TaskStore::account_name`], [`TaskSpec::verify_for`]). A task that is ours but disabled (in the Task Scheduler
//! or Task Manager) is reported as [`TaskState::Disabled`].

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Component, Path, Prefix};

use crate::Error;
use crate::identity::is_user_sid;

/// The argument the logon task passes (start with the window hidden).
pub const LOGON_TASK_ARGUMENT: &str = "--hidden";

/// The only argument the start-up task passes: apply the active profile headless, then exit.
pub const BOOT_TASK_ARGUMENT: &str = "--apply-at-boot";

/// Description shown in the Task Scheduler for both tasks.
const DESCRIPTION_LOGON: &str = "Starts CaseControl when you sign in.";
const DESCRIPTION_BOOT: &str =
    "Applies the CaseControl lighting profile when Windows starts, before sign-in.";

/// Name of the logon task of the user with `sid` (in the root folder `\`).
pub fn logon_task_name(sid: &str) -> String {
    format!("CaseControl Logon {sid}")
}

/// Name of the start-up task of the user with `sid` (in the root folder `\`).
pub fn boot_task_name(sid: &str) -> String {
    format!("CaseControl Start-up {sid}")
}

/// Whether the command line (including the program name) is exactly `<program> --apply-at-boot`, the start-up
/// task's invocation. Any other command line, also one that contains the flag among others, is not.
pub fn is_apply_at_boot_invocation<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    matches!(args.as_slice(), [_, flag] if flag == BOOT_TASK_ARGUMENT)
}

/// Which of CaseControl's tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// Starts the app at sign-in (created by the user).
    Logon,
    /// Applies the profile at boot, before sign-in (created by the elevated helper).
    Boot,
}

impl TaskKind {
    /// The task's name for the user with `sid`.
    pub fn name(self, sid: &str) -> String {
        match self {
            TaskKind::Logon => logon_task_name(sid),
            TaskKind::Boot => boot_task_name(sid),
        }
    }

    /// The fixed argument string of the task's action.
    pub fn argument(self) -> &'static str {
        match self {
            TaskKind::Logon => LOGON_TASK_ARGUMENT,
            TaskKind::Boot => BOOT_TASK_ARGUMENT,
        }
    }

    /// The trigger the task has.
    pub fn trigger(self) -> TriggerKind {
        match self {
            TaskKind::Logon => TriggerKind::Logon,
            TaskKind::Boot => TriggerKind::Boot,
        }
    }

    /// The principal's logon type.
    pub fn logon_type(self) -> LogonType {
        match self {
            TaskKind::Logon => LogonType::InteractiveToken,
            TaskKind::Boot => LogonType::S4U,
        }
    }
}

/// `TASK_LOGON_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogonType {
    /// `TASK_LOGON_NONE` (0).
    None,
    /// `TASK_LOGON_PASSWORD` (1).
    Password,
    /// `TASK_LOGON_S4U` (2): runs whether the user is signed in or not, no stored password, no network access.
    S4U,
    /// `TASK_LOGON_INTERACTIVE_TOKEN` (3): only while the user is signed in, in their session.
    InteractiveToken,
    /// `TASK_LOGON_GROUP` (4).
    Group,
    /// `TASK_LOGON_SERVICE_ACCOUNT` (5).
    ServiceAccount,
    /// `TASK_LOGON_INTERACTIVE_TOKEN_OR_PASSWORD` (6).
    InteractiveTokenOrPassword,
    /// Any other value.
    Other(i32),
}

impl LogonType {
    /// From the raw `TASK_LOGON_TYPE` value.
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => LogonType::None,
            1 => LogonType::Password,
            2 => LogonType::S4U,
            3 => LogonType::InteractiveToken,
            4 => LogonType::Group,
            5 => LogonType::ServiceAccount,
            6 => LogonType::InteractiveTokenOrPassword,
            other => LogonType::Other(other),
        }
    }

    /// The raw `TASK_LOGON_TYPE` value.
    pub fn raw(self) -> i32 {
        match self {
            LogonType::None => 0,
            LogonType::Password => 1,
            LogonType::S4U => 2,
            LogonType::InteractiveToken => 3,
            LogonType::Group => 4,
            LogonType::ServiceAccount => 5,
            LogonType::InteractiveTokenOrPassword => 6,
            LogonType::Other(v) => v,
        }
    }

    fn xml(self) -> &'static str {
        match self {
            LogonType::S4U => "S4U",
            LogonType::InteractiveToken => "InteractiveToken",
            // Never produced by a TaskSpec.
            _ => "InteractiveToken",
        }
    }
}

/// `TASK_RUNLEVEL_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunLevel {
    /// `TASK_RUNLEVEL_LUA` (0): the limited (filtered) token.
    LeastPrivilege,
    /// `TASK_RUNLEVEL_HIGHEST` (1): the elevated token. CaseControl never asks for it.
    Highest,
    /// Any other value.
    Other(i32),
}

impl RunLevel {
    /// From the raw `TASK_RUNLEVEL_TYPE` value.
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => RunLevel::LeastPrivilege,
            1 => RunLevel::Highest,
            other => RunLevel::Other(other),
        }
    }
}

/// `TASK_TRIGGER_TYPE2`, as far as CaseControl cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    /// `TASK_TRIGGER_BOOT` (8).
    Boot,
    /// `TASK_TRIGGER_LOGON` (9).
    Logon,
    /// Any other trigger type.
    Other(i32),
}

impl TriggerKind {
    /// From the raw `TASK_TRIGGER_TYPE2` value.
    pub fn from_raw(v: i32) -> Self {
        match v {
            8 => TriggerKind::Boot,
            9 => TriggerKind::Logon,
            other => TriggerKind::Other(other),
        }
    }
}

/// One "start a program" action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecAction {
    /// The program.
    pub path: String,
    /// Its arguments.
    pub arguments: String,
    /// Its working directory.
    pub working_directory: String,
}

/// What a registered task looks like when read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInfo {
    /// The principal's SID (the store resolves an account name to its SID; unresolvable names stay as they are).
    pub user_sid: String,
    /// The principal's logon type.
    pub logon_type: LogonType,
    /// The principal's run level.
    pub run_level: RunLevel,
    /// Trigger types in order.
    pub triggers: Vec<TriggerKind>,
    /// The user a logon trigger is limited to, one entry per trigger (`None` for other triggers or "any user"), as
    /// the Task Scheduler stores it: normally `DOMAIN\user`, a SID only where one was stored. See
    /// [`TaskSpec::verify_for`].
    pub trigger_users: Vec<Option<String>>,
    /// The exec actions in order.
    pub exec_actions: Vec<ExecAction>,
    /// How many actions are not exec actions (COM handlers, e-mail, message boxes).
    pub other_actions: usize,
    /// Whether the task can run: the task itself and every one of its triggers are enabled. `false` when the user
    /// disabled it in the Task Scheduler or Task Manager.
    pub enabled: bool,
}

/// One of CaseControl's tasks for one user and executable. Built only through [`TaskSpec::logon`] and
/// [`TaskSpec::boot`], which refuse a non-user SID and an executable path that cannot be passed safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    kind: TaskKind,
    user_sid: String,
    exe: String,
    working_directory: String,
}

impl TaskSpec {
    /// The logon task for `user_sid` starting `exe`.
    pub fn logon(user_sid: &str, exe: &Path) -> Result<Self, Error> {
        Self::new(TaskKind::Logon, user_sid, exe)
    }

    /// The start-up task for `user_sid` starting `exe`.
    pub fn boot(user_sid: &str, exe: &Path) -> Result<Self, Error> {
        Self::new(TaskKind::Boot, user_sid, exe)
    }

    /// A task of `kind`.
    pub fn new(kind: TaskKind, user_sid: &str, exe: &Path) -> Result<Self, Error> {
        if !is_user_sid(user_sid) {
            return Err(Error::Invalid(format!(
                "{user_sid:?} is not the SID of a user account; tasks are only created for users"
            )));
        }
        let exe_text = exe
            .to_str()
            .ok_or_else(|| Error::Invalid(format!("{} is not valid Unicode", exe.display())))?;
        if !is_local_absolute(exe) {
            return Err(Error::Invalid(format!(
                "{exe_text} is not an absolute path on a local drive"
            )));
        }
        // `%` is refused because the Task Scheduler expands environment variables in the program path.
        if exe_text.chars().any(|c| c.is_control() || c == '"' || c == '%') {
            return Err(Error::Invalid(format!(
                "{exe_text:?} contains characters a task cannot carry"
            )));
        }
        let (Some(dir), Some(_)) = (exe.parent(), exe.file_name()) else {
            return Err(Error::Invalid(format!("{exe_text} names no file")));
        };
        let working_directory = dir
            .to_str()
            .ok_or_else(|| Error::Invalid(format!("{} is not valid Unicode", dir.display())))?
            .to_owned();
        Ok(Self {
            kind,
            user_sid: user_sid.to_owned(),
            exe: exe_text.to_owned(),
            working_directory,
        })
    }

    /// Which task.
    pub fn kind(&self) -> TaskKind {
        self.kind
    }

    /// The principal's SID.
    pub fn user_sid(&self) -> &str {
        &self.user_sid
    }

    /// The program the task starts.
    pub fn exe(&self) -> &str {
        &self.exe
    }

    /// The task's name in the root folder.
    pub fn name(&self) -> String {
        self.kind.name(&self.user_sid)
    }

    /// The principal's logon type.
    pub fn logon_type(&self) -> LogonType {
        self.kind.logon_type()
    }

    /// The run level: always [`RunLevel::LeastPrivilege`].
    pub fn run_level(&self) -> RunLevel {
        RunLevel::LeastPrivilege
    }

    /// The task as it should read back.
    pub fn expected(&self) -> TaskInfo {
        TaskInfo {
            user_sid: self.user_sid.clone(),
            logon_type: self.logon_type(),
            run_level: self.run_level(),
            triggers: vec![self.kind.trigger()],
            trigger_users: vec![match self.kind {
                TaskKind::Logon => Some(self.user_sid.clone()),
                TaskKind::Boot => None,
            }],
            exec_actions: vec![ExecAction {
                path: self.exe.clone(),
                arguments: self.kind.argument().to_owned(),
                working_directory: self.working_directory.clone(),
            }],
            other_actions: 0,
            enabled: true,
        }
    }

    /// Compares a read-back task with this intent. Returns every difference. A logon trigger's user only matches when
    /// it is the SID itself; use [`TaskSpec::verify_for`] with the SID's account name for a task read back from the
    /// Task Scheduler.
    pub fn verify(&self, actual: &TaskInfo) -> Result<(), Vec<String>> {
        self.verify_for(actual, None)
    }

    /// [`TaskSpec::verify`] where the logon trigger's user may also be `account_name` (the `DOMAIN\user` of this
    /// spec's SID, from [`TaskStore::account_name`]), compared case-insensitively. Either match is accepted.
    pub fn verify_for(&self, actual: &TaskInfo, account_name: Option<&str>) -> Result<(), Vec<String>> {
        let want = self.expected();
        let mut diffs = Vec::new();
        if !actual.user_sid.eq_ignore_ascii_case(&want.user_sid) {
            diffs.push(format!(
                "principal is {:?}, expected {:?}",
                actual.user_sid, want.user_sid
            ));
        }
        if actual.logon_type != want.logon_type {
            diffs.push(format!(
                "logon type is {:?}, expected {:?}",
                actual.logon_type, want.logon_type
            ));
        }
        if actual.run_level != want.run_level {
            diffs.push(format!(
                "run level is {:?}, expected {:?}",
                actual.run_level, want.run_level
            ));
        }
        if actual.triggers != want.triggers {
            diffs.push(format!(
                "triggers are {:?}, expected {:?}",
                actual.triggers, want.triggers
            ));
        }
        if self.kind == TaskKind::Logon {
            let same_user = matches!(actual.trigger_users.as_slice(),
                [Some(u)] if same_account(u, &self.user_sid, account_name));
            if !same_user {
                let expected = match account_name {
                    Some(name) => format!("{:?} ({name:?})", self.user_sid),
                    None => format!("{:?}", self.user_sid),
                };
                diffs.push(format!(
                    "logon trigger is for {:?}, expected {expected}",
                    actual.trigger_users
                ));
            }
        }
        if actual.other_actions != 0 {
            diffs.push(format!("{} unexpected non-exec actions", actual.other_actions));
        }
        if !actual.enabled {
            diffs.push("the task or its trigger is disabled".to_owned());
        }
        match actual.exec_actions.as_slice() {
            [a] => {
                let w = &want.exec_actions[0];
                if !a.path.eq_ignore_ascii_case(&w.path) {
                    diffs.push(format!("program is {:?}, expected {:?}", a.path, w.path));
                }
                if a.arguments != w.arguments {
                    diffs.push(format!(
                        "arguments are {:?}, expected {:?}",
                        a.arguments, w.arguments
                    ));
                }
                if !a.working_directory.eq_ignore_ascii_case(&w.working_directory) {
                    diffs.push(format!(
                        "working directory is {:?}, expected {:?}",
                        a.working_directory, w.working_directory
                    ));
                }
            }
            other => diffs.push(format!("{} exec actions, expected 1", other.len())),
        }
        if diffs.is_empty() { Ok(()) } else { Err(diffs) }
    }

    /// The Task Scheduler XML (schema 1.2) of this task. Every interpolated value is XML-escaped.
    pub fn to_xml(&self) -> String {
        let sid = xml_escape(&self.user_sid);
        let (description, trigger, settings_limit) = match self.kind {
            TaskKind::Logon => (
                DESCRIPTION_LOGON,
                format!(
                    "    <LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>{sid}</UserId>\n    </LogonTrigger>\n"
                ),
                "PT0S",
            ),
            TaskKind::Boot => (
                DESCRIPTION_BOOT,
                "    <BootTrigger>\n      <Enabled>true</Enabled>\n    </BootTrigger>\n".to_owned(),
                "PT2M",
            ),
        };
        format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n",
                "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n",
                "  <RegistrationInfo>\n",
                "    <Author>CaseControl</Author>\n",
                "    <Description>{description}</Description>\n",
                "  </RegistrationInfo>\n",
                "  <Triggers>\n",
                "{trigger}",
                "  </Triggers>\n",
                "  <Principals>\n",
                "    <Principal id=\"Author\">\n",
                "      <UserId>{sid}</UserId>\n",
                "      <LogonType>{logon_type}</LogonType>\n",
                "      <RunLevel>LeastPrivilege</RunLevel>\n",
                "    </Principal>\n",
                "  </Principals>\n",
                "  <Settings>\n",
                "    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\n",
                "    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\n",
                "    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\n",
                "    <AllowHardTerminate>true</AllowHardTerminate>\n",
                "    <StartWhenAvailable>false</StartWhenAvailable>\n",
                "    <AllowStartOnDemand>true</AllowStartOnDemand>\n",
                "    <Enabled>true</Enabled>\n",
                "    <Hidden>false</Hidden>\n",
                "    <ExecutionTimeLimit>{limit}</ExecutionTimeLimit>\n",
                "    <Priority>4</Priority>\n",
                "  </Settings>\n",
                "  <Actions Context=\"Author\">\n",
                "    <Exec>\n",
                "      <Command>{command}</Command>\n",
                "      <Arguments>{arguments}</Arguments>\n",
                "      <WorkingDirectory>{dir}</WorkingDirectory>\n",
                "    </Exec>\n",
                "  </Actions>\n",
                "</Task>\n",
            ),
            description = xml_escape(description),
            trigger = trigger,
            sid = sid,
            logon_type = self.logon_type().xml(),
            limit = settings_limit,
            command = xml_escape(&self.exe),
            arguments = xml_escape(self.kind.argument()),
            dir = xml_escape(&self.working_directory),
        )
    }
}

/// Whether a logon trigger's stored user `stored` names the account with `sid`: the SID itself, or the SID's
/// `DOMAIN\user` name `account_name`. Both compare case-insensitively (account names in Windows are; `to_lowercase`
/// also covers non-ASCII names).
fn same_account(stored: &str, sid: &str, account_name: Option<&str>) -> bool {
    let stored = stored.trim();
    stored.eq_ignore_ascii_case(sid)
        || account_name.is_some_and(|name| {
            let name = name.trim();
            !name.is_empty() && stored.to_lowercase() == name.to_lowercase()
        })
}

/// `C:\...` (a drive-letter path); UNC and device paths are refused.
fn is_local_absolute(path: &Path) -> bool {
    #[cfg(windows)]
    {
        path.is_absolute()
            && matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    }
    #[cfg(not(windows))]
    {
        let _ = (Component::RootDir, Prefix::Disk(b'C'));
        path.is_absolute()
    }
}

/// Escapes `&`, `<`, `>`, `"` and `'` for XML text and attribute values.
pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The text of the first `<tag>` element (written without attributes) inside `xml`. Good enough for the Task
/// Scheduler's own output, which never nests an element in itself.
fn xml_element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

/// The bodies of the `<tag ...>...</tag>` elements (with or without attributes) in `xml`, in order. A self-closing
/// `<tag/>` yields an empty body.
fn xml_blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        // `<LogonTriggerX` or `<Principals` is another element.
        let Some(next) = after.chars().next() else { break };
        if !(next == '>' || next == '/' || next.is_whitespace()) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        if after[..gt].ends_with('/') {
            out.push("");
            rest = &after[gt + 1..];
            continue;
        }
        let body = &after[gt + 1..];
        let Some(end) = body.find(&close) else { break };
        out.push(&body[..end]);
        rest = &body[end + close.len()..];
    }
    out
}

/// Decodes the five predefined XML entities and numeric character references; anything else stays as it is.
fn xml_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => {
                u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
            }
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

/// The principal's `<UserId>` in a task's XML (`IRegisteredTask::Xml`): the first `<Principal>` of `<Principals>`,
/// trimmed and decoded. The Task Scheduler writes the SID there for tasks registered with one; other tasks may carry
/// an account name. `None` when the principal has no user (a group principal).
pub fn principal_from_xml(xml: &str) -> Option<String> {
    let principals = xml_element(xml, "Principals")?;
    let principal = xml_blocks(principals, "Principal").into_iter().next()?;
    let user = xml_unescape(xml_element(principal, "UserId")?.trim());
    (!user.is_empty()).then_some(user)
}

/// The `<UserId>` of every `<LogonTrigger>` in a task's XML, in order (`None` for a logon trigger of any user).
pub fn logon_trigger_users_from_xml(xml: &str) -> Vec<Option<String>> {
    let Some(triggers) = xml_element(xml, "Triggers") else {
        return Vec::new();
    };
    xml_blocks(triggers, "LogonTrigger")
        .into_iter()
        .map(|t| {
            xml_element(t, "UserId")
                .map(|u| xml_unescape(u.trim()))
                .filter(|u| !u.is_empty())
        })
        .collect()
}

/// How CaseControl recognises its own task before it replaces or deletes one: the principal is the user, there is
/// exactly one action, an exec action whose program file name is CaseControl's executable name and whose arguments
/// are the task kind's fixed argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ownership {
    /// Which task.
    pub kind: TaskKind,
    /// The user's SID.
    pub user_sid: String,
    /// CaseControl's executable file name (for example `casecontrol-desktop.exe`); any folder matches, so a task
    /// left behind by an install in another folder is still recognised.
    pub exe_file_name: String,
}

impl Ownership {
    /// The ownership test for tasks like `spec`.
    pub fn of(spec: &TaskSpec) -> Self {
        Self {
            kind: spec.kind,
            user_sid: spec.user_sid.clone(),
            exe_file_name: file_name(&spec.exe).to_owned(),
        }
    }

    /// The task's name.
    pub fn name(&self) -> String {
        self.kind.name(&self.user_sid)
    }

    /// `Ok` when `info` is a task CaseControl created for this user; the reason otherwise.
    pub fn check(&self, info: &TaskInfo) -> Result<(), String> {
        if !info.user_sid.eq_ignore_ascii_case(&self.user_sid) {
            return Err(format!(
                "it runs as {:?}, not as {:?}",
                info.user_sid, self.user_sid
            ));
        }
        if info.other_actions != 0 {
            return Err("it has actions CaseControl never creates".to_owned());
        }
        let [action] = info.exec_actions.as_slice() else {
            return Err(format!(
                "it has {} programs, CaseControl creates 1",
                info.exec_actions.len()
            ));
        };
        if self.exe_file_name.is_empty() || !file_name(&action.path).eq_ignore_ascii_case(&self.exe_file_name)
        {
            return Err(format!("it starts {:?}, not {}", action.path, self.exe_file_name));
        }
        if action.arguments != self.kind.argument() {
            return Err(format!("its arguments are {:?}", action.arguments));
        }
        Ok(())
    }
}

/// The file-name component of a Windows path (also with a surrounding pair of quotes).
fn file_name(path: &str) -> &str {
    let path = path.trim().trim_matches('"');
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// How [`TaskStore::register`] treats an existing task of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterMode {
    /// `TASK_CREATE`: fails if a task of that name exists.
    Create,
    /// `TASK_CREATE_OR_UPDATE`: replaces the task of that name in place. Only used after the existing task was read
    /// back and confirmed to be ours.
    CreateOrUpdate,
}

/// The Task Scheduler as CaseControl uses it: the root folder only, registration, read back, delete.
/// `WindowsTaskStore` is the real one; [`MemoryTaskStore`] the fake.
pub trait TaskStore {
    /// Registers `spec` (its XML, with its principal and logon type). With [`RegisterMode::Create`] it fails if a
    /// task of that name exists; with [`RegisterMode::CreateOrUpdate`] it replaces that task in place.
    fn register(&self, spec: &TaskSpec, mode: RegisterMode) -> Result<(), Error>;
    /// Reads task `name`; `None` if there is none.
    fn read(&self, name: &str) -> Result<Option<TaskInfo>, Error>;
    /// Deletes task `name`.
    fn delete(&self, name: &str) -> Result<(), Error>;
    /// The `DOMAIN\user` account name of `sid`, as a logon trigger's user is stored; `None` when it cannot be
    /// resolved (the trigger then only matches when it stores the SID itself).
    fn account_name(&self, _sid: &str) -> Option<String> {
        None
    }
}

/// [`TaskSpec::verify_for`] with the account name `store` gives for `spec`'s SID (looked up only for the logon
/// task, the one with a per-user trigger).
fn verify_in(store: &dyn TaskStore, spec: &TaskSpec, info: &TaskInfo) -> Result<(), Vec<String>> {
    let name = match spec.kind() {
        TaskKind::Logon => store.account_name(spec.user_sid()),
        TaskKind::Boot => None,
    };
    spec.verify_for(info, name.as_deref())
}

/// The state of a task as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// There is no task of that name.
    Missing,
    /// Ours and exactly as `spec` describes.
    Matches,
    /// Ours but different (for example a stale executable path after a reinstall elsewhere): the differences.
    Differs(Vec<String>),
    /// Ours but disabled (the task or its trigger), for example switched off in Task Manager: it does not run.
    /// Takes precedence over [`TaskState::Differs`].
    Disabled,
    /// A task of that name exists that is not ours: why.
    Foreign(String),
}

/// What task `spec` looks like in `store`.
pub fn task_state(store: &dyn TaskStore, spec: &TaskSpec) -> Result<TaskState, Error> {
    let Some(info) = store.read(&spec.name())? else {
        return Ok(TaskState::Missing);
    };
    if let Err(why) = Ownership::of(spec).check(&info) {
        return Ok(TaskState::Foreign(why));
    }
    if !info.enabled {
        return Ok(TaskState::Disabled);
    }
    Ok(match verify_in(store, spec, &info) {
        Ok(()) => TaskState::Matches,
        Err(diffs) => TaskState::Differs(diffs),
    })
}

/// The text of [`Error::NotOurs`] for task `name` (`why` from [`Ownership::check`]), meant to be shown as it is.
pub fn not_ours_message(name: &str, why: &str) -> String {
    format!(
        "A scheduled task named \"{name}\" already exists and was not created by CaseControl ({why}). CaseControl \
         left it unchanged. Remove or rename that task in the Task Scheduler, then try again."
    )
}

/// Registers `spec`, then reads it back and verifies it. An existing task of the same name is replaced in place
/// ([`RegisterMode::CreateOrUpdate`], no gap without a task) only when it is ours; a free name is registered with
/// [`RegisterMode::Create`]. On any difference after registering the task is deleted and [`Error::Mismatch`]
/// returned; a task of that name that is not ours is left alone ([`Error::NotOurs`]).
pub fn register_verified(store: &dyn TaskStore, spec: &TaskSpec) -> Result<(), Error> {
    let name = spec.name();
    let mode = match store.read(&name)? {
        Some(existing) => {
            Ownership::of(spec)
                .check(&existing)
                .map_err(|why| Error::NotOurs(not_ours_message(&name, &why)))?;
            tracing::info!(task = %name, "updating CaseControl's existing task in place");
            RegisterMode::CreateOrUpdate
        }
        None => RegisterMode::Create,
    };
    store.register(spec, mode)?;
    let diffs = match store.read(&name) {
        Ok(Some(info)) => match verify_in(store, spec, &info) {
            Ok(()) => {
                tracing::info!(task = %name, "task registered and verified");
                return Ok(());
            }
            Err(diffs) => diffs,
        },
        Ok(None) => vec!["the task is missing after registering".to_owned()],
        Err(e) => vec![format!("the task cannot be read back: {e}")],
    };
    tracing::warn!(task = %name, ?diffs, "registered task differs from the intent; deleting it");
    if let Err(e) = store.delete(&name) {
        tracing::warn!(task = %name, error = %e, "deleting the mismatching task failed");
    }
    Err(Error::Mismatch(diffs))
}

/// Deletes the task `ownership` names after reading it back and confirming it is ours. Returns whether a task was
/// deleted (`false` when there was none); a task of that name that is not ours is left alone ([`Error::NotOurs`]).
pub fn delete_ours(store: &dyn TaskStore, ownership: &Ownership) -> Result<bool, Error> {
    let name = ownership.name();
    let Some(info) = store.read(&name)? else {
        return Ok(false);
    };
    ownership
        .check(&info)
        .map_err(|why| Error::NotOurs(not_ours_message(&name, &why)))?;
    store.delete(&name)?;
    tracing::info!(task = %name, "task deleted");
    Ok(true)
}

/// An in-memory [`TaskStore`] for tests. Records every call; [`MemoryTaskStore::with_tamper`] changes what a
/// registration stores (to simulate a Task Scheduler that stored something else).
#[derive(Default)]
pub struct MemoryTaskStore {
    tasks: RefCell<BTreeMap<String, TaskInfo>>,
    calls: RefCell<Vec<String>>,
    tamper: Option<fn(&mut TaskInfo)>,
    fail_register: Option<String>,
    account_names: BTreeMap<String, String>,
}

impl std::fmt::Debug for MemoryTaskStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryTaskStore")
            .field("tasks", &self.tasks.borrow())
            .field("calls", &self.calls.borrow())
            .finish_non_exhaustive()
    }
}

impl MemoryTaskStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store whose registrations store `info` changed by `tamper`.
    pub fn with_tamper(tamper: fn(&mut TaskInfo)) -> Self {
        Self {
            tamper: Some(tamper),
            ..Self::default()
        }
    }

    /// A store whose registrations fail with `message`.
    pub fn failing_register(message: &str) -> Self {
        Self {
            fail_register: Some(message.to_owned()),
            ..Self::default()
        }
    }

    /// Makes [`TaskStore::account_name`] answer `name` for `sid`.
    pub fn with_account_name(mut self, sid: &str, name: &str) -> Self {
        self.account_names.insert(sid.to_owned(), name.to_owned());
        self
    }

    /// Puts a task in place directly (an existing task, possibly someone else's).
    pub fn insert(&self, name: &str, info: TaskInfo) {
        self.tasks.borrow_mut().insert(name.to_owned(), info);
    }

    /// The stored task `name`.
    pub fn get(&self, name: &str) -> Option<TaskInfo> {
        self.tasks.borrow().get(name).cloned()
    }

    /// Every call so far: `register <name>` (create), `update <name>` (create or update), `read <name>`,
    /// `delete <name>`.
    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl TaskStore for MemoryTaskStore {
    fn register(&self, spec: &TaskSpec, mode: RegisterMode) -> Result<(), Error> {
        let name = spec.name();
        let verb = match mode {
            RegisterMode::Create => "register",
            RegisterMode::CreateOrUpdate => "update",
        };
        self.calls.borrow_mut().push(format!("{verb} {name}"));
        if let Some(message) = &self.fail_register {
            return Err(Error::Invalid(message.clone()));
        }
        let mut tasks = self.tasks.borrow_mut();
        if mode == RegisterMode::Create && tasks.contains_key(&name) {
            return Err(Error::Invalid(format!("task {name:?} already exists")));
        }
        let mut info = spec.expected();
        if let Some(tamper) = self.tamper {
            tamper(&mut info);
        }
        tasks.insert(name, info);
        Ok(())
    }

    fn read(&self, name: &str) -> Result<Option<TaskInfo>, Error> {
        self.calls.borrow_mut().push(format!("read {name}"));
        Ok(self.tasks.borrow().get(name).cloned())
    }

    fn delete(&self, name: &str) -> Result<(), Error> {
        self.calls.borrow_mut().push(format!("delete {name}"));
        self.tasks
            .borrow_mut()
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| Error::Invalid(format!("task {name:?} does not exist")))
    }

    fn account_name(&self, sid: &str) -> Option<String> {
        self.account_names.get(sid).cloned()
    }
}

/// The real Task Scheduler (COM `ITaskService`, root folder `\`). Each call connects anew; COM is initialised for
/// the calling thread when needed (multithreaded; a thread already in another apartment is used as it is).
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsTaskStore;

#[cfg(windows)]
impl TaskStore for WindowsTaskStore {
    fn register(&self, spec: &TaskSpec, mode: RegisterMode) -> Result<(), Error> {
        tracing::warn!(task = %spec.name(), kind = ?spec.kind(), exe = spec.exe(), ?mode, "registering a scheduled task");
        crate::sys::task_register(
            &spec.name(),
            &spec.to_xml(),
            spec.user_sid(),
            spec.logon_type().raw(),
            mode == RegisterMode::CreateOrUpdate,
        )
    }

    fn read(&self, name: &str) -> Result<Option<TaskInfo>, Error> {
        crate::sys::task_read(name)
    }

    fn delete(&self, name: &str) -> Result<(), Error> {
        tracing::warn!(task = %name, "deleting a scheduled task");
        crate::sys::task_delete(name)
    }

    fn account_name(&self, sid: &str) -> Option<String> {
        crate::sys::account_name(sid)
            .inspect_err(|e| tracing::debug!(sid, error = %e, "the SID's account name cannot be resolved"))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";
    #[cfg(windows)]
    const EXE: &str = r"C:\Users\u\AppData\Local\CaseControl\casecontrol-desktop.exe";
    #[cfg(not(windows))]
    const EXE: &str = "/home/u/CaseControl/casecontrol-desktop.exe";

    fn logon() -> TaskSpec {
        TaskSpec::logon(SID, Path::new(EXE)).expect("logon spec")
    }

    fn boot() -> TaskSpec {
        TaskSpec::boot(SID, Path::new(EXE)).expect("boot spec")
    }

    /// The text between `<tag>` and `</tag>` (first occurrence).
    fn element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let start = xml.find(&open)? + open.len();
        let end = xml[start..].find(&close)? + start;
        Some(&xml[start..end])
    }

    #[test]
    fn names_carry_the_sid() {
        assert_eq!(logon().name(), format!("CaseControl Logon {SID}"));
        assert_eq!(boot().name(), format!("CaseControl Start-up {SID}"));
        assert_ne!(
            logon_task_name("S-1-5-21-1-2-3-1001"),
            logon_task_name("S-1-5-21-1-2-3-1002")
        );
    }

    #[test]
    fn logon_task_xml() {
        let xml = logon().to_xml();
        assert!(xml.contains("<LogonTrigger>"));
        assert!(!xml.contains("<BootTrigger>"));
        assert!(!xml.contains("<Delay>"));
        assert_eq!(
            element(&xml, "LogonTrigger").and_then(|t| element(t, "UserId")),
            Some(SID)
        );
        let principal = element(&xml, "Principals").expect("principals");
        assert_eq!(element(principal, "UserId"), Some(SID));
        assert_eq!(element(principal, "LogonType"), Some("InteractiveToken"));
        assert_eq!(element(principal, "RunLevel"), Some("LeastPrivilege"));
        assert_eq!(element(&xml, "Priority"), Some("4"));
        assert_eq!(element(&xml, "DisallowStartIfOnBatteries"), Some("false"));
        assert_eq!(element(&xml, "StopIfGoingOnBatteries"), Some("false"));
        assert_eq!(element(&xml, "ExecutionTimeLimit"), Some("PT0S"));
        assert_eq!(element(&xml, "MultipleInstancesPolicy"), Some("IgnoreNew"));
        assert_eq!(element(&xml, "Command"), Some(EXE));
        assert_eq!(element(&xml, "Arguments"), Some("--hidden"));
        assert_eq!(
            element(&xml, "WorkingDirectory"),
            Path::new(EXE).parent().and_then(Path::to_str)
        );
        assert_eq!(xml.matches("<Exec>").count(), 1);
    }

    #[test]
    fn boot_task_xml_is_s4u_least_privilege_and_never_system_or_highest() {
        let xml = boot().to_xml();
        assert!(xml.contains("<BootTrigger>"));
        assert!(!xml.contains("<LogonTrigger>"));
        let principal = element(&xml, "Principals").expect("principals");
        assert_eq!(element(principal, "UserId"), Some(SID));
        assert_eq!(element(principal, "LogonType"), Some("S4U"));
        assert_eq!(element(principal, "RunLevel"), Some("LeastPrivilege"));
        for forbidden in [
            "HighestAvailable",
            "S-1-5-18",
            "SYSTEM",
            "LocalSystem",
            "Password",
            "InteractiveToken",
            "ServiceAccount",
            "GroupId",
        ] {
            assert!(!xml.contains(forbidden), "{forbidden}");
        }
        assert_eq!(element(&xml, "ExecutionTimeLimit"), Some("PT2M"));
        assert_eq!(element(&xml, "Priority"), Some("4"));
        assert_eq!(element(&xml, "DisallowStartIfOnBatteries"), Some("false"));
        assert_eq!(element(&xml, "StopIfGoingOnBatteries"), Some("false"));
        assert_eq!(element(&xml, "MultipleInstancesPolicy"), Some("IgnoreNew"));
        assert_eq!(element(&xml, "Arguments"), Some("--apply-at-boot"));
        assert_eq!(element(&xml, "Command"), Some(EXE));
    }

    #[test]
    fn interpolated_values_are_escaped() {
        assert_eq!(xml_escape(r#"a&b<c>d"e'f"#), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
        #[cfg(windows)]
        {
            let exe = r"C:\R&D <x>\it's\casecontrol-desktop.exe";
            let xml = TaskSpec::logon(SID, Path::new(exe)).expect("spec").to_xml();
            assert_eq!(
                element(&xml, "Command"),
                Some(r"C:\R&amp;D &lt;x&gt;\it&apos;s\casecontrol-desktop.exe")
            );
            assert!(!xml.contains("<x>"));
        }
    }

    #[test]
    fn specs_refuse_non_user_principals_and_unsafe_paths() {
        for sid in [
            "S-1-5-18",
            "S-1-5-19",
            "S-1-5-20",
            "S-1-5-32-544",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
            "S-1-1-0",
            "SYSTEM",
            "",
            "S-1-5-21-1-2-3-1001</UserId>",
        ] {
            assert!(TaskSpec::boot(sid, Path::new(EXE)).is_err(), "{sid}");
            assert!(TaskSpec::logon(sid, Path::new(EXE)).is_err(), "{sid}");
        }
        assert!(TaskSpec::boot("S-1-12-1-1111-2222-3333-4444", Path::new(EXE)).is_ok());
        assert!(TaskSpec::boot(SID, Path::new("casecontrol-desktop.exe")).is_err());
        #[cfg(windows)]
        {
            assert!(TaskSpec::boot(SID, Path::new(r"\\server\share\casecontrol-desktop.exe")).is_err());
            assert!(TaskSpec::boot(SID, Path::new("C:\\a\"b\\c.exe")).is_err());
            assert!(TaskSpec::boot(SID, Path::new("C:\\a\nb\\c.exe")).is_err());
        }
    }

    #[test]
    fn verify_reports_every_difference() {
        let spec = boot();
        assert_eq!(spec.verify(&spec.expected()), Ok(()));
        let mut info = spec.expected();
        info.run_level = RunLevel::Highest;
        info.logon_type = LogonType::ServiceAccount;
        info.user_sid = "S-1-5-18".into();
        info.triggers = vec![TriggerKind::Logon];
        info.exec_actions[0].arguments = "--apply-at-boot --x".into();
        let diffs = spec.verify(&info).expect_err("differs");
        assert_eq!(diffs.len(), 5, "{diffs:?}");
        // Path case does not matter on Windows.
        let mut info = spec.expected();
        info.exec_actions[0].path = info.exec_actions[0].path.to_uppercase();
        info.exec_actions[0].working_directory = info.exec_actions[0].working_directory.to_uppercase();
        assert_eq!(spec.verify(&info), Ok(()));
        // A logon trigger for another user.
        let spec = logon();
        let mut info = spec.expected();
        info.trigger_users = vec![None];
        assert!(spec.verify(&info).is_err());
    }

    #[test]
    fn register_verified_registers_and_reads_back() {
        let store = MemoryTaskStore::new();
        register_verified(&store, &boot()).expect("registered");
        assert_eq!(store.get(&boot().name()), Some(boot().expected()));
        let name = boot().name();
        assert_eq!(
            store.calls(),
            [
                format!("read {name}"),
                format!("register {name}"),
                format!("read {name}")
            ]
        );
        assert_eq!(task_state(&store, &boot()), Ok(TaskState::Matches));
        assert_eq!(task_state(&store, &logon()), Ok(TaskState::Missing));
    }

    #[test]
    fn mismatch_after_register_deletes_the_task() {
        let store = MemoryTaskStore::with_tamper(|info| info.run_level = RunLevel::Highest);
        let err = register_verified(&store, &boot()).expect_err("mismatch");
        assert!(matches!(err, Error::Mismatch(ref d) if d.len() == 1), "{err:?}");
        assert_eq!(store.get(&boot().name()), None);
        assert_eq!(store.calls().last(), Some(&format!("delete {}", boot().name())));
    }

    #[test]
    fn an_existing_task_is_replaced_only_when_ours() {
        // Ours but stale (another install folder): replaced.
        let store = MemoryTaskStore::new();
        let mut stale = boot().expected();
        stale.exec_actions[0].path = r"D:\Old\casecontrol-desktop.exe".into();
        store.insert(&boot().name(), stale);
        assert!(matches!(task_state(&store, &boot()), Ok(TaskState::Differs(_))));
        register_verified(&store, &boot()).expect("replaced");
        assert_eq!(store.get(&boot().name()), Some(boot().expected()));

        // Someone else's program under our name: left alone.
        let store = MemoryTaskStore::new();
        let mut foreign = boot().expected();
        foreign.exec_actions[0].path = r"C:\Tools\evil.exe".into();
        store.insert(&boot().name(), foreign.clone());
        assert!(matches!(
            register_verified(&store, &boot()),
            Err(Error::NotOurs(_))
        ));
        assert_eq!(store.get(&boot().name()), Some(foreign));
        assert!(
            !store
                .calls()
                .iter()
                .any(|c| c.starts_with("register") || c.starts_with("delete"))
        );
        assert!(matches!(task_state(&store, &boot()), Ok(TaskState::Foreign(_))));
    }

    /// Replacing our own task never deletes it first: it is updated in place, so there is no moment without one.
    #[test]
    fn replacing_our_task_updates_in_place_without_a_gap() {
        let store = MemoryTaskStore::new();
        let mut stale = logon().expected();
        stale.exec_actions[0].path = r"D:\Old\casecontrol-desktop.exe".into();
        store.insert(&logon().name(), stale);
        register_verified(&store, &logon()).expect("updated");
        let name = logon().name();
        assert_eq!(
            store.calls(),
            [
                format!("read {name}"),
                format!("update {name}"),
                format!("read {name}")
            ]
        );
        assert_eq!(store.get(&name), Some(logon().expected()));

        // A free name is registered with "create" only.
        let store = MemoryTaskStore::new();
        register_verified(&store, &logon()).expect("created");
        assert!(store.calls().contains(&format!("register {name}")));
        assert!(!store.calls().iter().any(|c| c.starts_with("update")));

        // Verify failure after the update deletes the task.
        let store = MemoryTaskStore::with_tamper(|i| i.run_level = RunLevel::Highest);
        store.insert(&name, logon().expected());
        assert!(matches!(
            register_verified(&store, &logon()),
            Err(Error::Mismatch(_))
        ));
        assert_eq!(store.get(&name), None);
    }

    #[test]
    fn not_ours_has_a_clear_message() {
        let store = MemoryTaskStore::new();
        let mut foreign = boot().expected();
        foreign.exec_actions[0].path = r"C:\Tools\evil.exe".into();
        store.insert(&boot().name(), foreign);
        let err = register_verified(&store, &boot()).expect_err("not ours");
        let text = err.to_string();
        assert!(
            text.contains("already exists and was not created by CaseControl"),
            "{text}"
        );
        assert!(text.contains(&boot().name()), "{text}");
        assert!(text.contains("Task Scheduler"), "{text}");
        let err = delete_ours(&store, &Ownership::of(&boot())).expect_err("not ours");
        assert!(matches!(err, Error::NotOurs(ref t) if t.contains("left it unchanged")));
    }

    #[test]
    fn a_disabled_task_is_its_own_state() {
        let store = MemoryTaskStore::new();
        let mut disabled = logon().expected();
        disabled.enabled = false;
        store.insert(&logon().name(), disabled.clone());
        assert_eq!(task_state(&store, &logon()), Ok(TaskState::Disabled));
        // Disabled wins over differences; a foreign task stays foreign.
        disabled.exec_actions[0].path = r"D:\Old\casecontrol-desktop.exe".into();
        store.insert(&logon().name(), disabled.clone());
        assert_eq!(task_state(&store, &logon()), Ok(TaskState::Disabled));
        disabled.exec_actions[0].path = r"C:\Tools\evil.exe".into();
        store.insert(&logon().name(), disabled);
        assert!(matches!(task_state(&store, &logon()), Ok(TaskState::Foreign(_))));
        // A registration that reads back disabled is a mismatch.
        let mut info = logon().expected();
        info.enabled = false;
        assert!(logon().verify(&info).is_err());
    }

    #[test]
    fn percent_in_the_executable_path_is_refused() {
        #[cfg(windows)]
        let exe = r"C:\Users\%USERNAME%\casecontrol-desktop.exe";
        #[cfg(not(windows))]
        let exe = "/home/%USER%/casecontrol-desktop.exe";
        assert!(TaskSpec::boot(SID, Path::new(exe)).is_err());
        assert!(TaskSpec::logon(SID, Path::new(exe)).is_err());
    }

    /// The XML the Task Scheduler returns for a Microsoft Entra account: the principal carries the SID (even where
    /// `IPrincipal::UserId` returns a bare name), the logon trigger the `DOMAIN\user` name, never a SID.
    #[test]
    fn principal_and_logon_trigger_users_come_from_the_xml() {
        const ENTRA: &str = "S-1-12-1-1111111111-2222222222-3333333333-4444444444";
        const ENTRA_NAME: &str = r"AzureAD\JaneDoe";
        let xml = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n",
                "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n",
                "  <Triggers>\n",
                "    <BootTrigger><Enabled>true</Enabled></BootTrigger>\n",
                "    <LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>{name}</UserId>\n    </LogonTrigger>\n",
                "    <LogonTrigger id=\"any\"><Enabled>true</Enabled></LogonTrigger>\n",
                "    <LogonTrigger/>\n",
                "  </Triggers>\n",
                "  <Principals>\n",
                "    <Principal id=\"Author\">\n      <UserId> {sid} </UserId>\n",
                "      <LogonType>InteractiveToken</LogonType>\n    </Principal>\n",
                "  </Principals>\n",
                "</Task>\n"
            ),
            sid = ENTRA,
            name = ENTRA_NAME
        );
        assert_eq!(principal_from_xml(&xml).as_deref(), Some(ENTRA));
        assert_eq!(
            logon_trigger_users_from_xml(&xml),
            vec![Some(ENTRA_NAME.to_owned()), None, None]
        );
        // Our own XML reads back to our SID.
        assert_eq!(principal_from_xml(&logon().to_xml()).as_deref(), Some(SID));
        assert_eq!(
            logon_trigger_users_from_xml(&logon().to_xml()),
            vec![Some(SID.to_owned())]
        );
        assert!(logon_trigger_users_from_xml(&boot().to_xml()).is_empty());
        // Names are decoded; a group principal has no user.
        let named = "<Principals><Principal id=\"A\"><UserId>R&amp;D\\u</UserId></Principal></Principals>";
        assert_eq!(principal_from_xml(named).as_deref(), Some(r"R&D\u"));
        let group =
            "<Principals><Principal id=\"A\"><GroupId>S-1-5-32-545</GroupId></Principal></Principals>";
        assert_eq!(principal_from_xml(group), None);
        assert_eq!(principal_from_xml("<Task/>"), None);
        assert_eq!(xml_unescape("a&#65;&#x42;&bogus;&"), "aAB&bogus;&");
    }

    /// Windows stores a logon trigger's user as `DOMAIN\user`. Registering must accept that name (any case) when it
    /// is the SID's account name, and still refuse (and delete) a trigger for someone else.
    #[test]
    fn logon_trigger_stored_as_account_name_verifies() {
        const NAME: &str = r"AzureAD\JaneDoe";
        let as_name = |i: &mut TaskInfo| i.trigger_users = vec![Some(r"AZUREAD\janedoe".into())];

        // The SID's name resolves: registered and kept, and its state is Matches.
        let store = MemoryTaskStore::with_tamper(as_name).with_account_name(SID, NAME);
        register_verified(&store, &logon()).expect("verified by account name");
        assert!(store.get(&logon().name()).is_some());
        assert_eq!(task_state(&store, &logon()), Ok(TaskState::Matches));
        assert!(!store.calls().iter().any(|c| c.starts_with("delete")));

        // The stored SID still matches without a name.
        let mut info = logon().expected();
        info.trigger_users = vec![Some(SID.to_lowercase())];
        assert_eq!(logon().verify_for(&info, None), Ok(()));

        // Another account's name: a mismatch, the task is deleted.
        let store = MemoryTaskStore::with_tamper(|i| i.trigger_users = vec![Some(r"AzureAD\Mallory".into())])
            .with_account_name(SID, NAME);
        assert!(matches!(
            register_verified(&store, &logon()),
            Err(Error::Mismatch(_))
        ));
        assert_eq!(store.get(&logon().name()), None);

        // A name that cannot be checked (the SID does not resolve) is not accepted.
        let store = MemoryTaskStore::with_tamper(as_name);
        assert!(matches!(
            register_verified(&store, &logon()),
            Err(Error::Mismatch(_))
        ));

        // An empty account name never matches an empty trigger user.
        let mut info = logon().expected();
        info.trigger_users = vec![Some(" ".into())];
        assert!(logon().verify_for(&info, Some("")).is_err());
    }

    #[test]
    fn delete_only_our_task() {
        let ownership = Ownership::of(&boot());
        let store = MemoryTaskStore::new();
        assert_eq!(delete_ours(&store, &ownership), Ok(false));

        store.insert(&boot().name(), boot().expected());
        assert_eq!(delete_ours(&store, &ownership), Ok(true));
        assert_eq!(store.get(&boot().name()), None);

        for tamper in [
            (|i: &mut TaskInfo| i.user_sid = "S-1-5-21-9-9-9-1002".into()) as fn(&mut TaskInfo),
            |i| i.exec_actions[0].path = r"C:\Windows\System32\cmd.exe".into(),
            |i| i.exec_actions[0].arguments = "--apply-at-boot --evil".into(),
            |i| i.other_actions = 1,
            |i| i.exec_actions.push(i.exec_actions[0].clone()),
        ] {
            let mut info = boot().expected();
            tamper(&mut info);
            let store = MemoryTaskStore::new();
            store.insert(&boot().name(), info.clone());
            assert!(
                matches!(delete_ours(&store, &ownership), Err(Error::NotOurs(_))),
                "{info:?}"
            );
            assert_eq!(store.get(&boot().name()), Some(info));
        }
    }

    #[test]
    fn apply_at_boot_invocation_is_exact() {
        assert!(is_apply_at_boot_invocation(["cc.exe", "--apply-at-boot"]));
        assert!(!is_apply_at_boot_invocation(["cc.exe"]));
        assert!(!is_apply_at_boot_invocation(["--apply-at-boot"]));
        assert!(!is_apply_at_boot_invocation([
            "cc.exe",
            "--apply-at-boot",
            "--hidden"
        ]));
        assert!(!is_apply_at_boot_invocation([
            "cc.exe",
            "--hidden",
            "--apply-at-boot"
        ]));
        assert!(!is_apply_at_boot_invocation(["cc.exe", "--APPLY-AT-BOOT"]));
    }

    #[test]
    fn raw_values_round_trip() {
        for v in 0..8 {
            assert_eq!(LogonType::from_raw(v).raw(), v);
        }
        assert_eq!(RunLevel::from_raw(0), RunLevel::LeastPrivilege);
        assert_eq!(RunLevel::from_raw(1), RunLevel::Highest);
        assert_eq!(TriggerKind::from_raw(8), TriggerKind::Boot);
        assert_eq!(TriggerKind::from_raw(9), TriggerKind::Logon);
    }

    /// Reads (never writes) the real Task Scheduler: our logon task name for the current user. Asserts only that
    /// the call works.
    /// Read-only lookups: the current user's SID turns into a `DOMAIN\user` name that maps back to the same SID.
    #[cfg(windows)]
    #[test]
    fn real_account_name_round_trips() {
        let sid = crate::current_user_sid().expect("sid");
        let name = WindowsTaskStore.account_name(&sid).expect("account name");
        assert!(name.contains('\\'), "{name}");
        assert_eq!(crate::sys::account_sid(&name).expect("sid of name"), sid);
        assert_eq!(WindowsTaskStore.account_name("not a sid"), None);
    }

    #[cfg(windows)]
    #[test]
    fn real_task_scheduler_read_ok() {
        let sid = crate::current_user_sid().expect("sid");
        WindowsTaskStore.read(&logon_task_name(&sid)).expect("read");
        WindowsTaskStore
            .read("CaseControl test task that does not exist")
            .expect("read missing");
    }
}
