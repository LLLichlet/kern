use std::error::Error as StdError;
use std::fmt::{Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Usage(String),
    Io {
        path: Option<PathBuf>,
        source: io::Error,
    },
    ManifestNotFound {
        start: PathBuf,
    },
    ManifestParse {
        path: PathBuf,
        message: String,
    },
    LockfileParse {
        path: PathBuf,
        message: String,
    },
    Validation {
        path: PathBuf,
        message: String,
    },
    ScriptValidation {
        path: PathBuf,
        message: String,
    },
    LockfileValidation {
        path: PathBuf,
        message: String,
    },
}

impl Error {
    pub fn from_io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: Some(path.to_path_buf()),
            source,
        }
    }

    pub fn from_io_plain(source: io::Error) -> Self {
        Self::Io { path: None, source }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "{msg}"),
            Self::Io { path, source } => {
                if let Some(path) = path {
                    write!(f, "I/O error at `{}`: {source}", path.display())
                } else {
                    write!(f, "I/O error: {source}")
                }
            }
            Self::ManifestNotFound { start } => write!(
                f,
                "could not find `Kraft.toml` starting from `{}` or any parent directory",
                start.display()
            ),
            Self::ManifestParse { path, message } => {
                write!(f, "failed to parse `{}`: {}", path.display(), message)
            }
            Self::LockfileParse { path, message } => {
                write!(
                    f,
                    "failed to parse lockfile `{}`: {}",
                    path.display(),
                    message
                )
            }
            Self::Validation { path, message } => {
                write!(f, "invalid manifest `{}`: {message}", path.display())
            }
            Self::ScriptValidation { path, message } => {
                write!(f, "invalid kraft script `{}`: {message}", path.display())
            }
            Self::LockfileValidation { path, message } => {
                write!(f, "invalid lockfile `{}`: {message}", path.display())
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
