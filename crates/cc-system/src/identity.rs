//! Who is running: the current token's user, the interactive user of a Windows session, that user's profile folder
//! (read from `HKLM\...\ProfileList`, never written) and the owner of a file.

use std::path::{Path, PathBuf};

use crate::Error;

/// A Windows account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The account's SID in string form (`S-1-5-21-...`).
    pub sid: String,
    /// Domain or computer name.
    pub domain: String,
    /// User name.
    pub name: String,
}

/// Whether `sid` is the string SID of a real user account: a local or domain account (`S-1-5-21-<3 sub
/// authorities>-<rid>`) or a Microsoft Entra ID account (`S-1-12-1-<4 sub authorities>`). Well-known and service
/// SIDs (SYSTEM `S-1-5-18`, LOCAL / NETWORK SERVICE, `S-1-5-80-...`, groups such as Administrators) are not.
pub fn is_user_sid(sid: &str) -> bool {
    let parts: Vec<&str> = sid.split('-').collect();
    let numeric = |p: &&str| !p.is_empty() && p.len() <= 10 && p.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        ["S", "1", "5", "21", rest @ ..] => rest.len() == 4 && rest.iter().all(numeric),
        ["S", "1", "12", "1", rest @ ..] => rest.len() == 4 && rest.iter().all(numeric),
        _ => false,
    }
}

/// The SID of the user the current process token belongs to. In an elevated process this is the account that
/// approved the elevation (an administrator who typed their credentials), not necessarily the signed-in user.
pub fn current_user_sid() -> Result<String, Error> {
    #[cfg(windows)]
    {
        crate::sys::token_user_sid()
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
}

/// How the current process token is elevated, for logs (ADR 0001 #42: the start-up task's log names it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenElevation {
    /// `TOKEN_ELEVATION_TYPE`: 1 default (no split token: UAC off, a service, S4U), 2 full (elevated), 3 limited.
    pub elevation_type: i32,
    /// The SID of the integrity level, e.g. `S-1-16-8192` (medium).
    pub integrity_sid: String,
}

impl TokenElevation {
    /// A short name of the elevation type.
    pub fn elevation_name(&self) -> &'static str {
        match self.elevation_type {
            1 => "default",
            2 => "full",
            3 => "limited",
            _ => "unknown",
        }
    }

    /// A short name of the integrity level.
    pub fn integrity_name(&self) -> &'static str {
        integrity_name(&self.integrity_sid)
    }
}

/// The name of the integrity level with SID `sid` (`S-1-16-<rid>`): untrusted, low, medium, medium-plus, high,
/// system, protected, or `unknown`.
pub fn integrity_name(sid: &str) -> &'static str {
    match sid
        .strip_prefix("S-1-16-")
        .and_then(|rid| rid.parse::<u32>().ok())
    {
        Some(0) => "untrusted",
        Some(0x1000) => "low",
        Some(0x2000) => "medium",
        Some(0x2100) => "medium-plus",
        Some(0x3000) => "high",
        Some(0x4000) => "system",
        Some(0x5000) => "protected",
        _ => "unknown",
    }
}

/// The current process token's elevation type and integrity level.
pub fn token_elevation() -> Result<TokenElevation, Error> {
    #[cfg(windows)]
    {
        let (elevation_type, integrity_sid) = crate::sys::token_elevation()?;
        Ok(TokenElevation {
            elevation_type,
            integrity_sid,
        })
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
}

/// The Windows session of the current process.
pub fn current_session_id() -> Result<u32, Error> {
    #[cfg(windows)]
    {
        crate::sys::process_session(std::process::id())
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
}

/// The user signed in to Windows session `session` (`WTSQuerySessionInformation` user and domain, resolved with
/// `LookupAccountName`). Fails when nobody is signed in there.
pub fn session_user(session: u32) -> Result<Account, Error> {
    #[cfg(windows)]
    {
        crate::sys::session_user(session)
    }
    #[cfg(not(windows))]
    {
        let _ = session;
        Err(Error::Unsupported)
    }
}

/// The user signed in to the current process's session (see [`session_user`]).
pub fn interactive_user() -> Result<Account, Error> {
    session_user(current_session_id()?)
}

/// The profile folder of the user with `sid`, from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\
/// ProfileList\<sid>\ProfileImagePath` (environment variables expanded). Fails when the user has no profile.
pub fn profile_path(sid: &str) -> Result<PathBuf, Error> {
    if !is_user_sid(sid) {
        return Err(Error::Invalid(format!("{sid:?} is not a user SID")));
    }
    #[cfg(windows)]
    {
        crate::sys::profile_path(sid)
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
}

/// The SID of the owner of file or folder `path`.
pub fn file_owner_sid(path: &Path) -> Result<String, Error> {
    #[cfg(windows)]
    {
        crate::sys::object_security(path).and_then(|s| {
            s.owner
                .ok_or_else(|| Error::Invalid(format!("{} has no owner", path.display())))
        })
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(Error::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_sids() {
        for good in [
            "S-1-5-21-1111111111-2222222222-3333333333-1001",
            "S-1-5-21-1-2-3-500",
            "S-1-12-1-1234567890-1234567890-1234567890-1234567890",
        ] {
            assert!(is_user_sid(good), "{good}");
        }
        for bad in [
            "S-1-5-18",
            "S-1-5-19",
            "S-1-5-20",
            "S-1-5-32-544",
            "S-1-5-32-545",
            "S-1-5-11",
            "S-1-1-0",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
            "S-1-5-21-1-2-3",
            "S-1-5-21-1-2-3-4-5",
            "S-1-5-21-1-2-3-x",
            "S-1-5-21-1-2-3-12345678901",
            "s-1-5-21-1-2-3-1001",
            "",
            "S-1-5-21-1-2-3-1001\" ",
        ] {
            assert!(!is_user_sid(bad), "{bad}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn real_identity_ok() {
        let sid = current_user_sid().expect("token user");
        assert!(sid.starts_with("S-1-"), "{sid}");
        current_session_id().expect("session");
        // A CI runner's process may run in a session without an interactive user; only the calls are checked.
        let _ = interactive_user();
        if is_user_sid(&sid) {
            let _ = profile_path(&sid);
        }
        file_owner_sid(&std::env::temp_dir()).expect("owner of the temp folder");
        let elevation = token_elevation().expect("token elevation");
        assert!((1..=3).contains(&elevation.elevation_type), "{elevation:?}");
        assert!(elevation.integrity_sid.starts_with("S-1-16-"), "{elevation:?}");
        assert_ne!(elevation.integrity_name(), "unknown", "{elevation:?}");
    }

    #[test]
    fn integrity_levels_have_names() {
        assert_eq!(integrity_name("S-1-16-8192"), "medium");
        assert_eq!(integrity_name("S-1-16-12288"), "high");
        assert_eq!(integrity_name("S-1-16-16384"), "system");
        assert_eq!(integrity_name("S-1-16-4096"), "low");
        assert_eq!(integrity_name("S-1-5-18"), "unknown");
        assert_eq!(integrity_name("S-1-16-x"), "unknown");
        let t = TokenElevation {
            elevation_type: 3,
            integrity_sid: "S-1-16-8192".into(),
        };
        assert_eq!((t.elevation_name(), t.integrity_name()), ("limited", "medium"));
    }
}
