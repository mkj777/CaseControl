//! Small file helpers.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{ConfigError, Result};

/// `path` with `suffix` appended to the file name (`config.toml` -> `config.toml.tmp`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

/// Reads `path` as UTF-8 text through a bounded reader (`take`), so neither a large nor a growing file makes it read
/// more than `max` + 1 bytes. The bytes are read first and their length checked before they are decoded, so a file
/// larger than `max` always fails with [`ErrorKind::FileTooLarge`](std::io::ErrorKind::FileTooLarge), also when the
/// limit falls inside a multi-byte character; text that is not UTF-8 fails with `InvalidData`.
pub fn read_text_bounded(path: &Path, max: u64) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("{} is larger than {max} bytes", path.display()),
        ));
    }
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
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

/// [`write_atomic`], then the renamed file is opened again and flushed (`FlushFileBuffers` on Windows, `fsync`
/// elsewhere), so the new content is on disk when this returns `Ok`. Used for records that must survive a power loss
/// right after the write (device backups, ADR 0001 #42).
pub(crate) fn write_durable(path: &Path, contents: &str) -> Result<()> {
    write_atomic(path, contents)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| ConfigError::io(path, e))
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
        write_durable(&path, "three").expect("durable write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "three");
        assert!(write_durable(&path.join("x"), "y").is_err());
    }

    #[test]
    fn bounded_read_reports_too_large_before_decoding() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.toml");
        let kind = |max| read_text_bounded(&path, max).expect_err("fails").kind();
        // Six bytes ("\u{e9}" is two): with a limit of 4 the `max + 1` bytes read end inside the third character, which
        // `read_to_string` would report as `InvalidData`.
        std::fs::write(&path, "\u{e9}\u{e9}\u{e9}").expect("write");
        assert_eq!(kind(4), std::io::ErrorKind::FileTooLarge);
        assert_eq!(kind(5), std::io::ErrorKind::FileTooLarge);
        assert_eq!(read_text_bounded(&path, 6).expect("fits"), "\u{e9}\u{e9}\u{e9}");
        std::fs::write(&path, [0xFF_u8, 0xFE]).expect("write");
        assert_eq!(kind(2), std::io::ErrorKind::InvalidData);
        assert_eq!(
            read_text_bounded(&tmp.path().join("missing"), 2)
                .expect_err("missing")
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }
}
