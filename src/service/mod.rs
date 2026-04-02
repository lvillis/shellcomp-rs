pub(crate) mod detect;
pub(crate) mod install;
pub(crate) mod uninstall;

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::model::{
    ActivationMode, ActivationReport, Availability, CleanupReport, FailureKind, FailureReport,
    FileChange, Operation, Shell,
};

pub(crate) fn manual_activation_report(
    shell: &Shell,
    target_path: &Path,
) -> Result<ActivationReport> {
    let target = target_path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: target_path.to_path_buf(),
    })?;

    let next_step = match shell {
        Shell::Bash => format!(
            "Source `{target}` from your shell startup file, or move it into the standard bash-completion directory."
        ),
        Shell::Zsh => format!(
            "Add `{target}` to your zsh completion setup manually, then ensure its directory is in `fpath` and run `compinit -i`."
        ),
        Shell::Fish => {
            format!("Place `{target}` under Fish's completions directory or source it manually.")
        }
        _ => format!("Activate `{target}` manually for `{shell}`."),
    };

    Ok(ActivationReport {
        mode: ActivationMode::Manual,
        availability: Availability::ManualActionRequired,
        location: Some(target_path.to_path_buf()),
        reason: Some(
            "A custom install path was provided, so shellcomp skipped automatic activation wiring."
                .to_owned(),
        ),
        next_step: Some(next_step),
    })
}

pub(crate) fn push_unique(paths: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
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
