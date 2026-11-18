//! Profiles on disk: one TOML file per profile in [`Paths::profiles_dir`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{ConfigError, Result};
use crate::fsutil::write_atomic;
use crate::paths::Paths;
use crate::profile::Profile;

/// Maximum length of a profile name and of a profile file stem, in characters.
pub const MAX_NAME_LEN: usize = 64;

/// File stems Windows reserves for devices; a profile file must not be called like this.
const RESERVED_STEMS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Reads and writes profiles.
///
/// A profile is identified by its `name` field, compared case-insensitively after trimming; the file name is only
/// derived from it (see [`file_stem_for`]) and never parsed back. Files that fail to parse are skipped with a warning
/// and never overwritten.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    dir: PathBuf,
}

/// A profile file found on disk.
struct Entry {
    path: PathBuf,
    profile: Profile,
}

impl ProfileStore {
    /// A store over `paths.profiles_dir`. Does no I/O; the directory is created on the first save.
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.profiles_dir.clone(),
        }
    }

    /// Names of all readable profiles, sorted case-insensitively.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut names: Vec<String> = self.scan()?.0.into_iter().map(|e| e.profile.name).collect();
        names.sort_by_cached_key(|n| (n.to_lowercase(), n.clone()));
        Ok(names)
    }

    /// Loads the profile called `name` (case-insensitive).
    pub fn load(&self, name: &str) -> Result<Profile> {
        self.find(name)
            .map(|e| e.profile)
            .ok_or_else(|| ConfigError::NotFound(name.trim().to_owned()))
    }

    /// Saves `profile`, replacing a profile with the same name (case-insensitive; the stored name takes the new
    /// spelling). The name is trimmed. A new profile gets a file name derived from its name, with a numeric suffix if
    /// that file name is already used by another profile.
    pub fn save(&self, profile: &Profile) -> Result<()> {
        let name = validate_name(&profile.name)?;
        let (entries, taken) = self.scan()?;
        let existing = entries.into_iter().find(|e| same_name(&e.profile.name, &name));
        let path = match existing {
            Some(e) => e.path,
            None => self.dir.join(format!("{}.toml", free_stem(&name, &taken, None))),
        };
        self.write(&path, profile, name)
    }

    /// Deletes the profile called `name` (case-insensitive).
    pub fn delete(&self, name: &str) -> Result<()> {
        let entry = self
            .find(name)
            .ok_or_else(|| ConfigError::NotFound(name.trim().to_owned()))?;
        std::fs::remove_file(&entry.path).map_err(|e| ConfigError::io(&entry.path, e))
    }

    /// Renames profile `old` to `new`. Fails with [`ConfigError::Exists`] if another profile already has the name
    /// `new`; changing only the case of a name is allowed. The file is renamed along with the profile.
    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        let new_name = validate_name(new)?;
        let (entries, taken) = self.scan()?;
        let mut source = None;
        for e in entries {
            if same_name(&e.profile.name, old) {
                source = Some(e);
            } else if same_name(&e.profile.name, &new_name) {
                return Err(ConfigError::Exists(new_name));
            }
        }
        let source = source.ok_or_else(|| ConfigError::NotFound(old.trim().to_owned()))?;
        let old_stem = stem_lower(&source.path);
        let stem = free_stem(&new_name, &taken, old_stem.as_deref());
        if Some(stem.to_lowercase()) == old_stem {
            // Same file (possibly differing in case only): rewrite it in place.
            return self.write(&source.path, &source.profile, new_name);
        }
        let target = self.dir.join(format!("{stem}.toml"));
        self.write(&target, &source.profile, new_name)?;
        std::fs::remove_file(&source.path).map_err(|e| ConfigError::io(&source.path, e))
    }

    fn write(&self, path: &Path, profile: &Profile, name: String) -> Result<()> {
        let profile = Profile {
            name,
            ..profile.clone()
        };
        let text = toml::to_string(&profile).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        write_atomic(path, &text)
    }

    fn find(&self, name: &str) -> Option<Entry> {
        match self.scan() {
            Ok((entries, _)) => entries.into_iter().find(|e| same_name(&e.profile.name, name)),
            Err(err) => {
                tracing::warn!(error = %err, "cannot read profiles directory");
                None
            }
        }
    }

    /// All readable profiles, plus the lower-cased stems of every `.toml` file (readable or not).
    fn scan(&self) -> Result<(Vec<Entry>, HashSet<String>)> {
        let mut entries = Vec::new();
        let mut taken = HashSet::new();
        let dir = match std::fs::read_dir(&self.dir) {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((entries, taken)),
            Err(e) => return Err(ConfigError::io(&self.dir, e)),
        };
        for item in dir {
            let path = item.map_err(|e| ConfigError::io(&self.dir, e))?.path();
            let is_toml = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
            if !is_toml || !path.is_file() {
                continue;
            }
            if let Some(stem) = stem_lower(&path) {
                taken.insert(stem);
            }
            match read_profile(&path) {
                Ok(profile) => entries.push(Entry { path, profile }),
                Err(err) => tracing::warn!(error = %err, "skipping unreadable profile file"),
            }
        }
        Ok((entries, taken))
    }
}

fn read_profile(path: &Path) -> Result<Profile> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::io(path, e))?;
    let profile: Profile = toml::from_str(&text).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    if profile.name.trim().is_empty() {
        return Err(ConfigError::Parse {
            path: path.to_path_buf(),
            message: "empty profile name".into(),
        });
    }
    Ok(profile)
}

fn stem_lower(path: &Path) -> Option<String> {
    path.file_stem().map(|s| s.to_string_lossy().to_lowercase())
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Checks a profile name and returns it trimmed: not empty, at most [`MAX_NAME_LEN`] characters, no control
/// characters.
pub fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    let reason = if trimmed.is_empty() {
        Some("name is empty")
    } else if trimmed.chars().count() > MAX_NAME_LEN {
        Some("name is longer than 64 characters")
    } else if trimmed.chars().any(char::is_control) {
        Some("name contains control characters")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ConfigError::InvalidName {
            name: name.to_owned(),
            reason,
        }),
        None => Ok(trimmed.to_owned()),
    }
}

/// The file stem a profile called `name` gets when no collision exists: `[A-Za-z0-9 _-]` kept, every other character
/// replaced with `_`, cut to [`MAX_NAME_LEN`] characters, trailing spaces removed (Windows drops them), and a `_`
/// appended to names Windows reserves for devices (`CON`, `NUL`, `COM1`, ...).
pub fn file_stem_for(name: &str) -> String {
    let mut stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_NAME_LEN)
        .collect();
    stem.truncate(stem.trim_end().len());
    if stem.is_empty() {
        stem.push('_');
    }
    if RESERVED_STEMS.iter().any(|r| r.eq_ignore_ascii_case(&stem)) {
        if stem.len() == MAX_NAME_LEN {
            stem.pop();
        }
        stem.push('_');
    }
    stem
}

/// [`file_stem_for`] `name`, made unique against `taken` (lower-cased stems) by a `-2`, `-3`, ... suffix. `own` is
/// the lower-cased stem of the file being renamed, which counts as free.
fn free_stem(name: &str, taken: &HashSet<String>, own: Option<&str>) -> String {
    let base = file_stem_for(name);
    let is_free = |s: &str| {
        let lower = s.to_lowercase();
        Some(lower.as_str()) == own || !taken.contains(&lower)
    };
    if is_free(&base) {
        return base;
    }
    (2u32..)
        .map(|n| {
            let suffix = format!("-{n}");
            // The stem is ASCII, so byte length equals character count.
            let keep = base.len().min(MAX_NAME_LEN - suffix.len());
            format!("{}{suffix}", base[..keep].trim_end())
        })
        .find(|s| is_free(s))
        .expect("an unbounded suffix range always finds a free stem")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cc_core::{DeviceId, Rgb};

    use super::*;
    use crate::profile::DeviceSetting;

    fn store() -> (tempfile::TempDir, ProfileStore, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(tmp.path());
        let dir = paths.profiles_dir.clone();
        (tmp, ProfileStore::new(&paths), dir)
    }

    fn profile(name: &str) -> Profile {
        let mut devices = BTreeMap::new();
        devices.insert(
            DeviceId::hid(0x31E3, 0x1310, None),
            DeviceSetting::Direct {
                colors: vec![Rgb::new(1, 2, 3)],
            },
        );
        Profile {
            name: name.into(),
            devices,
            brightness: BTreeMap::new(),
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn sanitizes_file_stems() {
        assert_eq!(file_stem_for("Evening"), "Evening");
        assert_eq!(file_stem_for("  Work - Day_1 "), "Work - Day_1");
        assert_eq!(file_stem_for("a/b\\c:d*e?.f"), "a_b_c_d_e__f");
        assert_eq!(file_stem_for("Grün"), "Gr_n");
        assert_eq!(file_stem_for("..."), "___");
        assert_eq!(file_stem_for("con"), "con_");
        assert_eq!(file_stem_for("LPT1"), "LPT1_");
        assert_eq!(file_stem_for(&"x".repeat(80)), "x".repeat(64));
        assert_eq!(file_stem_for(&format!("{}  y", "x".repeat(63))), "x".repeat(63));
    }

    #[test]
    fn validates_names() {
        assert_eq!(validate_name("  Night ").expect("valid"), "Night");
        assert!(validate_name(&"é".repeat(64)).is_ok());
        for bad in ["", "   ", "a\nb", &"x".repeat(65)] {
            assert!(
                matches!(validate_name(bad), Err(ConfigError::InvalidName { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn empty_store() {
        let (_tmp, store, _) = store();
        assert!(store.list().expect("list").is_empty());
        assert!(matches!(store.load("x"), Err(ConfigError::NotFound(_))));
        assert!(matches!(store.delete("x"), Err(ConfigError::NotFound(_))));
        assert!(matches!(store.rename("x", "y"), Err(ConfigError::NotFound(_))));
    }

    #[test]
    fn crud() {
        let (_tmp, store, dir) = store();
        store.save(&profile("  Evening ")).expect("save");
        store.save(&profile("work")).expect("save");
        store.save(&profile("Alpha")).expect("save");
        assert_eq!(store.list().expect("list"), ["Alpha", "Evening", "work"]);
        assert_eq!(files(&dir), ["Alpha.toml", "Evening.toml", "work.toml"]);

        let loaded = store.load("EVENING").expect("load is case-insensitive");
        assert_eq!(loaded, profile("Evening"));

        // Saving under the same name (any case) replaces the profile in its file.
        let mut changed = profile("evening");
        changed.devices.clear();
        store.save(&changed).expect("overwrite");
        assert_eq!(files(&dir), ["Alpha.toml", "Evening.toml", "work.toml"]);
        assert_eq!(store.load("Evening").expect("load"), changed);

        store.delete("ALPHA").expect("delete");
        assert_eq!(store.list().expect("list"), ["evening", "work"]);
        assert!(matches!(store.load("Alpha"), Err(ConfigError::NotFound(_))));
        assert!(matches!(
            store.save(&profile("  ")),
            Err(ConfigError::InvalidName { .. })
        ));
    }

    #[test]
    fn colliding_file_names_get_suffixes() {
        let (_tmp, store, dir) = store();
        store.save(&profile("a/b")).expect("save");
        store.save(&profile("a:b")).expect("save");
        store.save(&profile("a?b")).expect("save");
        assert_eq!(files(&dir), ["a_b-2.toml", "a_b-3.toml", "a_b.toml"]);
        assert_eq!(store.list().expect("list"), ["a/b", "a:b", "a?b"]);
        assert_eq!(store.load("a:b").expect("load").name, "a:b");

        // 64-character names: the suffix replaces the end of the stem, which stays at 64 characters.
        let long_a = format!("{}/", "x".repeat(63));
        let long_b = format!("{}:", "x".repeat(63));
        store.save(&profile(&long_a)).expect("save");
        store.save(&profile(&long_b)).expect("save");
        assert!(dir.join(format!("{}_.toml", "x".repeat(63))).is_file());
        assert!(dir.join(format!("{}-2.toml", "x".repeat(62))).is_file());
        assert_eq!(store.load(&long_b).expect("load").name, long_b);
    }

    #[test]
    fn unreadable_files_are_skipped_and_never_overwritten() {
        let (_tmp, store, dir) = store();
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("Broken.toml"), "name = = 1").expect("write");
        std::fs::write(dir.join("notes.txt"), "not a profile").expect("write");
        assert!(store.list().expect("list").is_empty());
        store.save(&profile("Broken")).expect("save");
        assert_eq!(
            std::fs::read_to_string(dir.join("Broken.toml")).expect("read"),
            "name = = 1"
        );
        assert!(dir.join("Broken-2.toml").is_file());
        assert_eq!(store.list().expect("list"), ["Broken"]);
    }

    #[test]
    fn rename() {
        let (_tmp, store, dir) = store();
        store.save(&profile("Day")).expect("save");
        store.save(&profile("Night")).expect("save");

        assert!(matches!(
            store.rename("Day", " NIGHT "),
            Err(ConfigError::Exists(_))
        ));
        assert!(matches!(
            store.rename("Day", ""),
            Err(ConfigError::InvalidName { .. })
        ));

        store.rename("day", "Morning").expect("rename");
        assert_eq!(files(&dir), ["Morning.toml", "Night.toml"]);
        assert_eq!(store.load("Morning").expect("load"), profile("Morning"));

        // Case-only rename keeps the file.
        store.rename("night", "NIGHT").expect("case rename");
        assert_eq!(store.list().expect("list"), ["Morning", "NIGHT"]);
        assert_eq!(files(&dir).len(), 2);

        // Renaming to a name whose file stem is taken by the renamed profile itself reuses the file.
        store.save(&profile("a/b")).expect("save");
        store.rename("a/b", "a:b").expect("rename");
        assert!(files(&dir).contains(&"a_b.toml".to_owned()));
        assert_eq!(store.load("a:b").expect("load").name, "a:b");
        assert_eq!(files(&dir).len(), 3);
    }
}
