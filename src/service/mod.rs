pub(crate) mod detect;
pub(crate) mod install;
pub(crate) mod migrate;
pub(crate) mod uninstall;

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::{
    ActivationMode, ActivationPolicy, ActivationReport, Availability, CleanupReport, FailureKind,
    FailureReport, FileChange, Operation, Shell,
};

pub(crate) fn manual_activation_report(
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
    custom_path: bool,
    activation_policy: ActivationPolicy,
) -> Result<ActivationReport> {
    let target = target_path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: target_path.to_path_buf(),
    })?;

    let next_step = match shell {
        Shell::Bash => format!(
            "Source `{target}` from your shell startup file, or move it into the standard bash-completion directory."
        ),
        Shell::Zsh => {
            let expected = format!("_{program_name}");
            if target_path.file_name().and_then(|value| value.to_str()) == Some(expected.as_str()) {
                format!(
                    "Ensure `{target}` is in a directory listed in `fpath`, then run `compinit -i`."
                )
            } else {
                format!(
                    "Rename the file to `{expected}` or load it manually, then ensure its directory is in `fpath` and run `compinit -i`."
                )
            }
        }
        Shell::Fish => {
            format!("Place `{target}` under Fish's completions directory or source it manually.")
        }
        Shell::Powershell => format!(
            "Add `. {}` to `$PROFILE.CurrentUserAllHosts` or another PowerShell profile.",
            powershell_quote(target)
        ),
        Shell::Elvish => {
            format!(
                "Evaluate `{target}` from your Elvish rc.elv. If you use a command such as `slurp`, make sure the installed path is quoted correctly for Elvish."
            )
        }
        Shell::Other(_) => format!("Activate `{target}` manually for `{shell}`."),
    };

    let reason = if custom_path && matches!(activation_policy, ActivationPolicy::Manual) {
        "A custom install path was provided, so shellcomp skipped automatic activation wiring."
            .to_owned()
    } else {
        match activation_policy {
            ActivationPolicy::AutoManaged => {
                "The shell does not support safe managed activation for this installation target, so manual activation is required."
                    .to_owned()
            }
            ActivationPolicy::Manual => {
            "Automatic activation wiring was skipped because the activation policy is manual."
                .to_owned()
            }
        }
    };

    Ok(ActivationReport {
        mode: ActivationMode::Manual,
        availability: Availability::ManualActionRequired,
        location: Some(target_path.to_path_buf()),
        reason: Some(reason),
        next_step: Some(next_step),
    })
}

pub(crate) fn push_unique(paths: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub(crate) fn zsh_target_is_autoloadable(program_name: &str, target_path: &Path) -> bool {
    let expected = format!("_{program_name}");
    target_path.file_name().and_then(|value| value.to_str()) == Some(expected.as_str())
}

fn powershell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

pub(crate) struct FailureContext<'a> {
    pub(crate) operation: Operation,
    pub(crate) shell: &'a Shell,
    pub(crate) target_path: Option<&'a Path>,
    pub(crate) affected_locations: Vec<PathBuf>,
    pub(crate) kind: FailureKind,
}

#[derive(Default)]
pub(crate) struct FailureStatus {
    pub(crate) file_change: Option<FileChange>,
    pub(crate) activation: Option<ActivationReport>,
    pub(crate) cleanup: Option<CleanupReport>,
}

pub(crate) fn failure(
    context: FailureContext<'_>,
    reason: impl Into<String>,
    next_step: Option<String>,
) -> Error {
    failure_with_status(context, FailureStatus::default(), reason, next_step)
}

pub(crate) fn failure_with_status(
    context: FailureContext<'_>,
    status: FailureStatus,
    reason: impl Into<String>,
    next_step: Option<String>,
) -> Error {
    Error::failure(FailureReport {
        operation: context.operation,
        shell: context.shell.clone(),
        target_path: context.target_path.map(Path::to_path_buf),
        affected_locations: context.affected_locations,
        kind: context.kind,
        file_change: status.file_change,
        activation: status.activation,
        cleanup: status.cleanup,
        reason: reason.into(),
        next_step,
    })
}
