use std::error::Error as StdError;
use std::fmt::{Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};

use crate::Shell;
use crate::model::FailureReport;

/// Convenience result type used by all public `shellcomp` APIs.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
/// Errors returned by `shellcomp` operations.
///
/// Recoverable operational failures that callers are expected to render are wrapped as
/// [`Error::Failure`] with a [`crate::FailureReport`]. Lower-level variants represent immediate
/// input validation or filesystem problems.
pub enum Error {
    /// The requested program name was empty.
    EmptyProgramName,
    /// The requested program name contains unsupported path separators or reserved values.
    InvalidProgramName {
        /// The rejected program name.
        program_name: String,
    },
    /// `HOME` could not be resolved for an operation that requires it.
    MissingHome,
    /// The requested shell is known to the API but not implemented yet.
    UnsupportedShell(Shell),
    /// A target path did not contain a parent directory.
    PathHasNoParent {
        /// The invalid path.
        path: PathBuf,
    },
    /// A path could not be represented as UTF-8 for shell wiring purposes.
    NonUtf8Path {
        /// The path that could not be encoded.
        path: PathBuf,
    },
    /// A managed text file contained invalid UTF-8.
    InvalidUtf8File {
        /// The unreadable file path.
        path: PathBuf,
    },
    /// A managed block start marker was found without its matching end marker.
    ManagedBlockMissingEnd {
        /// The file containing the broken managed block.
        path: PathBuf,
        /// The expected start marker.
        start_marker: String,
        /// The expected end marker.
        end_marker: String,
    },
    /// A structured recoverable failure report intended for callers.
    ///
    /// Match this variant when you need stable failure kinds, affected paths, or caller-facing
    /// recovery guidance without parsing a display string.
    Failure(Box<FailureReport>),
    /// A filesystem operation failed.
    ///
    /// Most user-facing operational I/O failures are mapped to [`Error::Failure`] by the public
    /// APIs. This variant is still exposed because low-level helpers and validation paths may
    /// surface it directly.
    Io {
        /// The operation being attempted.
        action: &'static str,
        /// The path involved in the operation.
        path: PathBuf,
        /// The underlying I/O error.
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn failure(report: FailureReport) -> Self {
        Self::Failure(Box::new(report))
    }

    /// Returns the most relevant filesystem location for this error, when one exists.
    ///
    /// For [`Error::Failure`], this returns the report's primary `target_path`. Use
    /// [`crate::FailureReport::affected_locations`] when you need the full set of related paths.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use shellcomp::Error;
    ///
    /// let error = Error::InvalidProgramName {
    ///     program_name: "bad/name".to_owned(),
    /// };
    ///
    /// assert_eq!(error.location(), None);
    /// ```
    pub fn location(&self) -> Option<&Path> {
        match self {
            Self::PathHasNoParent { path }
            | Self::NonUtf8Path { path }
            | Self::InvalidUtf8File { path }
            | Self::Io { path, .. } => Some(path.as_path()),
            Self::ManagedBlockMissingEnd { path, .. } => Some(path.as_path()),
            Self::Failure(report) => report.target_path.as_deref(),
            Self::EmptyProgramName
            | Self::InvalidProgramName { .. }
            | Self::MissingHome
            | Self::UnsupportedShell(_) => None,
        }
    }

    /// Returns a human-readable failure reason intended for caller-side rendering.
    ///
    /// This is suitable for logs or CLI output, but callers that need stable branching should
    /// prefer matching [`Error::Failure`] and reading [`crate::FailureReport::kind`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use shellcomp::Error;
    ///
    /// let error = Error::MissingHome;
    /// assert!(error.reason().is_some());
    /// ```
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Failure(report) => Some(report.reason.as_str()),
            Self::EmptyProgramName => Some("Program name must not be empty."),
            Self::InvalidProgramName { .. } => Some(
                "Program names must use a safe portable character set: ASCII letters, digits, `.`, `_`, and `-`.",
            ),
            Self::MissingHome => Some(
                "The operation requires HOME because the default shell-managed path could not be resolved.",
            ),
            Self::UnsupportedShell(_) => Some(
                "This shell is modelled in the API but not implemented in the production support set yet.",
            ),
            Self::PathHasNoParent { .. } => {
                Some("The provided path does not have a parent directory.")
            }
            Self::NonUtf8Path { .. } => Some(
                "The path cannot be represented safely in shell startup wiring because it is not valid UTF-8.",
            ),
            Self::InvalidUtf8File { .. } => Some(
                "The managed file could not be parsed as UTF-8, so shellcomp cannot safely update it.",
            ),
            Self::ManagedBlockMissingEnd { .. } => {
                Some("A managed shell block is malformed because its closing marker is missing.")
            }
            Self::Io { .. } => None,
        }
    }

    /// Returns a suggested next step for this error, when one exists.
    ///
    /// This is primarily intended for CLI or UI layers that want to surface actionable guidance
    /// without inventing shell-specific recovery text themselves.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use shellcomp::Error;
    ///
    /// let error = Error::PathHasNoParent {
    ///     path: "/".into(),
    /// };
    ///
    /// assert!(error.next_step().is_some());
    /// ```
    pub fn next_step(&self) -> Option<&str> {
        match self {
            Self::Failure(report) => report.next_step.as_deref(),
            Self::MissingHome => Some(
                "Set HOME for the current process or pass `path_override` so the library does not need a default managed path.",
            ),
            Self::PathHasNoParent { .. } => Some(
                "Pass a file path with a real parent directory, or create the parent directory before calling shellcomp.",
            ),
            Self::InvalidProgramName { .. } => Some(
                "Rename the binary or pass a sanitized program name that only uses ASCII letters, digits, `.`, `_`, and `-`.",
            ),
            _ => None,
        }
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyProgramName => write!(f, "program name must not be empty"),
            Self::InvalidProgramName { program_name } => {
                write!(
                    f,
                    "program name `{program_name}` contains unsupported characters"
                )
            }
            Self::MissingHome => write!(f, "HOME is not set and no fallback path can be resolved"),
            Self::UnsupportedShell(shell) => write!(f, "shell `{shell}` is not supported yet"),
            Self::PathHasNoParent { path } => {
                write!(
                    f,
                    "path `{}` does not have a parent directory",
                    path.display()
                )
            }
            Self::NonUtf8Path { path } => {
                write!(
                    f,
                    "path `{}` cannot be represented as UTF-8",
                    path.display()
                )
            }
            Self::InvalidUtf8File { path } => {
                write!(f, "file `{}` is not valid UTF-8", path.display())
            }
            Self::ManagedBlockMissingEnd {
                path,
                start_marker,
                end_marker,
            } => write!(
                f,
                "managed block `{start_marker}` in `{}` is missing closing marker `{end_marker}`",
                path.display()
            ),
            Self::Failure(report) => write!(f, "{}", report.reason),
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "failed to {action} `{}`: {source}", path.display()),
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
    use std::path::PathBuf;

    use super::Error;
    use crate::model::{
        ActivationMode, ActivationReport, Availability, FailureKind, FailureReport, Operation,
        Shell,
    };

    #[test]
    fn failure_helpers_forward_report_context() {
        let error = Error::failure(FailureReport {
            operation: Operation::Install,
            shell: Shell::Bash,
            target_path: Some(PathBuf::from("/tmp/tool")),
            affected_locations: vec![PathBuf::from("/tmp/tool"), PathBuf::from("/tmp/.bashrc")],
            kind: FailureKind::ProfileUnavailable,
            file_change: Some(crate::FileChange::Created),
            activation: Some(ActivationReport {
                mode: ActivationMode::Manual,
                availability: Availability::ManualActionRequired,
                location: Some(PathBuf::from("/tmp/.bashrc")),
                reason: Some("profile update failed".to_owned()),
                next_step: Some("edit your shell profile manually".to_owned()),
            }),
            cleanup: None,
            reason: "Could not update the managed Bash startup block.".to_owned(),
            next_step: Some("edit your shell profile manually".to_owned()),
        });

        assert_eq!(error.location(), Some(PathBuf::from("/tmp/tool").as_path()));
        assert_eq!(
            error.reason(),
            Some("Could not update the managed Bash startup block.")
        );
        assert_eq!(error.next_step(), Some("edit your shell profile manually"));
    }

    #[test]
    fn builtin_error_helpers_return_actionable_context() {
        let error = Error::InvalidProgramName {
            program_name: "bad/name".to_owned(),
        };

        assert_eq!(
            error.reason(),
            Some(
                "Program names must use a safe portable character set: ASCII letters, digits, `.`, `_`, and `-`."
            )
        );
        assert_eq!(
            error.next_step(),
            Some(
                "Rename the binary or pass a sanitized program name that only uses ASCII letters, digits, `.`, `_`, and `-`."
            )
        );
        assert_eq!(error.location(), None);
    }
}
