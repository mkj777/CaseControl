//! Small file helpers.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{ConfigError, Result};

/// `path` with `suffix` appended to the file name (`config.toml` -> `config.toml.tmp`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

/// Writes `contents` to `<path>.tmp`, flushes it to disk and renames it over `path`, so a reader never sees a
/// half-written file. Creates the parent directory if needed.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }
    let tmp = with_suffix(path, ".tmp");
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(contents.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(ConfigError::io(tmp, e));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ConfigError::io(path, e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces_without_leftovers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("sub").join("a.toml");
        write_atomic(&path, "one").expect("first write");
        write_atomic(&path, "two").expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "two");
        assert!(!with_suffix(&path, ".tmp").exists());
    }
}
