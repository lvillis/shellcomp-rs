use std::fmt::{Display, Formatter};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// Supported shells for completion installation and detection.
///
/// Bash, Zsh, and Fish are the current production support set. Other variants remain available so
/// callers can branch on a stable API surface while unsupported shells return structured errors.
pub enum Shell {
    /// GNU Bash.
    Bash,
    /// Z shell.
    Zsh,
    /// Fish shell.
    Fish,
    /// Reserved for future implementation.
    Elvish,
    /// Reserved for future implementation.
    Powershell,
    /// An escape hatch for shells not modelled explicitly yet.
    ///
    /// This variant exists so callers can keep shell selection in their own domain model even when
    /// `shellcomp` does not implement that shell yet.
    Other(String),
}

impl Display for Shell {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bash => write!(f, "bash"),
            Self::Zsh => write!(f, "zsh"),
            Self::Fish => write!(f, "fish"),
            Self::Elvish => write!(f, "elvish"),
            Self::Powershell => write!(f, "powershell"),
            Self::Other(value) => write!(f, "{value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of a file mutation.
///
/// This enum is used across install, uninstall, and managed shell profile updates so callers can
/// report exactly what changed.
pub enum FileChange {
    /// The target did not exist and was created.
    Created,
    /// The target existed and its content changed.
    Updated,
    /// The target already matched the requested content.
    Unchanged,
    /// The target existed and was removed.
    Removed,
    /// The target did not exist, so nothing changed.
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// How a completion becomes active for a shell.
///
/// This describes the wiring mechanism rather than the current state. Pair it with
/// [`Availability`] to understand whether the completion is already usable.
pub enum ActivationMode {
    /// Activation relies on a system-level shell completion loader.
    SystemLoader,
    /// Activation is managed through a controlled shell startup block.
    ManagedRcBlock,
    /// Activation relies on the shell's native completion directory.
    NativeDirectory,
    /// Activation must be wired manually by the caller or user.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Availability state for an installed completion.
///
/// This represents the best state `shellcomp` can determine from the current environment and
/// managed files.
pub enum Availability {
    /// The completion should be usable immediately.
    ActiveNow,
    /// The completion should be available after opening a new shell.
    AvailableAfterNewShell,
    /// The completion should be available after sourcing the affected startup file.
    AvailableAfterSource,
    /// Manual action is still required before the completion can work.
    ManualActionRequired,
    /// The library could not determine current availability.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// High-level operation associated with a structured failure.
pub enum Operation {
    /// Install a completion script and its activation wiring.
    Install,
    /// Detect activation status for a completion.
    DetectActivation,
    /// Remove a completion script and its activation wiring.
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Stable failure kinds for recoverable operational errors.
///
/// [`crate::Error::Failure`] wraps a [`FailureReport`] carrying one of these kinds so callers can
/// branch on failure categories without parsing human-readable text.
pub enum FailureKind {
    /// `HOME` was not available and no fallback path could be derived.
    MissingHome,
    /// The requested shell is not implemented.
    UnsupportedShell,
    /// The requested target path was invalid.
    ///
    /// This typically means a provided `path_override` did not include a usable parent directory.
    InvalidTargetPath,
    /// The default managed install path could not be resolved.
    DefaultPathUnavailable,
    /// The completion file or its directory could not be created or written.
    CompletionTargetUnavailable,
    /// The completion file existed but could not be read as expected.
    CompletionFileUnreadable,
    /// The managed shell profile could not be written or removed.
    ProfileUnavailable,
    /// The managed shell profile was present but malformed.
    ///
    /// This usually means a previously managed block has a missing end marker or otherwise cannot
    /// be updated safely.
    ProfileCorrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Input for a completion install operation.
///
/// Use [`path_override`](InstallRequest::path_override) when you want to control the destination
/// explicitly and handle shell activation yourself.
pub struct InstallRequest<'a> {
    /// Target shell.
    pub shell: Shell,
    /// Binary name used by the completion script.
    ///
    /// The name must be non-empty and use a portable ASCII character set consisting of letters,
    /// digits, `.`, `_`, and `-`.
    pub program_name: &'a str,
    /// Completion script bytes to install.
    pub script: &'a [u8],
    /// Optional custom install path. When set, activation is reported as manual.
    ///
    /// `shellcomp` will still write the file idempotently, but it will not attempt to manage shell
    /// startup wiring for a custom location.
    pub path_override: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Input for a completion uninstall operation.
///
/// Passing [`path_override`](UninstallRequest::path_override) removes only that explicit file and
/// skips managed shell profile cleanup.
pub struct UninstallRequest<'a> {
    /// Target shell.
    pub shell: Shell,
    /// Binary name used by the completion script.
    ///
    /// The same validation rules as [`InstallRequest::program_name`] apply.
    pub program_name: &'a str,
    /// Optional custom install path to remove instead of the default managed path.
    ///
    /// This is mainly useful when a caller previously installed with
    /// [`InstallRequest::path_override`].
    pub path_override: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structured result of a completion install operation.
///
/// The report is designed for caller-side rendering. `shellcomp` does not print user-facing output
/// directly.
pub struct InstallReport {
    /// Target shell.
    pub shell: Shell,
    /// Final script path that was written.
    pub target_path: PathBuf,
    /// Outcome of writing the completion file.
    ///
    /// This only describes the completion script file itself. Startup wiring state is described by
    /// [`InstallReport::activation`].
    pub file_change: FileChange,
    /// Activation details for the installed completion.
    pub activation: ActivationReport,
    /// Files touched or referenced while completing the operation.
    ///
    /// This includes [`InstallReport::target_path`] and may also include a managed shell startup
    /// file such as `~/.bashrc` or `~/.zshrc`.
    pub affected_locations: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structured cleanup status for activation wiring removal.
///
/// This is returned as part of [`RemoveReport`] and also embedded in [`FailureReport`] when an
/// uninstall operation fails after partial cleanup work.
pub struct CleanupReport {
    /// Activation mechanism associated with the cleanup.
    pub mode: ActivationMode,
    /// Outcome of the cleanup operation.
    pub change: FileChange,
    /// Shell-specific location involved in cleanup.
    ///
    /// For managed Bash/Zsh cleanup this is typically the startup file path. For Fish or manual
    /// custom-path installs, it is often `None`.
    pub location: Option<PathBuf>,
    /// Human-readable cleanup context.
    pub reason: Option<String>,
    /// Suggested next step when cleanup could not be completed automatically.
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structured result of a completion uninstall operation.
///
/// File removal and activation cleanup are reported separately so callers can preserve partial
/// progress in their own UX or logs.
pub struct RemoveReport {
    /// Target shell.
    pub shell: Shell,
    /// Final script path that was removed or checked.
    pub target_path: PathBuf,
    /// Outcome of removing the completion file.
    pub file_change: FileChange,
    /// Structured activation cleanup result.
    pub cleanup: CleanupReport,
    /// Files touched or referenced while completing the operation.
    ///
    /// This always includes [`RemoveReport::target_path`] and may also include a managed startup
    /// file when shell wiring cleanup was attempted.
    pub affected_locations: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structured activation status for an installed completion.
///
/// Use this report to decide whether to tell the user "ready now", "open a new shell", "source
/// your rc file", or "finish setup manually".
///
/// # Examples
///
/// ```rust
/// use shellcomp::{ActivationMode, Availability};
///
/// let mode = ActivationMode::ManagedRcBlock;
/// let availability = Availability::AvailableAfterSource;
///
/// assert_eq!(mode, ActivationMode::ManagedRcBlock);
/// assert_eq!(availability, Availability::AvailableAfterSource);
/// ```
pub struct ActivationReport {
    /// Activation mechanism in use.
    pub mode: ActivationMode,
    /// Current or expected availability state.
    ///
    /// Interpret this together with [`ActivationReport::mode`]. For example,
    /// [`ActivationMode::ManagedRcBlock`] plus [`Availability::AvailableAfterSource`] means the
    /// completion wiring is present but the current shell session may still need `source ~/.bashrc`
    /// or `source ~/.zshrc`.
    pub availability: Availability,
    /// Shell-specific location related to activation.
    ///
    /// For system-loader or native-directory activation this is often the completion file path.
    /// For managed startup wiring it is typically the startup file path.
    pub location: Option<PathBuf>,
    /// Machine-readable operations use enums; this field carries human-readable context.
    pub reason: Option<String>,
    /// Suggested next step for callers to render directly or adapt.
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Structured failure report for recoverable operational errors.
///
/// `shellcomp` uses this report to preserve partial state like completion file changes or manual
/// recovery instructions. It is wrapped by [`crate::Error::Failure`].
///
/// # Examples
///
/// ```rust
/// use std::path::PathBuf;
///
/// use shellcomp::{Error, FailureKind, InstallRequest, Shell, install};
///
/// let error = install(InstallRequest {
///     shell: Shell::Fish,
///     program_name: "demo",
///     script: b"complete -c demo\n",
///     path_override: Some(PathBuf::from("/")),
/// })
/// .expect_err("path without parent should fail");
///
/// match error {
///     Error::Failure(report) => {
///         assert_eq!(report.kind, FailureKind::InvalidTargetPath);
///         assert!(report.reason.contains("does not have a parent directory"));
///     }
///     other => panic!("unexpected error: {other}"),
/// }
/// ```
pub struct FailureReport {
    /// Operation that failed.
    pub operation: Operation,
    /// Target shell.
    pub shell: Shell,
    /// Path involved in the failure when known.
    ///
    /// This is usually the completion file target path, not necessarily the shell startup file that
    /// caused the failure.
    pub target_path: Option<PathBuf>,
    /// Related locations inspected or mutated before the failure.
    ///
    /// This may include both the completion target path and shell-specific locations such as
    /// `~/.bashrc` or `~/.zshrc`.
    pub affected_locations: Vec<PathBuf>,
    /// Stable failure kind for programmatic branching.
    pub kind: FailureKind,
    /// Outcome of the completion file change before the failure occurred, when available.
    ///
    /// This is especially useful during install failures where the completion file may already have
    /// been written successfully before shell activation wiring failed.
    pub file_change: Option<FileChange>,
    /// Activation state or fallback guidance known at failure time, when available.
    ///
    /// For example, a failed Bash/Zsh install may still return a manual activation recommendation.
    pub activation: Option<ActivationReport>,
    /// Cleanup state known at failure time, when available.
    pub cleanup: Option<CleanupReport>,
    /// Human-readable failure reason.
    pub reason: String,
    /// Suggested next step for recovery or manual completion.
    pub next_step: Option<String>,
}
