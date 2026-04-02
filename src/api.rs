use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::infra::{env::Environment, paths};
use crate::model::{
    ActivationPolicy, InstallReport, InstallRequest, MigrateManagedBlocksReport,
    MigrateManagedBlocksRequest, RemoveReport, Shell, UninstallRequest,
};
use crate::service::{detect, install, migrate, uninstall};

/// Returns the default managed install path for a shell and binary name.
///
/// The returned path follows the managed layout implemented by `shellcomp` for supported shells.
/// It validates `program_name` before constructing the path.
///
/// The concrete layout currently used by the production support set is:
///
/// - Bash: `$XDG_DATA_HOME/bash-completion/completions/<program>`
/// - Zsh: `$ZDOTDIR/.zfunc/_<program>`
/// - Fish: `$XDG_CONFIG_HOME/fish/completions/<program>.fish`
/// - PowerShell: `$XDG_DATA_HOME/powershell/completions/<program>.ps1`
/// - Elvish: `$XDG_CONFIG_HOME/elvish/lib/shellcomp/<program>.elv`
///
/// # Errors
///
/// Returns an error if `program_name` is invalid, `HOME`-derived directories cannot be resolved,
/// or the shell is not in the current production support set.
///
/// # Examples
///
/// ```no_run
/// use shellcomp::{Shell, default_install_path};
///
/// let path = default_install_path(Shell::Fish, "demo")?;
/// assert!(path.ends_with("fish/completions/demo.fish"));
/// # Ok::<(), shellcomp::Error>(())
/// ```
pub fn default_install_path(shell: Shell, program_name: &str) -> Result<PathBuf> {
    paths::default_install_path(&Environment::system(), &shell, program_name)
}

/// Installs a completion script and returns a structured report.
///
/// When `path_override` is `None`, the script is written into the shell's managed default
/// location and `shellcomp` attempts to wire activation automatically. When `path_override` is
/// set, legacy behavior is to treat non-default custom paths as manual activation, while an
/// override equal to the managed default path still keeps the default activation semantics.
///
/// This function is idempotent with respect to the written script contents and managed startup
/// wiring. Re-installing an identical script normally returns [`crate::FileChange::Unchanged`].
///
/// # Errors
///
/// Returns [`crate::Error::Failure`] for structured operational failures such as missing `HOME`,
/// unwritable target files, or shell profile update failures.
///
/// Immediate validation problems such as an invalid `program_name` are returned as direct
/// [`crate::Error`] variants instead of [`crate::Error::Failure`].
///
/// # Examples
///
/// ```no_run
/// use shellcomp::{InstallRequest, Shell, install};
///
/// let report = install(InstallRequest {
///     shell: Shell::Zsh,
///     program_name: "demo",
///     script: b"#compdef demo\n",
///     path_override: None,
/// })?;
///
/// assert_eq!(report.shell, Shell::Zsh);
/// # Ok::<(), shellcomp::Error>(())
/// ```
pub fn install(request: InstallRequest<'_>) -> Result<InstallReport> {
    install::execute(&Environment::system(), request)
}

/// Installs a completion script with explicit activation intent.
///
/// This is the opt-in API for callers that want a custom path but still want `shellcomp` to
/// manage activation when the shell supports it.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
///
/// use shellcomp::{ActivationPolicy, InstallRequest, Shell, install_with_policy};
///
/// let report = install_with_policy(
///     InstallRequest {
///         shell: Shell::Bash,
///         program_name: "demo",
///         script: b"complete -F _demo demo\n",
///         path_override: Some(PathBuf::from("/tmp/demo.bash")),
///     },
///     ActivationPolicy::AutoManaged,
/// )?;
///
/// println!("{report:#?}");
/// # Ok::<(), shellcomp::Error>(())
/// ```
pub fn install_with_policy(
    request: InstallRequest<'_>,
    activation_policy: ActivationPolicy,
) -> Result<InstallReport> {
    install::execute_with_policy(&Environment::system(), request, activation_policy)
}

/// Removes a previously managed completion script and any managed activation wiring.
///
/// When `path_override` is set, legacy behavior removes only that file path for non-default custom
/// targets. If the override is equal to the shell's managed default path, uninstall keeps the
/// default cleanup semantics for that shell.
///
/// This function is idempotent. Removing an already absent completion file returns
/// [`crate::FileChange::Absent`] rather than failing.
///
/// # Errors
///
/// Returns [`crate::Error::Failure`] for structured operational failures such as unresolved
/// managed paths or unwritable shell profile files.
///
/// # Examples
///
/// ```no_run
/// use shellcomp::{Shell, UninstallRequest, uninstall};
///
/// let report = uninstall(UninstallRequest {
///     shell: Shell::Bash,
///     program_name: "demo",
///     path_override: None,
/// })?;
///
/// assert_eq!(report.shell, Shell::Bash);
/// # Ok::<(), shellcomp::Error>(())
/// ```
pub fn uninstall(request: UninstallRequest<'_>) -> Result<RemoveReport> {
    uninstall::execute(&Environment::system(), request)
}

/// Removes a completion script with explicit activation cleanup intent.
///
/// Use this when the completion file lives at a custom path and you still want `shellcomp` to
/// clean up managed activation wiring for shells such as Bash or Zsh.
pub fn uninstall_with_policy(
    request: UninstallRequest<'_>,
    activation_policy: ActivationPolicy,
) -> Result<RemoveReport> {
    uninstall::execute_with_policy(&Environment::system(), request, activation_policy)
}

/// Detects how a completion would be activated for the current environment.
///
/// Detection inspects the default managed location for the given shell and binary name. For custom
/// paths, use [`detect_activation_at_path`].
///
/// The returned [`crate::ActivationReport`] distinguishes the wiring mechanism
/// ([`crate::ActivationMode`]) from current readiness ([`crate::Availability`]).
///
/// # Errors
///
/// Returns [`crate::Error::Failure`] when the managed path or startup wiring cannot be inspected
/// safely.
///
/// # Examples
///
/// ```no_run
/// use shellcomp::{Shell, detect_activation};
///
/// let report = detect_activation(Shell::Fish, "demo")?;
/// println!("{report:#?}");
/// # Ok::<(), shellcomp::Error>(())
/// ```
pub fn detect_activation(shell: Shell, program_name: &str) -> Result<crate::ActivationReport> {
    detect::execute(&Environment::system(), shell, program_name)
}

/// Detects activation state for an explicit completion file path.
///
/// This is useful when a caller installed to a custom path and wants detection against that exact
/// file rather than the shell's managed default location. If the explicit path matches the managed
/// default path, detection keeps the shell's default activation semantics.
pub fn detect_activation_at_path(
    shell: Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<crate::ActivationReport> {
    detect::execute_at_path(&Environment::system(), shell, program_name, target_path)
}

/// Removes caller-provided legacy managed markers and upserts the equivalent `shellcomp` block.
///
/// This helper is intended for CLI projects that previously shipped their own managed completion
/// blocks and want to migrate to `shellcomp` without leaving duplicate startup wiring behind.
///
/// For shells that do not use a managed startup file, such as Fish, this operation is a no-op.
pub fn migrate_managed_blocks(
    request: MigrateManagedBlocksRequest<'_>,
) -> Result<MigrateManagedBlocksReport> {
    migrate::execute(&Environment::system(), request)
}

#[cfg(feature = "clap")]
#[cfg_attr(docsrs, doc(cfg(feature = "clap")))]
/// Renders a completion script from a `clap::CommandFactory` implementation.
///
/// This helper is intentionally optional so the core crate does not require `clap`.
/// It only renders script bytes; installation and activation are still handled by [`install`].
///
/// # Errors
///
/// Returns [`crate::Error::UnsupportedShell`] for `Shell::Other(_)`.
///
/// # Examples
///
/// ```no_run
/// use clap::Parser;
/// use shellcomp::{Shell, render_clap_completion};
///
/// #[derive(Parser)]
/// struct Cli {
///     #[arg(long)]
///     verbose: bool,
/// }
///
/// let script = render_clap_completion::<Cli>(Shell::Bash, "demo")?;
/// assert!(!script.is_empty());
/// # Ok::<(), shellcomp::Error>(())
/// ```
pub fn render_clap_completion<T: clap::CommandFactory>(
    shell: Shell,
    bin_name: &str,
) -> Result<Vec<u8>> {
    use clap_complete::{Generator, generate};

    fn map_shell(shell: Shell) -> Result<impl Generator> {
        match shell {
            Shell::Bash => Ok(clap_complete::Shell::Bash),
            Shell::Zsh => Ok(clap_complete::Shell::Zsh),
            Shell::Fish => Ok(clap_complete::Shell::Fish),
            Shell::Elvish => Ok(clap_complete::Shell::Elvish),
            Shell::Powershell => Ok(clap_complete::Shell::PowerShell),
            Shell::Other(value) => Err(crate::Error::UnsupportedShell(Shell::Other(value))),
        }
    }

    let generator = map_shell(shell)?;
    let mut command = T::command();
    let mut output = Vec::new();
    generate(generator, &mut command, bin_name, &mut output);
    Ok(output)
}

#[cfg(all(test, feature = "clap"))]
mod tests {
    use clap::Parser;

    use super::render_clap_completion;
    use crate::Shell;

    #[derive(Parser)]
    struct TestCli {
        #[arg(long)]
        verbose: bool,
    }

    #[test]
    fn renders_clap_completion() {
        let script = render_clap_completion::<TestCli>(Shell::Bash, "test-cli")
            .expect("bash completion should render");
        let rendered = String::from_utf8(script).expect("completion output should be utf-8");
        assert!(rendered.contains("test-cli"));
        assert!(rendered.contains("_test-cli"));
    }

    #[test]
    fn rejects_other_shell_for_clap_generation() {
        let error = render_clap_completion::<TestCli>(Shell::Other("xonsh".to_owned()), "test-cli")
            .expect_err("unsupported shell should fail");

        assert!(matches!(
            error,
            crate::Error::UnsupportedShell(Shell::Other(value)) if value == "xonsh"
        ));
    }
}
