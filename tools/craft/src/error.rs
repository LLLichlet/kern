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
    AnalysisContextParse {
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
    Execution(String),
    AnalysisContextValidation {
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

    pub fn exit_code(&self) -> i32 {
        const EXIT_FAILURE: i32 = 1;
        const EXIT_USAGE: i32 = 2;

        match self {
            Self::Usage(_) => EXIT_USAGE,
            _ => EXIT_FAILURE,
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Self::Usage(_) => Some(
                "run `craft help` to list commands, or `craft help <command>` for a specific command"
                    .to_string(),
            ),
            Self::ManifestNotFound { .. } => Some(
                "run `craft` inside a package directory, or pass `--project-path path/to/pkg`"
                    .to_string(),
            ),
            Self::ScriptValidation { path, .. } => match path.file_name().and_then(|name| name.to_str()) {
                Some("craft.rn") => Some(
                    "declare `pub fn craft(p: *mut plan.Plan) void` and import `craft.plan`"
                        .to_string(),
                ),
                Some("build.rn") => Some(
                    "declare `pub fn build(b: *mut builder.Builder) void` and import `craft.builder`"
                        .to_string(),
                ),
                _ => None,
            },
            Self::Validation { message, .. }
                if message.starts_with("release source policy rejected:") =>
            {
                Some(
                    "pin git sources with `rev` or `tag`, and prefer secure HTTPS URLs"
                        .to_string(),
                )
            }
            Self::Validation { message, .. }
                if message.starts_with("publish requires a current canonical `Craft.lock`") =>
            {
                Some("run `craft lock` before `craft publish`".to_string())
            }
            _ => None,
        }
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
                "could not find `Craft.toml` starting from `{}` or any parent directory",
                start.display()
            ),
            Self::ManifestParse { path, message } => {
                write!(f, "failed to parse `{}`: {}", path.display(), message)
            }
            Self::AnalysisContextParse { path, message } => {
                write!(
                    f,
                    "failed to parse analysis context `{}`: {}",
                    path.display(),
                    message
                )
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
                write!(f, "invalid craft script `{}`: {message}", path.display())
            }
            Self::Execution(message) => write!(f, "execution failed: {message}"),
            Self::AnalysisContextValidation { path, message } => {
                write!(
                    f,
                    "invalid analysis context `{}`: {message}",
                    path.display()
                )
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

#[cfg(test)]
mod tests {
    use super::Error;
    use std::path::PathBuf;

    #[test]
    fn usage_errors_return_usage_exit_code_and_help_hint() {
        let err = Error::Usage("bad args".to_string());
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.hint().as_deref(),
            Some(
                "run `craft help` to list commands, or `craft help <command>` for a specific command"
            )
        );
    }

    #[test]
    fn craft_script_validation_provides_entrypoint_hint() {
        let err = Error::ScriptValidation {
            path: PathBuf::from("craft.rn"),
            message: "missing required entry".to_string(),
        };
        assert_eq!(
            err.hint().as_deref(),
            Some("declare `pub fn craft(p: *mut plan.Plan) void` and import `craft.plan`")
        );
    }
}
