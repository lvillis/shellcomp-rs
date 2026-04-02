use std::path::PathBuf;

use crate::error::Result;
use crate::infra::{env::Environment, paths};
use crate::model::{InstallReport, InstallRequest, RemoveReport, Shell, UninstallRequest};
use crate::service::{detect, install, uninstall};

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
/// set, the script is written only to that path and activation is reported as manual.
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

/// Removes a previously managed completion script and any managed activation wiring.
///
/// When `path_override` is set, only that file path is removed. Managed shell startup cleanup is
/// skipped because `shellcomp` did not own activation for a custom path.
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

/// Detects how a completion would be activated for the current environment.
///
/// Detection inspects the default managed location for the given shell and binary name. This API
/// does not accept a custom path because callers can reason about explicit override paths directly.
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
