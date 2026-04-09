pub(crate) mod detect;
pub(crate) mod install;
pub(crate) mod migrate;
pub(crate) mod uninstall;

use std::cell::Cell;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use crate::error::{Error, Result};
use crate::infra::env::Environment;
use crate::model::{
    ActivationMode, ActivationPolicy, ActivationReport, Availability, CleanupReport, FailureKind,
    FailureReport, FileChange, Operation, Shell,
};

thread_local! {
    static OPERATION_TRACE_ID: Cell<u64> = const { Cell::new(0) };
}

static NEXT_OPERATION_TRACE_ID: AtomicU64 = AtomicU64::new(1);

fn allocate_trace_id() -> u64 {
    NEXT_OPERATION_TRACE_ID.fetch_add(1, Relaxed)
}

struct TraceScope {
    previous: u64,
}

impl Drop for TraceScope {
    fn drop(&mut self) {
        OPERATION_TRACE_ID.with(|slot| slot.set(self.previous));
    }
}

fn with_trace_scope<R>(f: impl FnOnce(u64) -> R) -> R {
    let (trace_id, previous) = OPERATION_TRACE_ID.with(|slot| {
        let previous = slot.get();
        let trace_id = if previous == 0 {
            allocate_trace_id()
        } else {
            previous
        };
        slot.set(trace_id);
        (trace_id, previous)
    });

    let _scope = TraceScope { previous };
    f(trace_id)
}

pub(crate) fn with_operation_trace<R>(f: impl FnOnce(u64) -> R) -> R {
    with_trace_scope(f)
}

fn active_trace_id() -> u64 {
    OPERATION_TRACE_ID.with(|slot| {
        let trace_id = slot.get();
        if trace_id == 0 {
            let fallback = allocate_trace_id();
            slot.set(fallback);
            fallback
        } else {
            trace_id
        }
    })
}

pub(crate) fn validate_target_path(path: &Path) -> Result<()> {
    if path.is_relative() {
        return Err(Error::InvalidTargetPath {
            path: path.to_path_buf(),
            reason: "target path must be absolute",
        });
    }

    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(Error::InvalidTargetPath {
            path: path.to_path_buf(),
            reason: "target path must be normalized",
        });
    }

    for candidate in path_ancestor_sequence(path) {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::InvalidTargetPath {
                    path: path.to_path_buf(),
                    reason: "target path must not be a symbolic link",
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Error::io("inspect path", candidate, error));
            }
        }
    }

    Ok(())
}

fn path_ancestor_sequence(path: &Path) -> Vec<PathBuf> {
    let mut parts = Vec::new();
    let mut current = Some(path);

    while let Some(entry) = current {
        parts.push(entry.to_path_buf());
        current = entry.parent();
    }

    parts
}

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
        Shell::Elvish => format!(
            "Add `eval (slurp < {})` to your Elvish rc.elv.",
            elvish_quote(target)
        ),
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

pub(crate) fn missing_completion_next_step(
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<String> {
    let target = target_path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: target_path.to_path_buf(),
    })?;

    Ok(match shell {
        Shell::Bash => format!(
            "Run your CLI's completion install command, or install the script at `{target}` and source it from your shell startup file."
        ),
        Shell::Zsh => {
            let expected = format!("_{program_name}");
            if target_path.file_name().and_then(|value| value.to_str()) == Some(expected.as_str()) {
                format!(
                    "Run your CLI's completion install command, or place `{target}` in a directory listed in `fpath` and run `compinit -i`."
                )
            } else {
                format!(
                    "Run your CLI's completion install command, or place the file at `{target}`, rename it to `{expected}`, and ensure its directory is in `fpath` before running `compinit -i`."
                )
            }
        }
        Shell::Fish => format!(
            "Run your CLI's completion install command, or place the completion file at `{target}` manually."
        ),
        Shell::Powershell => format!(
            "Run your CLI's completion install command, or place the script at `{target}` and add `. {}` to a PowerShell profile.",
            powershell_quote(target)
        ),
        Shell::Elvish => format!(
            "Run your CLI's completion install command, or place the script at `{target}` and add `eval (slurp < {})` to rc.elv.",
            elvish_quote(target)
        ),
        Shell::Other(_) => format!(
            "Run your CLI's completion install command, or install `{target}` manually for `{shell}`."
        ),
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

pub(crate) fn home_env_hint(env: &Environment, shell: &Shell) -> &'static str {
    if matches!(shell, Shell::Powershell) && env.is_windows_platform() {
        "HOME or USERPROFILE"
    } else {
        "HOME"
    }
}

fn powershell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

fn elvish_quote(path: &str) -> String {
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
        trace_id: active_trace_id(),
    })
}
