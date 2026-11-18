//! Autostart entries: where Windows starts programs from, the `StartupApproved` switch Task Manager uses, and the
//! pure parsers the scanner and the actions share.
//!
//! Sources (read-only scan, ADR 0001 #31): the `Run` / `RunOnce` keys of HKCU, HKLM and HKLM `WOW6432Node`, the
//! per-user and the common Startup folder, each with its state under
//! `Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{Run,Run32,StartupFolder}`, plus the start type
//! of services (read by the service scan). Turning an entry off writes the `StartupApproved` value; the `Run` value
//! itself is never deleted.

use serde::{Deserialize, Serialize};

use crate::sources::{ScanError, StartType};

/// `Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved`, under HKCU or HKLM.
pub const STARTUP_APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// Where an autostart item lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutostartLocation {
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
    HkcuRun,
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce` (runs once; cannot be switched off).
    HkcuRunOnce,
    /// `HKLM\Software\Microsoft\Windows\CurrentVersion\Run`.
    HklmRun,
    /// `HKLM\Software\Microsoft\Windows\CurrentVersion\RunOnce` (runs once; cannot be switched off).
    HklmRunOnce,
    /// `HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run` (32-bit programs).
    HklmRun32,
    /// `HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\RunOnce` (runs once; cannot be switched off).
    HklmRunOnce32,
    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`.
    UserStartupFolder,
    /// `%ProgramData%\Microsoft\Windows\Start Menu\Programs\StartUp`.
    CommonStartupFolder,
    /// A Win32 service; "enabled" means start type Auto.
    Service,
}

/// The `StartupApproved` value that switches an app entry on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalKey {
    /// HKLM (`true`) or HKCU (`false`).
    pub machine: bool,
    /// Subkey of [`STARTUP_APPROVED`]: `Run`, `Run32` or `StartupFolder`.
    pub subkey: &'static str,
}

impl ApprovalKey {
    /// Full path below the hive.
    pub fn path(&self) -> String {
        format!(r"{STARTUP_APPROVED}\{}", self.subkey)
    }
}

impl AutostartLocation {
    /// Every location an [`AutostartSource`] reports (all but [`AutostartLocation::Service`]).
    pub const APP_LOCATIONS: [AutostartLocation; 8] = [
        Self::HkcuRun,
        Self::HkcuRunOnce,
        Self::HklmRun,
        Self::HklmRunOnce,
        Self::HklmRun32,
        Self::HklmRunOnce32,
        Self::UserStartupFolder,
        Self::CommonStartupFolder,
    ];

    /// The `StartupApproved` value for entries of this location, `None` for `RunOnce` keys and services.
    pub fn approval_key(self) -> Option<ApprovalKey> {
        let (machine, subkey) = match self {
            Self::HkcuRun => (false, "Run"),
            Self::HklmRun => (true, "Run"),
            Self::HklmRun32 => (true, "Run32"),
            Self::UserStartupFolder => (false, "StartupFolder"),
            Self::CommonStartupFolder => (true, "StartupFolder"),
            Self::HkcuRunOnce | Self::HklmRunOnce | Self::HklmRunOnce32 | Self::Service => return None,
        };
        Some(ApprovalKey { machine, subkey })
    }

    /// Whether changing an item here needs administrator rights (HKLM, the common Startup folder, services).
    pub fn needs_admin(self) -> bool {
        match self {
            Self::HkcuRun | Self::HkcuRunOnce | Self::UserStartupFolder => false,
            Self::HklmRun
            | Self::HklmRunOnce
            | Self::HklmRun32
            | Self::HklmRunOnce32
            | Self::CommonStartupFolder
            | Self::Service => true,
        }
    }
}

/// One raw autostart entry as an [`AutostartSource`] reads it, before catalogue matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartEntry {
    /// Where it lives (never [`AutostartLocation::Service`]).
    pub location: AutostartLocation,
    /// Registry value name, or the file name in a Startup folder (`OpenRGB.lnk`). This is also the value name
    /// under `StartupApproved`.
    pub name: String,
    /// The command line (Run keys) or the shortcut target with its arguments (Startup folders).
    pub command: String,
    /// The executable the entry starts, if it could be determined.
    pub executable: Option<String>,
    /// The `StartupApproved` state: `Some(false)` when switched off, `None` when there is no value (= on).
    pub approved: Option<bool>,
}

/// Reads autostart entries.
pub trait AutostartSource {
    /// Every entry of the [`AutostartLocation::APP_LOCATIONS`]. Unreadable locations contribute nothing.
    fn autostart_entries(&self) -> Result<Vec<AutostartEntry>, ScanError>;
}

/// An [`AutostartSource`] that reports nothing (the default of [`Scanner::new`](crate::Scanner::new)).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAutostart;

impl AutostartSource for NoAutostart {
    fn autostart_entries(&self) -> Result<Vec<AutostartEntry>, ScanError> {
        Ok(Vec::new())
    }
}

/// An autostart item attached to a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartItem {
    /// Where it lives.
    pub location: AutostartLocation,
    /// Value name, Startup folder file name, or service name.
    pub name: String,
    /// Command line or shortcut target; `None` for services.
    pub command: Option<String>,
    /// Starts with Windows now: `StartupApproved` on (or absent), `RunOnce` present, or service start type Auto.
    pub enabled: bool,
    /// Service start type; `None` for app entries or when unreadable.
    pub start_type: Option<StartType>,
    /// CaseControl can switch it off and on: Run keys and Startup folders through `StartupApproved`, services
    /// whose start type is Auto or Manual. `RunOnce` entries and disabled or boot services cannot be toggled.
    pub toggleable: bool,
    /// Changing it needs administrator rights (one UAC prompt per batch).
    pub needs_admin: bool,
}

impl AutostartItem {
    /// The item for a raw app entry.
    pub fn from_entry(entry: &AutostartEntry) -> Self {
        Self {
            location: entry.location,
            name: entry.name.clone(),
            command: Some(entry.command.clone()),
            enabled: entry.approved.unwrap_or(true),
            start_type: None,
            toggleable: entry.location.approval_key().is_some(),
            needs_admin: entry.location.needs_admin(),
        }
    }

    /// The item for a Win32 service.
    pub fn service(name: &str, start_type: Option<StartType>) -> Self {
        Self {
            location: AutostartLocation::Service,
            name: name.to_owned(),
            command: None,
            enabled: start_type == Some(StartType::Auto),
            start_type,
            toggleable: matches!(start_type, Some(StartType::Auto | StartType::Manual)),
            needs_admin: true,
        }
    }

    /// Enabled and switchable: what "Turn off autostart" changes.
    pub fn can_turn_off(&self) -> bool {
        self.toggleable && self.enabled
    }
}

/// Parses a `StartupApproved` value: `Some(true)` = on, `Some(false)` = off, `None` = empty / not a state.
///
/// Byte 0 carries the state; Windows writes `0x02` (on) and `0x03` (off) and older builds `0x06` / `0x07` or
/// `0x01`, so an even byte means on and an odd byte means off.
pub fn parse_startup_approved(bytes: &[u8]) -> Option<bool> {
    bytes.first().map(|b| b & 1 == 0)
}

/// The 12-byte `StartupApproved` value Task Manager writes: `02 00 00 00` + 8 zero bytes for on, `03 00 00 00` +
/// the FILETIME of the change (little-endian) for off.
pub fn encode_startup_approved(enabled: bool, filetime: u64) -> [u8; 12] {
    let mut out = [0u8; 12];
    if enabled {
        out[0] = 0x02;
    } else {
        out[0] = 0x03;
        out[4..].copy_from_slice(&filetime.to_le_bytes());
    }
    out
}

/// 100 ns intervals between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// `time` as a Windows FILETIME (100 ns since 1601-01-01 UTC); 0 for times before 1970.
pub fn filetime(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| FILETIME_UNIX_EPOCH + u64::try_from(d.as_nanos() / 100).unwrap_or(u64::MAX / 2))
        .unwrap_or(0)
}

/// The executable of a Run-key command line: the quoted part if it starts with a quote; otherwise the shortest
/// prefix ending in `.exe` followed by the end or whitespace (unquoted paths with spaces, as Windows resolves them);
/// otherwise the first whitespace-separated token. `None` for an empty command.
pub fn command_executable(command: &str) -> Option<String> {
    let s = command.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        let path = rest[..end].trim();
        return (!path.is_empty()).then(|| path.to_owned());
    }
    // ASCII lower-casing keeps byte offsets, so indices found in `lower` are valid in `s`.
    let lower = s.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(".exe") {
        let end = from + i + 4;
        if end == s.len() || s[end..].starts_with(char::is_whitespace) {
            return Some(s[..end].to_owned());
        }
        from = end;
    }
    s.split_whitespace().next().map(str::to_owned)
}

/// The file-name component of a Windows path (`C:\dir\tool.exe` -> `tool.exe`).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Target of a Windows shortcut (`.lnk`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellLink {
    /// Local target path (`LinkInfo` local base path + common path suffix), or the relative path.
    pub target: Option<String>,
    /// Command-line arguments.
    pub arguments: Option<String>,
}

const LNK_HEADER_SIZE: usize = 0x4C;
const LNK_HAS_ID_LIST: u32 = 0x01;
const LNK_HAS_LINK_INFO: u32 = 0x02;
const LNK_HAS_NAME: u32 = 0x04;
const LNK_HAS_RELATIVE_PATH: u32 = 0x08;
const LNK_HAS_WORKING_DIR: u32 = 0x10;
const LNK_HAS_ARGUMENTS: u32 = 0x20;
const LNK_IS_UNICODE: u32 = 0x80;
const LINK_INFO_VOLUME_AND_LOCAL_PATH: u32 = 0x01;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// NUL-terminated single-byte string at `at` (decoded as Latin-1, enough for matching executable names).
fn ansi_z(b: &[u8], at: usize) -> Option<String> {
    let tail = b.get(at..)?;
    let len = tail.iter().position(|&c| c == 0)?;
    Some(tail[..len].iter().map(|&c| char::from(c)).collect())
}

/// Little-endian UTF-16 code units of `bytes` (a trailing odd byte is ignored).
fn utf16_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect()
}

/// NUL-terminated UTF-16LE string at `at`.
fn wide_z(b: &[u8], at: usize) -> Option<String> {
    let tail = b.get(at..)?;
    let units = utf16_units(tail);
    // A string without a terminator inside the buffer is malformed.
    let len = units.iter().position(|&u| u == 0)?;
    Some(String::from_utf16_lossy(&units[..len]))
}

/// Parses the target and arguments of a shell link (MS-SHLLINK). Returns `None` for anything that is not a
/// well-formed link; never panics on malformed input.
pub fn parse_shell_link(b: &[u8]) -> Option<ShellLink> {
    if u32_at(b, 0)? as usize != LNK_HEADER_SIZE || b.len() < LNK_HEADER_SIZE {
        return None;
    }
    let flags = u32_at(b, 0x14)?;
    let mut pos = LNK_HEADER_SIZE;
    if flags & LNK_HAS_ID_LIST != 0 {
        pos = pos.checked_add(2 + usize::from(u16_at(b, pos)?))?;
    }
    let mut link = ShellLink::default();
    if flags & LNK_HAS_LINK_INFO != 0 {
        let info = pos;
        let size = u32_at(b, info)? as usize;
        let header = u32_at(b, info + 4)? as usize;
        let info_flags = u32_at(b, info + 8)?;
        if size < header || b.len() < info.checked_add(size)? {
            return None;
        }
        let info_bytes = &b[info..info + size];
        if info_flags & LINK_INFO_VOLUME_AND_LOCAL_PATH != 0 {
            let unicode = header >= 0x24;
            let base = if unicode {
                wide_z(info_bytes, u32_at(info_bytes, 0x1C)? as usize)
            } else {
                ansi_z(info_bytes, u32_at(info_bytes, 0x10)? as usize)
            };
            let suffix = if unicode {
                wide_z(info_bytes, u32_at(info_bytes, 0x20)? as usize)
            } else {
                ansi_z(info_bytes, u32_at(info_bytes, 0x18)? as usize)
            };
            link.target = base
                .map(|base| format!("{base}{}", suffix.unwrap_or_default()))
                .filter(|t| !t.is_empty());
        }
        pos = info + size;
    }
    let unicode = flags & LNK_IS_UNICODE != 0;
    let read_string = |pos: &mut usize| -> Option<String> {
        let count = usize::from(u16_at(b, *pos)?);
        let start = *pos + 2;
        let bytes = if unicode { count * 2 } else { count };
        let raw = b.get(start..start + bytes)?;
        *pos = start + bytes;
        Some(if unicode {
            String::from_utf16_lossy(&utf16_units(raw))
        } else {
            raw.iter().map(|&c| char::from(c)).collect()
        })
    };
    if flags & LNK_HAS_NAME != 0 {
        read_string(&mut pos)?;
    }
    if flags & LNK_HAS_RELATIVE_PATH != 0 {
        let relative = read_string(&mut pos)?;
        if link.target.is_none() && !relative.is_empty() {
            link.target = Some(relative);
        }
    }
    if flags & LNK_HAS_WORKING_DIR != 0 {
        read_string(&mut pos)?;
    }
    if flags & LNK_HAS_ARGUMENTS != 0 {
        link.arguments = Some(read_string(&mut pos)?).filter(|a| !a.is_empty());
    }
    Some(link)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn approved_values_parse_like_task_manager() {
        assert_eq!(
            parse_startup_approved(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(true)
        );
        assert_eq!(
            parse_startup_approved(&[0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Some(true)
        );
        // SignalRgb on the development machine.
        assert_eq!(
            parse_startup_approved(&[0x03, 0, 0, 0, 0xF3, 0x1A, 0x5A, 0x34, 0x20, 0x6E, 0xDD, 0x01]),
            Some(false)
        );
        assert_eq!(parse_startup_approved(&[0x01]), Some(false));
        assert_eq!(parse_startup_approved(&[0x07, 0, 0, 0]), Some(false));
        assert_eq!(parse_startup_approved(&[]), None);
    }

    #[test]
    fn approved_values_encode_like_task_manager() {
        assert_eq!(
            encode_startup_approved(true, 0x01DD_6E20_345A_1AF3),
            [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            encode_startup_approved(false, 0x01DD_6E20_345A_1AF3),
            [0x03, 0, 0, 0, 0xF3, 0x1A, 0x5A, 0x34, 0x20, 0x6E, 0xDD, 0x01]
        );
        for on in [true, false] {
            assert_eq!(parse_startup_approved(&encode_startup_approved(on, 7)), Some(on));
        }
    }

    #[test]
    fn filetime_conversion() {
        assert_eq!(filetime(std::time::UNIX_EPOCH), 116_444_736_000_000_000);
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        assert_eq!(filetime(t), 116_444_736_010_000_000);
    }

    #[test]
    fn command_lines() {
        let cases = [
            (
                r#""C:\Users\max\AppData\Local\VortxEngine\SignalRgbLauncher.exe" --silent"#,
                Some(r"C:\Users\max\AppData\Local\VortxEngine\SignalRgbLauncher.exe"),
            ),
            (
                r#""C:\Program Files\LGHUB\system_tray\lghub_system_tray.exe" --minimized"#,
                Some(r"C:\Program Files\LGHUB\system_tray\lghub_system_tray.exe"),
            ),
            (
                r"C:\Program Files\Docker\Docker\Docker Desktop.exe",
                Some(r"C:\Program Files\Docker\Docker\Docker Desktop.exe"),
            ),
            (
                r"C:\Program Files (x86)\LonelyScreen\lonelyscreen.exe /start_context sys_auto",
                Some(r"C:\Program Files (x86)\LonelyScreen\lonelyscreen.exe"),
            ),
            (r"C:\a.exe.bak\b.exe -x", Some(r"C:\a.exe.bak\b.exe")),
            ("rundll32 shell32.dll,Foo", Some("rundll32")),
            (r#""unterminated\x.exe"#, Some(r"unterminated\x.exe")),
            ("   ", None),
            ("\"\"", None),
        ];
        for (cmd, expected) in cases {
            assert_eq!(command_executable(cmd).as_deref(), expected, "{cmd}");
        }
        assert_eq!(file_name(r"C:\x\OpenRGB.exe"), "OpenRGB.exe");
        assert_eq!(file_name("OpenRGB.exe"), "OpenRGB.exe");
        assert_eq!(file_name("C:/x/y.exe"), "y.exe");
    }

    #[test]
    fn locations() {
        assert_eq!(
            AutostartLocation::HkcuRun.approval_key().map(|k| k.path()),
            Some(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run".to_owned())
        );
        let k = AutostartLocation::HklmRun32.approval_key().expect("run32");
        assert!(k.machine);
        assert_eq!(k.subkey, "Run32");
        let k = AutostartLocation::UserStartupFolder
            .approval_key()
            .expect("folder");
        assert!(!k.machine);
        assert_eq!(k.subkey, "StartupFolder");
        assert!(
            AutostartLocation::CommonStartupFolder
                .approval_key()
                .expect("common")
                .machine
        );
        for loc in [
            AutostartLocation::HkcuRunOnce,
            AutostartLocation::HklmRunOnce,
            AutostartLocation::HklmRunOnce32,
            AutostartLocation::Service,
        ] {
            assert!(loc.approval_key().is_none(), "{loc:?}");
        }
        assert!(!AutostartLocation::HkcuRun.needs_admin());
        assert!(!AutostartLocation::UserStartupFolder.needs_admin());
        assert!(AutostartLocation::HklmRun.needs_admin());
        assert!(AutostartLocation::Service.needs_admin());
    }

    #[test]
    fn items_from_entries_and_services() {
        let entry = AutostartEntry {
            location: AutostartLocation::HkcuRun,
            name: "SignalRgb".into(),
            command: "x".into(),
            executable: None,
            approved: None,
        };
        let item = AutostartItem::from_entry(&entry);
        assert!(item.enabled && item.toggleable && !item.needs_admin && item.can_turn_off());
        let off = AutostartItem::from_entry(&AutostartEntry {
            approved: Some(false),
            ..entry.clone()
        });
        assert!(!off.enabled && off.toggleable && !off.can_turn_off());
        let once = AutostartItem::from_entry(&AutostartEntry {
            location: AutostartLocation::HklmRunOnce,
            ..entry
        });
        assert!(once.enabled && !once.toggleable && once.needs_admin);

        let auto = AutostartItem::service("OpenRGB", Some(StartType::Auto));
        assert!(auto.enabled && auto.toggleable && auto.needs_admin);
        let manual = AutostartItem::service("OpenRGB", Some(StartType::Manual));
        assert!(!manual.enabled && manual.toggleable);
        for st in [None, Some(StartType::Disabled), Some(StartType::Boot)] {
            assert!(!AutostartItem::service("x", st).toggleable, "{st:?}");
        }
    }

    /// Builds a shell link the way MS-SHLLINK lays it out: header, optional ID list, LinkInfo, string data.
    pub(crate) fn build_lnk(local_base: &str, args: Option<&str>, unicode_info: bool) -> Vec<u8> {
        let mut flags = LNK_HAS_ID_LIST | LNK_HAS_LINK_INFO | LNK_IS_UNICODE | LNK_HAS_NAME;
        if args.is_some() {
            flags |= LNK_HAS_ARGUMENTS;
        }
        let mut b = vec![0u8; LNK_HEADER_SIZE];
        b[0..4].copy_from_slice(&0x4Cu32.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&flags.to_le_bytes());
        // ID list: 4 bytes of opaque items.
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&[1, 2, 3, 4]);
        // LinkInfo.
        let header: u32 = if unicode_info { 0x24 } else { 0x1C };
        let mut info = vec![0u8; header as usize];
        let volume = vec![0x10u8, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0];
        let volume_off = info.len() as u32;
        info.extend_from_slice(&volume);
        let base_off = info.len() as u32;
        info.extend(local_base.bytes());
        info.push(0);
        let suffix_off = info.len() as u32;
        info.push(0);
        let (mut base_w_off, mut suffix_w_off) = (0u32, 0u32);
        if unicode_info {
            base_w_off = info.len() as u32;
            for u in local_base.encode_utf16() {
                info.extend_from_slice(&u.to_le_bytes());
            }
            info.extend_from_slice(&[0, 0]);
            suffix_w_off = info.len() as u32;
            info.extend_from_slice(&[0, 0]);
        }
        let size = info.len() as u32;
        info[0..4].copy_from_slice(&size.to_le_bytes());
        info[4..8].copy_from_slice(&header.to_le_bytes());
        info[8..12].copy_from_slice(&LINK_INFO_VOLUME_AND_LOCAL_PATH.to_le_bytes());
        info[12..16].copy_from_slice(&volume_off.to_le_bytes());
        info[16..20].copy_from_slice(&base_off.to_le_bytes());
        info[24..28].copy_from_slice(&suffix_off.to_le_bytes());
        if unicode_info {
            info[28..32].copy_from_slice(&base_w_off.to_le_bytes());
            info[32..36].copy_from_slice(&suffix_w_off.to_le_bytes());
        }
        b.extend_from_slice(&info);
        let mut push_string = |s: &str| {
            let units: Vec<u16> = s.encode_utf16().collect();
            b.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for u in units {
                b.extend_from_slice(&u.to_le_bytes());
            }
        };
        push_string("A comment");
        if let Some(a) = args {
            push_string(a);
        }
        b
    }

    #[test]
    fn shell_link_target_and_arguments() {
        let lnk = build_lnk(
            r"C:\Program Files\OpenRGB\OpenRGB.exe",
            Some("--startminimized"),
            false,
        );
        assert_eq!(
            parse_shell_link(&lnk),
            Some(ShellLink {
                target: Some(r"C:\Program Files\OpenRGB\OpenRGB.exe".into()),
                arguments: Some("--startminimized".into()),
            })
        );
        let lnk = build_lnk(r"C:\Tools\SignalRgbLauncher.exe", None, true);
        assert_eq!(
            parse_shell_link(&lnk).and_then(|l| l.target),
            Some(r"C:\Tools\SignalRgbLauncher.exe".into())
        );
    }

    #[test]
    fn malformed_shell_links_are_rejected_without_panicking() {
        assert_eq!(parse_shell_link(&[]), None);
        assert_eq!(parse_shell_link(b"not a link at all"), None);
        let lnk = build_lnk(r"C:\x.exe", Some("a"), true);
        for cut in 0..lnk.len() {
            let _ = parse_shell_link(&lnk[..cut]);
        }
        let mut bad = lnk.clone();
        // LinkInfo size far beyond the buffer.
        let info = LNK_HEADER_SIZE + 2 + 4;
        bad[info..info + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_shell_link(&bad), None);
    }
}
