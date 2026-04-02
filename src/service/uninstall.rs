use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::infra::{env::Environment, fs, paths};
use crate::model::{CleanupReport, FailureKind, Operation, RemoveReport, UninstallRequest};
use crate::service::{FailureContext, failure, push_unique};
use crate::shell;

pub(crate) fn execute(env: &Environment, request: UninstallRequest<'_>) -> Result<RemoveReport> {
    paths::validate_program_name(request.program_name)?;
    let target_path =
        resolve_target_path(env, &request).map_err(|error| map_resolve_error(&request, error))?;
    let file_change = fs::remove_file_if_exists(&target_path)
        .map_err(|error| map_file_error(&request, &target_path, error))?;

    let mut affected_locations = Vec::new();
    push_unique(&mut affected_locations, target_path.clone());

    let cleanup = if request.path_override.is_some() {
        CleanupReport {
            mode: crate::ActivationMode::Manual,
            change: crate::FileChange::Absent,
            location: None,
            reason: Some(
                "A custom path override was used, so shellcomp did not manage activation cleanup."
                    .to_owned(),
            ),
            next_step: None,
        }
    } else {
        let outcome =
            shell::uninstall_default(env, &request.shell, request.program_name, &target_path)
                .map_err(|error| map_cleanup_error(&request, &target_path, error))?;
        for path in outcome.affected_locations {
            push_unique(&mut affected_locations, path);
        }
        outcome.cleanup
    };

    Ok(RemoveReport {
        shell: request.shell,
        target_path,
        file_change,
        cleanup,
        affected_locations,
    })
}

fn resolve_target_path(env: &Environment, request: &UninstallRequest<'_>) -> Result<PathBuf> {
    match &request.path_override {
        Some(path) => {
            if path.parent().is_none() {
                return Err(Error::PathHasNoParent { path: path.clone() });
            }
            Ok(path.clone())
        }
        None => paths::default_install_path(env, &request.shell, request.program_name),
    }
}

fn map_resolve_error(request: &UninstallRequest<'_>, error: Error) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &request.shell,
                target_path: request.path_override.as_deref(),
                affected_locations: Vec::new(),
                kind: FailureKind::MissingHome,
            },
            "Could not resolve the managed completion path because HOME is not set.",
            Some(
                "Set HOME for the current process or pass the exact `path_override` that should be removed."
                    .to_owned(),
            ),
        ),
        Error::PathHasNoParent { path } => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &request.shell,
                target_path: Some(&path),
                affected_locations: vec![path.clone()],
                kind: FailureKind::InvalidTargetPath,
            },
            format!(
                "The requested uninstall path `{}` does not have a parent directory.",
                path.display()
            ),
            Some(
                "Pass the exact file path that should be removed, including a real parent directory."
                    .to_owned(),
            ),
        ),
        Error::UnsupportedShell(shell) => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &shell,
                target_path: request.path_override.as_deref(),
                affected_locations: Vec::new(),
                kind: FailureKind::UnsupportedShell,
            },
            format!("Shell `{shell}` is not implemented in the current production support set."),
            None,
        ),
        other => other,
    }
}

fn map_file_error(
    request: &UninstallRequest<'_>,
    target_path: &std::path::Path,
    error: Error,
) -> Error {
    match error {
        Error::Io { action, .. } => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: match action {
                    "remove file" => FailureKind::CompletionTargetUnavailable,
                    _ => FailureKind::CompletionFileUnreadable,
                },
            },
            format!(
                "Could not remove the {} completion file at `{}`.",
                request.shell,
                target_path.display()
            ),
            Some(
                "Remove the file manually or fix the file permissions, then run uninstall again."
                    .to_owned(),
            ),
        ),
        other => other,
    }
}

fn map_cleanup_error(
    request: &UninstallRequest<'_>,
    target_path: &std::path::Path,
    error: Error,
) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: FailureKind::MissingHome,
            },
            format!(
                "Could not resolve the managed {} startup file because HOME is not set.",
                request.shell
            ),
            Some(
                "Set HOME for the current process or remove the managed shell block manually."
                    .to_owned(),
            ),
        ),
        Error::Io { path, .. } | Error::InvalidUtf8File { path } => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileUnavailable,
            },
            format!(
                "Could not clean up the managed {} activation block.",
                request.shell
            ),
            Some(
                "Review the managed shell startup file manually and remove the shellcomp-managed block yourself."
                    .to_owned(),
            ),
        ),
        Error::ManagedBlockMissingEnd { path, .. } => failure(
            FailureContext {
                operation: Operation::Uninstall,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileCorrupted,
            },
            format!(
                "Could not clean up the managed {} activation block.",
                request.shell
            ),
            Some(
                "Review the managed shell startup file manually and remove the shellcomp-managed block yourself."
                    .to_owned(),
            ),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::execute;
    use crate::infra::env::Environment;
    use crate::model::{FileChange, InstallRequest, Operation, Shell, UninstallRequest};
    use crate::service::install;

    #[test]
    fn uninstall_removes_managed_bash_block() {
        let temp_root = crate::tests::temp_dir("uninstall-bash-managed");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        install::execute(
            &env,
            InstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                script: b"complete -F _tool tool\n",
                path_override: None,
            },
        )
        .expect("install should succeed");

        let report = execute(
            &env,
            UninstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: None,
            },
        )
        .expect("uninstall should succeed");

        assert_eq!(report.file_change, FileChange::Removed);
        assert_eq!(report.cleanup.change, FileChange::Removed);

        let bashrc = fs::read_to_string(home.join(".bashrc")).expect(".bashrc should exist");
        assert!(!bashrc.contains("shellcomp bash tool"));
    }

    #[test]
    fn uninstall_is_idempotent() {
        let temp_root = crate::tests::temp_dir("uninstall-idempotent");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = execute(
            &env,
            UninstallRequest {
                shell: Shell::Fish,
                program_name: "tool",
                path_override: None,
            },
        )
        .expect("uninstall should succeed");

        assert_eq!(report.file_change, FileChange::Absent);
        assert_eq!(report.cleanup.mode, crate::ActivationMode::NativeDirectory);
        assert_eq!(report.cleanup.change, FileChange::Absent);
    }

    #[test]
    fn uninstall_with_path_override_does_not_touch_rc_files() {
        let temp_root = crate::tests::temp_dir("uninstall-custom-path");
        let target = temp_root.join("custom").join("tool.bash");
        fs::create_dir_all(target.parent().expect("custom path should have parent"))
            .expect("custom dir should be creatable");
        fs::write(&target, "complete -F _tool tool\n").expect("target file should exist");

        let report = execute(
            &Environment::test().without_real_path_lookups(),
            UninstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: Some(target.clone()),
            },
        )
        .expect("uninstall should succeed");

        assert_eq!(report.file_change, FileChange::Removed);
        assert_eq!(report.cleanup.mode, crate::ActivationMode::Manual);
        assert_eq!(report.cleanup.change, FileChange::Absent);
        assert!(!target.exists());
    }

    #[test]
    fn uninstall_returns_profile_corrupted_for_malformed_bash_block() {
        let temp_root = crate::tests::temp_dir("uninstall-bash-corrupted");
        let home = temp_root.join("home");
        let completion_dir = home.join(".local/share/bash-completion/completions");
        fs::create_dir_all(&completion_dir).expect("completion dir should be creatable");
        fs::write(
            home.join(".bashrc"),
            "# >>> shellcomp bash tool >>>\n. '/tmp/tool'\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error = execute(
            &env,
            UninstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: None,
            },
        )
        .expect_err("uninstall should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::Uninstall);
                assert_eq!(report.kind, crate::FailureKind::ProfileCorrupted);
                assert_eq!(report.target_path, Some(completion_dir.join("tool")));
                assert!(report.cleanup.is_none());
                assert!(
                    report
                        .affected_locations
                        .iter()
                        .any(|path| path.ends_with(".bashrc"))
                );
                assert!(report.next_step.is_some());
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }
}
