//! `shellcomp` is a deployment layer for shell completion scripts in Rust CLI projects.
//!
//! It does **not** generate completions. Instead, it focuses on the operational work around
//! completions:
//!
//! - choosing managed default install paths,
//! - writing completion files idempotently,
//! - wiring shell startup files when necessary,
//! - detecting current activation state,
//! - removing managed files and managed startup blocks,
//! - returning structured reports and structured failures for caller-side rendering.
//!
//! Pair it with `clap_complete` or any other generator when you need to render the script bytes.
//!
//! # Scope And Non-Goals
//!
//! `shellcomp` intentionally keeps a narrow scope:
//!
//! - it is **not** a shell completion generator;
//! - it is **not** a generic shell configuration manager;
//! - it does **not** attempt to understand arbitrary user shell customizations;
//! - it only modifies completion files it was asked to manage and startup blocks it can identify
//!   with stable markers.
//!
//! This is why the crate can stay small while still being production-usable.
//!
//! # Supported Shells
//!
//! Production support currently covers [`Shell::Bash`], [`Shell::Zsh`], and [`Shell::Fish`].
//! Other shells remain modelled in the API so callers can branch on a stable type, but unsupported
//! shells return [`Error::UnsupportedShell`].
//!
//! Managed behavior for the supported shells:
//!
//! - [`Shell::Bash`]: writes to the XDG data completion directory, then prefers a system
//!   `bash-completion` loader and falls back to a managed `~/.bashrc` block when no loader is
//!   detected.
//! - [`Shell::Zsh`]: writes `_binary-name` into the managed zsh completion directory and maintains
//!   a managed `~/.zshrc` block that updates `fpath` and runs `compinit -i` when needed.
//! - [`Shell::Fish`]: writes directly into Fish's native completions directory and does not manage
//!   a shell startup file.
//!
//! # Public API
//!
//! The crate is intentionally small:
//!
//! - [`default_install_path`] resolves the managed target path for a shell and binary name.
//! - [`install`] writes the completion file and returns an [`InstallReport`].
//! - [`detect_activation`] inspects the managed setup and returns an [`ActivationReport`].
//! - [`uninstall`] removes the managed file and returns a [`RemoveReport`].
//! - [`render_clap_completion`] is available behind the `clap` feature for users who want the
//!   crate to render completion bytes from `clap::CommandFactory`.
//!
//! # Reading Reports
//!
//! The public report types are designed so callers can render UX without parsing display strings:
//!
//! - [`InstallReport::file_change`] tells you whether the completion file was created, updated, or
//!   already matched.
//! - [`ActivationReport::mode`] tells you *how* the shell will load the completion.
//! - [`ActivationReport::availability`] tells you whether it is active now, available after a new
//!   shell, available after sourcing a file, or still requires manual work.
//! - [`RemoveReport::cleanup`] separates startup wiring cleanup from completion file removal so a
//!   caller can preserve partial progress.
//! - [`FailureReport`] carries structured failure kind, relevant paths, and suggested next steps.
//!
//! # Typical Integration Flow
//!
//! 1. Render a completion script with your preferred generator.
//! 2. Call [`install`] to place the script into a managed location.
//! 3. Show the returned [`ActivationReport`] to the user.
//! 4. Optionally call [`detect_activation`] later to re-check availability.
//! 5. Call [`uninstall`] to remove both the completion file and any managed activation wiring.
//!
//! # Install Example
//!
//! ```no_run
//! use shellcomp::{InstallRequest, Shell, install};
//!
//! let script = b"complete -F _demo demo\n";
//! let report = install(InstallRequest {
//!     shell: Shell::Bash,
//!     program_name: "demo",
//!     script,
//!     path_override: None,
//! })?;
//!
//! println!("installed to {}", report.target_path.display());
//! println!("activation: {:?}", report.activation);
//! # Ok::<(), shellcomp::Error>(())
//! ```
//!
//! # Custom Path Example
//!
//! Passing [`InstallRequest::path_override`] tells `shellcomp` to skip startup wiring and report
//! manual activation explicitly.
//!
//! ```no_run
//! use std::path::PathBuf;
//!
//! use shellcomp::{ActivationMode, Availability, InstallRequest, Shell, install};
//!
//! let report = install(InstallRequest {
//!     shell: Shell::Fish,
//!     program_name: "demo",
//!     script: b"complete -c demo\n",
//!     path_override: Some(PathBuf::from("/tmp/demo.fish")),
//! })?;
//!
//! assert_eq!(report.activation.mode, ActivationMode::Manual);
//! assert_eq!(report.activation.availability, Availability::ManualActionRequired);
//! # Ok::<(), shellcomp::Error>(())
//! ```
//!
//! # Structured Error Handling
//!
//! High-level operational failures are returned as [`Error::Failure`] with a stable
//! [`FailureKind`]. That lets callers keep their own presentation layer while still branching on
//! machine-readable failure categories.
//!
//! ```rust
//! use std::path::PathBuf;
//!
//! use shellcomp::{Error, FailureKind, InstallRequest, Shell, install};
//!
//! let error = install(InstallRequest {
//!     shell: Shell::Fish,
//!     program_name: "demo",
//!     script: b"complete -c demo\n",
//!     path_override: Some(PathBuf::from("/")),
//! })
//! .expect_err("path without parent should fail structurally");
//!
//! match error {
//!     Error::Failure(report) => {
//!         assert_eq!(report.kind, FailureKind::InvalidTargetPath);
//!     }
//!     other => panic!("unexpected error: {other}"),
//! }
//! ```
//!
//! Not every error becomes [`Error::Failure`]: immediate validation problems like
//! [`Error::InvalidProgramName`] are returned directly.
//!
//! # Idempotency
//!
//! Repeating the same install or uninstall operation is expected to be safe:
//!
//! - identical completion file writes return [`FileChange::Unchanged`];
//! - repeated removals return [`FileChange::Absent`];
//! - managed startup blocks are updated in place and removed by stable markers.
//!
//! This makes the crate suitable for CLI commands that users may run multiple times.

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

mod api;
mod error;
mod model;
mod service;

pub(crate) mod infra;
pub(crate) mod shell;

#[cfg(test)]
mod tests;

#[cfg(feature = "clap")]
#[cfg_attr(docsrs, doc(cfg(feature = "clap")))]
pub use api::render_clap_completion;
pub use api::{default_install_path, detect_activation, install, uninstall};
pub use error::{Error, Result};
pub use model::{
    ActivationMode, ActivationReport, Availability, CleanupReport, FailureKind, FailureReport,
    FileChange, InstallReport, InstallRequest, Operation, RemoveReport, Shell, UninstallRequest,
};
