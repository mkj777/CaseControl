//! The path and ACL check for the start-up task's executable (ADR 0001 #42).
//!
//! The start-up task runs the executable as the user before anyone signs in. That is only as safe as the HKCU Run
//! entry if nobody but that user (and SYSTEM, Administrators, TrustedInstaller) can replace the executable or put a
//! DLL next to it. The install folder can be chosen freely, and a folder on another drive may let other users write.
//! [`check_exe_path`] therefore requires: an absolute path on a fixed local drive (`GetDriveTypeW` is
//! `DRIVE_FIXED`: no removable, network or RAM drive) without `%` (the Task Scheduler expands it), a regular file,
//! no symbolic link, junction or other reparse point in any component, a canonical path equal to the given one, and
//! for the executable and every folder above it an owner and a DACL that grant the dangerous rights only to trusted
//! SIDs.
//!
//! "Dangerous" depends on the object's role ([`PathRole`]):
//! - the executable: write data, append, delete, `WRITE_DAC`, `WRITE_OWNER`;
//! - its folder: add file, add subfolder, delete child, delete, `WRITE_DAC`, `WRITE_OWNER` (DLL planting);
//! - every folder above: delete, delete child, `WRITE_DAC`, `WRITE_OWNER` (renaming or replacing a path component).
//!   Creating new entries there is harmless and allowed (every user may create folders in `C:\`).
//!
//! Generic rights count too (`GENERIC_ALL` everywhere, `GENERIC_WRITE` for the executable and its folder).
//! Inherit-only entries do not apply to the object and are skipped; deny entries are ignored (which can only make
//! the check stricter); an allow entry of a kind this module cannot read, a NULL DACL and a missing owner fail.

use std::path::Path;

use crate::Error;

/// `S-1-5-18`, LocalSystem.
pub const SYSTEM_SID: &str = "S-1-5-18";
/// `S-1-5-32-544`, BUILTIN\Administrators.
pub const ADMINISTRATORS_SID: &str = "S-1-5-32-544";
/// `NT SERVICE\TrustedInstaller`.
pub const TRUSTED_INSTALLER_SID: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

const FILE_WRITE_DATA: u32 = 0x0002; // = FILE_ADD_FILE on a folder
const FILE_APPEND_DATA: u32 = 0x0004; // = FILE_ADD_SUBDIRECTORY on a folder
const FILE_DELETE_CHILD: u32 = 0x0040;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// `DRIVE_FIXED` of `GetDriveTypeW`.
#[cfg_attr(not(windows), allow(dead_code))]
const DRIVE_FIXED: u32 = 3;

/// `INHERIT_ONLY_ACE`.
pub const INHERIT_ONLY_ACE: u8 = 0x08;

/// The kind of an access control entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AceKind {
    /// `ACCESS_ALLOWED_ACE_TYPE`.
    Allow,
    /// `ACCESS_DENIED_ACE_TYPE` (and the other deny kinds).
    Deny,
    /// An allow entry of another kind (object, callback); its SID is not read.
    OtherAllow,
    /// Anything else (audit, label, ...); not an access grant.
    Other,
}

/// One access control entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    /// Its kind.
    pub kind: AceKind,
    /// The trustee (for [`AceKind::Allow`] and [`AceKind::Deny`]).
    pub sid: Option<String>,
    /// The access mask.
    pub mask: u32,
    /// `AceFlags` (inheritance).
    pub flags: u8,
}

/// Owner and DACL of a file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSecurity {
    /// Owner SID.
    pub owner: Option<String>,
    /// The DACL; `None` is a NULL DACL (everyone has full access).
    pub dacl: Option<Vec<Ace>>,
}

/// What an object is to the executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRole {
    /// The executable itself.
    File,
    /// The folder that holds it.
    FileFolder,
    /// A folder further up, up to the drive root.
    Ancestor,
}

impl PathRole {
    /// The rights only trusted SIDs may hold on an object of this role.
    pub fn dangerous_rights(self) -> u32 {
        match self {
            PathRole::File => {
                FILE_WRITE_DATA
                    | FILE_APPEND_DATA
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | GENERIC_WRITE
                    | GENERIC_ALL
            }
            PathRole::FileFolder => {
                FILE_WRITE_DATA
                    | FILE_APPEND_DATA
                    | FILE_DELETE_CHILD
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | GENERIC_WRITE
                    | GENERIC_ALL
            }
            PathRole::Ancestor => FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL,
        }
    }
}

/// Whether `sid` is trusted for the executable of `principal_sid`'s task: the principal, SYSTEM, Administrators or
/// TrustedInstaller.
pub fn is_trusted(sid: &str, principal_sid: &str) -> bool {
    [
        principal_sid,
        SYSTEM_SID,
        ADMINISTRATORS_SID,
        TRUSTED_INSTALLER_SID,
    ]
    .iter()
    .any(|t| t.eq_ignore_ascii_case(sid))
}

/// Pure evaluation of one object's owner and DACL (see the module docs). `Err` names the first problem.
pub fn check_object(security: &ObjectSecurity, role: PathRole, principal_sid: &str) -> Result<(), String> {
    match &security.owner {
        Some(owner) if is_trusted(owner, principal_sid) => {}
        Some(owner) => return Err(format!("it is owned by {owner}")),
        None => return Err("its owner cannot be read".to_owned()),
    }
    let Some(dacl) = &security.dacl else {
        return Err("it has no access list (everyone has full access)".to_owned());
    };
    let dangerous = role.dangerous_rights();
    for ace in dacl {
        if ace.flags & INHERIT_ONLY_ACE != 0 || ace.mask & dangerous == 0 {
            continue;
        }
        match (ace.kind, &ace.sid) {
            (AceKind::Allow, Some(sid)) if is_trusted(sid, principal_sid) => {}
            (AceKind::Allow, Some(sid)) => {
                return Err(format!(
                    "{sid} may change it (rights 0x{:08X})",
                    ace.mask & dangerous
                ));
            }
            (AceKind::Allow, None) | (AceKind::OtherAllow, _) => {
                return Err(format!(
                    "an access entry of an unsupported kind grants rights 0x{:08X}",
                    ace.mask & dangerous
                ));
            }
            (AceKind::Deny | AceKind::Other, _) => {}
        }
    }
    Ok(())
}

/// Checks that `exe` is safe to run as `principal_sid` from the start-up task (see the module docs).
pub fn check_exe_path(exe: &Path, principal_sid: &str) -> Result<(), Error> {
    let refuse = |why: String| {
        Error::Invalid(format!(
            "{} cannot be used for the start-up task: {why}",
            exe.display()
        ))
    };
    if !crate::identity::is_user_sid(principal_sid) {
        return Err(refuse(format!("{principal_sid:?} is not a user SID")));
    }
    if !exe.is_absolute() || exe.to_str().is_none() {
        return Err(refuse("not an absolute Unicode path".to_owned()));
    }
    if exe.to_string_lossy().contains('%') {
        // The Task Scheduler expands environment variables in the program path.
        return Err(refuse("the path contains '%'".to_owned()));
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let Some(Component::Prefix(prefix)) = exe.components().next() else {
            return Err(refuse("not a path on a local drive letter".to_owned()));
        };
        let Prefix::Disk(letter) = prefix.kind() else {
            return Err(refuse("not a path on a local drive letter".to_owned()));
        };
        let root = format!("{}:\\", char::from(letter));
        let kind = crate::sys::drive_type(&root);
        if kind != DRIVE_FIXED {
            return Err(refuse(format!(
                "drive {root} is not a fixed local drive (drive type {kind})"
            )));
        }
    }
    for ancestor in exe.ancestors() {
        if ancestor.parent().is_none() {
            break; // the drive root
        }
        let meta = std::fs::symlink_metadata(ancestor)
            .map_err(|e| refuse(format!("{}: {e}", ancestor.display())))?;
        if is_reparse_point(&meta) {
            return Err(refuse(format!(
                "{} is a link or reparse point",
                ancestor.display()
            )));
        }
    }
    if !std::fs::symlink_metadata(exe)
        .map_err(|e| refuse(e.to_string()))?
        .is_file()
    {
        return Err(refuse("not a regular file".to_owned()));
    }
    let canonical = std::fs::canonicalize(exe).map_err(|e| refuse(e.to_string()))?;
    let canonical = canonical.to_string_lossy();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    if !canonical.eq_ignore_ascii_case(&exe.to_string_lossy()) {
        return Err(refuse(format!("it resolves to {canonical}")));
    }
    #[cfg(windows)]
    {
        for (i, object) in exe.ancestors().enumerate() {
            let role = match i {
                0 => PathRole::File,
                1 => PathRole::FileFolder,
                _ => PathRole::Ancestor,
            };
            let security = crate::sys::object_security(object)?;
            check_object(&security, role, principal_sid)
                .map_err(|why| refuse(format!("{}: {why}", object.display())))?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "S-1-5-21-1-2-3-1001";
    const OTHER: &str = "S-1-5-21-1-2-3-1002";
    const AUTHENTICATED_USERS: &str = "S-1-5-11";
    const USERS: &str = "S-1-5-32-545";
    const FULL: u32 = 0x001F_01FF;
    const READ_EXECUTE: u32 = 0x0012_00A9;
    const MODIFY: u32 = 0x0013_01BF;

    fn allow(sid: &str, mask: u32) -> Ace {
        Ace {
            kind: AceKind::Allow,
            sid: Some(sid.into()),
            mask,
            flags: 0,
        }
    }

    fn object(owner: &str, aces: Vec<Ace>) -> ObjectSecurity {
        ObjectSecurity {
            owner: Some(owner.into()),
            dacl: Some(aces),
        }
    }

    /// The default per-user install folder: SYSTEM, Administrators and the user with full control.
    fn profile_folder() -> ObjectSecurity {
        object(
            USER,
            vec![
                allow(SYSTEM_SID, FULL),
                allow(ADMINISTRATORS_SID, FULL),
                allow(USER, FULL),
            ],
        )
    }

    #[test]
    fn default_install_folder_passes_every_role() {
        for role in [PathRole::File, PathRole::FileFolder, PathRole::Ancestor] {
            assert_eq!(check_object(&profile_folder(), role, USER), Ok(()), "{role:?}");
        }
    }

    #[test]
    fn drive_root_passes_as_ancestor_only() {
        // C:\ on Windows 11: Authenticated Users may create folders in the root itself (AppendData) and get Modify
        // inherit-only for what they create; Users read and execute.
        let root = object(
            TRUSTED_INSTALLER_SID,
            vec![
                allow(ADMINISTRATORS_SID, FULL),
                allow(SYSTEM_SID, FULL),
                allow(USERS, READ_EXECUTE),
                allow(AUTHENTICATED_USERS, FILE_APPEND_DATA),
                Ace {
                    flags: INHERIT_ONLY_ACE | 0x03,
                    ..allow(AUTHENTICATED_USERS, MODIFY)
                },
            ],
        );
        assert_eq!(check_object(&root, PathRole::Ancestor, USER), Ok(()));
        assert!(check_object(&root, PathRole::FileFolder, USER).is_err());
    }

    #[test]
    fn other_writers_fail() {
        let shared = object(USER, vec![allow(USER, FULL), allow(AUTHENTICATED_USERS, MODIFY)]);
        for role in [PathRole::File, PathRole::FileFolder, PathRole::Ancestor] {
            assert!(check_object(&shared, role, USER).is_err(), "{role:?}");
        }
        // Another user's right to add files is DLL planting in the executable's folder, harmless further up.
        let add_file = object(USER, vec![allow(USER, FULL), allow(OTHER, FILE_WRITE_DATA)]);
        assert!(check_object(&add_file, PathRole::FileFolder, USER).is_err());
        assert!(check_object(&add_file, PathRole::File, USER).is_err());
        assert_eq!(check_object(&add_file, PathRole::Ancestor, USER), Ok(()));
        // Delete-child, WRITE_DAC, WRITE_OWNER and GENERIC_ALL fail everywhere.
        for mask in [FILE_DELETE_CHILD, WRITE_DAC, WRITE_OWNER, GENERIC_ALL, DELETE] {
            let o = object(USER, vec![allow(OTHER, mask)]);
            assert!(check_object(&o, PathRole::Ancestor, USER).is_err(), "{mask:#x}");
            assert!(check_object(&o, PathRole::FileFolder, USER).is_err(), "{mask:#x}");
        }
        // Read and execute for everyone is fine.
        let readable = object(USER, vec![allow(USER, FULL), allow("S-1-1-0", READ_EXECUTE)]);
        assert_eq!(check_object(&readable, PathRole::File, USER), Ok(()));
    }

    #[test]
    fn owner_null_dacl_and_unknown_entries() {
        let mut o = profile_folder();
        o.owner = Some(OTHER.into());
        assert!(check_object(&o, PathRole::Ancestor, USER).is_err());
        o.owner = None;
        assert!(check_object(&o, PathRole::Ancestor, USER).is_err());
        let null_dacl = ObjectSecurity {
            owner: Some(USER.into()),
            dacl: None,
        };
        assert!(check_object(&null_dacl, PathRole::Ancestor, USER).is_err());
        let unknown = object(
            USER,
            vec![Ace {
                kind: AceKind::OtherAllow,
                sid: None,
                mask: DELETE,
                flags: 0,
            }],
        );
        assert!(check_object(&unknown, PathRole::Ancestor, USER).is_err());
        // Deny entries and inherit-only grants do not matter.
        let deny = object(
            USER,
            vec![
                Ace {
                    kind: AceKind::Deny,
                    ..allow(OTHER, FULL)
                },
                Ace {
                    flags: INHERIT_ONLY_ACE,
                    ..allow(OTHER, FULL)
                },
            ],
        );
        assert_eq!(check_object(&deny, PathRole::File, USER), Ok(()));
    }

    #[test]
    fn trusted_sids_are_exactly_four() {
        for sid in [USER, SYSTEM_SID, ADMINISTRATORS_SID, TRUSTED_INSTALLER_SID] {
            assert!(is_trusted(sid, USER));
        }
        for sid in [
            OTHER,
            USERS,
            AUTHENTICATED_USERS,
            "S-1-1-0",
            "S-1-3-0",
            "S-1-5-32-547",
        ] {
            assert!(!is_trusted(sid, USER), "{sid}");
        }
    }

    #[test]
    fn relative_paths_and_non_user_principals_are_refused() {
        assert!(check_exe_path(Path::new("casecontrol-desktop.exe"), USER).is_err());
        assert!(check_exe_path(&std::env::temp_dir().join("x.exe"), SYSTEM_SID).is_err());
    }

    #[test]
    fn percent_in_the_path_is_refused_before_any_lookup() {
        let exe = std::env::temp_dir()
            .join("%APPDATA%")
            .join("casecontrol-desktop.exe");
        let err = check_exe_path(&exe, USER).expect_err("refused");
        assert!(err.to_string().contains('%'), "{err}");
    }

    /// The test executable's own drive is fixed on developer machines and runners; a drive letter that is not
    /// mapped (type `DRIVE_NO_ROOT_DIR`) is refused for its type, before any file is looked at.
    #[cfg(windows)]
    #[test]
    fn only_fixed_drives_pass_the_drive_check() {
        let exe = std::env::current_exe().expect("exe");
        let letter = exe.to_string_lossy().chars().next().expect("letter");
        assert_eq!(crate::sys::drive_type(&format!("{letter}:\\")), DRIVE_FIXED);
        let unused = ('D'..='Z')
            .rev()
            .find(|l| crate::sys::drive_type(&format!("{l}:\\")) == 1)
            .expect("an unmapped drive letter");
        let err = check_exe_path(
            Path::new(&format!(r"{unused}:\CaseControl\casecontrol-desktop.exe")),
            USER,
        )
        .expect_err("refused");
        assert!(err.to_string().contains("not a fixed local drive"), "{err}");
    }

    /// Reads (never writes) the real ACLs of the test executable. On a developer machine it lives under the user's
    /// profile and passes; elsewhere (CI checkout folders) the result is only logged.
    #[cfg(windows)]
    #[test]
    fn real_exe_check_runs() {
        let exe = std::env::current_exe().expect("exe");
        let sid = crate::current_user_sid().expect("sid");
        if crate::is_user_sid(&sid) {
            let result = check_exe_path(&exe, &sid);
            #[allow(clippy::print_stderr)]
            {
                eprintln!("check_exe_path({}) = {result:?}", exe.display());
            }
        }
        let security = crate::sys::object_security(&exe).expect("security of the test executable");
        assert!(security.owner.is_some());
    }
}
