//! Error type of this crate.

use std::path::PathBuf;

/// Everything that can go wrong while reading or writing settings and profiles.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file system operation on `path` failed.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// The file or directory the operation was about.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A file is not valid TOML or does not match the expected shape.
    #[error("cannot parse {path}: {message}")]
    Parse {
        /// The offending file.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// A value could not be serialized to TOML (should not happen for the types of this crate).
    #[error("cannot serialize to TOML: {0}")]
    Serialize(String),
    /// A profile name is empty, too long or contains control characters.
    #[error("invalid profile name {name:?}: {reason}")]
    InvalidName {
        /// The rejected name (as given).
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// A profile with this name (compared case-insensitively) already exists.
    #[error("a profile named {0:?} already exists")]
    Exists(String),
    /// No profile with this name exists.
    #[error("no profile named {0:?}")]
    NotFound(String),
}

impl ConfigError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Result alias of this crate.
pub type Result<T> = std::result::Result<T, ConfigError>;
