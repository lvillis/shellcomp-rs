use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::{env::Environment, fs, paths};
use crate::model::{
    ActivationMode, ActivationReport, Availability, FailureKind, FileChange, InstallReport,
    InstallRequest, Operation, Shell,
};
use crate::service::{
    FailureContext, FailureStatus, failure, failure_with_status, manual_activation_report,
    push_unique,
};
use crate::shell;

pub(crate) fn execute(env: &Environment, request: InstallRequest<'_>) -> Result<InstallReport> {
    paths::validate_program_name(request.program_name)?;
    let target_path =
        resolve_target_path(env, &request).map_err(|error| map_resolve_error(&request, error))?;
    let file_change = fs::write_if_changed(&target_path, request.script)
        .map_err(|error| map_write_error(&request, &target_path, error))?;

    let mut affected_locations = Vec::new();
    push_unique(&mut affected_locations, target_path.clone());

    let activation = if request.path_override.is_some() {
        manual_activation_report(&request.shell, &target_path)?
    } else {
        let outcome =
            shell::install_default(env, &request.shell, request.program_name, &target_path)
                .map_err(|error| {
                    map_activation_error(&request.shell, &target_path, file_change, error)
                })?;
        for path in outcome.affected_locations {
            push_unique(&mut affected_locations, path);
        }
        outcome.report
    };

    Ok(InstallReport {
        shell: request.shell,
        target_path,
        file_change,
        activation,
        affected_locations,
    })
}

fn resolve_target_path(env: &Environment, request: &InstallRequest<'_>) -> Result<PathBuf> {
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

fn map_resolve_error(request: &InstallRequest<'_>, error: Error) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::Install,
                shell: &request.shell,
                target_path: request.path_override.as_deref(),
                affected_locations: Vec::new(),
                kind: FailureKind::MissingHome,
            },
            "Could not resolve the default managed install path because HOME is not set.",
            Some(
                "Set HOME for the current process or pass `path_override` to install into an explicit path."
                    .to_owned(),
            ),
        ),
        Error::PathHasNoParent { path } => failure(
            FailureContext {
                operation: Operation::Install,
                shell: &request.shell,
                target_path: Some(&path),
                affected_locations: vec![path.clone()],
                kind: FailureKind::InvalidTargetPath,
            },
            format!(
                "The requested install path `{}` does not have a parent directory.",
                path.display()
            ),
            Some(
                "Pass a file path with a real parent directory, or create the parent directory first."
                    .to_owned(),
            ),
        ),
        Error::UnsupportedShell(shell) => failure(
            FailureContext {
                operation: Operation::Install,
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

fn map_write_error(request: &InstallRequest<'_>, target_path: &Path, error: Error) -> Error {
    match error {
        Error::Io { action, .. } => failure(
            FailureContext {
                operation: Operation::Install,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: match action {
                    "read file" => FailureKind::CompletionFileUnreadable,
                    _ => FailureKind::CompletionTargetUnavailable,
                },
            },
            format!(
                "Could not write the {} completion file to `{}`.",
                request.shell,
                target_path.display()
            ),
            Some(default_write_next_step(
                &request.shell,
                request.path_override.is_some(),
            )),
        ),
        Error::InvalidUtf8File { path } => failure(
            FailureContext {
                operation: Operation::Install,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![path.clone(), target_path.to_path_buf()],
                kind: FailureKind::CompletionFileUnreadable,
            },
            format!(
                "The existing completion file `{}` could not be read as UTF-8, so shellcomp could not update it safely.",
                path.display()
            ),
            Some(default_write_next_step(
                &request.shell,
                request.path_override.is_some(),
            )),
        ),
        other => other,
    }
}

fn default_write_next_step(shell: &Shell, has_override: bool) -> String {
    match shell {
        Shell::Bash => {
            if has_override {
                "Choose a writable custom path, then source that file manually from a Bash startup file.".to_owned()
            } else {
                "Choose a writable path with `path_override`, or install the completion file manually into a bash-completion directory.".to_owned()
            }
        }
        Shell::Zsh => {
            if has_override {
                "Choose a writable custom path, then add its directory to `fpath` and run `compinit -i` manually.".to_owned()
            } else {
                "Choose a writable path with `path_override`, or create the managed zsh completion directory manually.".to_owned()
            }
        }
        Shell::Fish => {
            if has_override {
                "Choose a writable custom path, then place or source the file manually in Fish."
                    .to_owned()
            } else {
                "Choose a writable path with `path_override`, or place the file into Fish's completions directory manually.".to_owned()
            }
        }
        _ => "Choose a writable path and activate the completion manually.".to_owned(),
    }
}

fn map_activation_error(
    shell: &Shell,
    target_path: &Path,
    file_change: FileChange,
    error: Error,
) -> Error {
    let (reason, next_step) = match shell {
        Shell::Bash => (
            "Could not update the managed Bash startup block.".to_owned(),
            Some(format!(
                "Source `{}` manually from a writable Bash startup file, or use `path_override` and handle activation yourself.",
                target_path.display()
            )),
        ),
        Shell::Zsh => (
            "Could not update the managed Zsh startup block.".to_owned(),
            Some(format!(
                "Add `{}` to `fpath` manually and run `compinit -i`, or use `path_override` and handle activation yourself.",
                target_path.parent().map_or_else(
                    || target_path.display().to_string(),
                    |parent| parent.display().to_string()
                )
            )),
        ),
        _ => return error,
    };

    let activation = ActivationReport {
        mode: ActivationMode::Manual,
        availability: Availability::ManualActionRequired,
        location: error.location().map(Path::to_path_buf),
        reason: Some(reason.clone()),
        next_step: next_step.clone(),
    };

    match error {
        Error::MissingHome => failure_with_status(
            FailureContext {
                operation: Operation::Install,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: FailureKind::MissingHome,
            },
            FailureStatus {
                file_change: Some(file_change),
                activation: Some(activation),
                cleanup: None,
            },
            format!(
                "Could not resolve the managed {} startup file because HOME is not set.",
                shell
            ),
            next_step,
        ),
        Error::Io { path, .. } | Error::InvalidUtf8File { path } => failure_with_status(
            FailureContext {
                operation: Operation::Install,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileUnavailable,
            },
            FailureStatus {
                file_change: Some(file_change),
                activation: Some(activation),
                cleanup: None,
            },
            reason,
            next_step,
        ),
        Error::ManagedBlockMissingEnd { path, .. } => failure_with_status(
            FailureContext {
                operation: Operation::Install,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileCorrupted,
            },
            FailureStatus {
                file_change: Some(file_change),
                activation: Some(activation),
                cleanup: None,
            },
            reason,
            next_step,
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::execute;
    use crate::infra::env::Environment;
    use crate::model::{
        ActivationMode, Availability, FileChange, InstallRequest, Operation, Shell,
    };

    #[test]
    fn install_with_path_override_requires_manual_activation() {
        let temp_root = crate::tests::temp_dir("install-path-override");
        let target = temp_root.join("custom").join("tool.bash");
        let env = Environment::test();

        let report = execute(
            &env,
            InstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                script: b"complete -F _tool tool\n",
                path_override: Some(target.clone()),
            },
        )
        .expect("install should succeed");

        assert_eq!(report.file_change, FileChange::Created);
        assert_eq!(report.activation.mode, ActivationMode::Manual);
        assert_eq!(
            report.activation.availability,
            Availability::ManualActionRequired
        );
        assert_eq!(
            fs::read(&target).expect("target file should exist"),
            b"complete -F _tool tool\n"
        );
    }

    #[test]
    fn install_bash_uses_managed_rc_block_without_loader() {
        let temp_root = crate::tests::temp_dir("install-bash-managed");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = execute(
            &env,
            InstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                script: b"complete -F _tool tool\n",
                path_override: None,
            },
        )
        .expect("install should succeed");

        assert_eq!(report.file_change, FileChange::Created);
        assert_eq!(report.activation.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(
            report.activation.availability,
            Availability::AvailableAfterSource
        );
        let bashrc = home.join(".bashrc");
        let bashrc_contents = fs::read_to_string(bashrc).expect(".bashrc should be created");
        assert!(bashrc_contents.contains("shellcomp bash tool"));
        assert!(bashrc_contents.contains(". '"));
    }

    #[test]
    fn install_returns_missing_home_when_default_path_cannot_resolve() {
        let env = Environment::test()
            .without_var("HOME")
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error = execute(
            &env,
            InstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                script: b"complete -F _tool tool\n",
                path_override: None,
            },
        )
        .expect_err("install should fail without HOME");

        assert!(matches!(
            error,
            crate::Error::Failure(report) if report.kind == crate::FailureKind::MissingHome
        ));
    }

    #[test]
    fn install_fails_structurally_when_managed_bash_profile_is_not_writable() {
        let temp_root = crate::tests::temp_dir("install-bash-actionable");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::create_dir_all(home.join(".bashrc")).expect(".bashrc directory should be creatable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error = execute(
            &env,
            InstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                script: b"complete -F _tool tool\n",
                path_override: None,
            },
        )
        .expect_err("install should fail structurally");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.kind, crate::FailureKind::ProfileUnavailable);
                assert_eq!(report.file_change, Some(FileChange::Created));
                let activation = report.activation.expect("activation context should exist");
                assert_eq!(activation.mode, ActivationMode::Manual);
                assert_eq!(activation.availability, Availability::ManualActionRequired);
                assert!(
                    report
                        .affected_locations
                        .iter()
                        .any(|path| path.ends_with(".bashrc"))
                );
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn install_fails_structurally_when_managed_zsh_profile_is_not_writable() {
        let temp_root = crate::tests::temp_dir("install-zsh-structural-failure");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::create_dir_all(home.join(".zshrc")).expect(".zshrc directory should be creatable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();

        let error = execute(
            &env,
            InstallRequest {
                shell: Shell::Zsh,
                program_name: "tool",
                script: b"#compdef tool\n",
                path_override: None,
            },
        )
        .expect_err("install should fail structurally");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.kind, crate::FailureKind::ProfileUnavailable);
                assert_eq!(report.file_change, Some(FileChange::Created));
                let activation = report.activation.expect("activation context should exist");
                assert_eq!(activation.mode, ActivationMode::Manual);
                assert_eq!(activation.availability, Availability::ManualActionRequired);
                assert!(
                    report
                        .affected_locations
                        .iter()
                        .any(|path| path.ends_with(".zshrc"))
                );
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn install_returns_profile_corrupted_for_malformed_bash_block() {
        let temp_root = crate::tests::temp_dir("install-bash-corrupted");
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
            InstallRequest {
                shell: Shell::Bash,
                program_name: "tool",
                script: b"complete -F _tool tool\n",
                path_override: None,
            },
        )
        .expect_err("install should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::Install);
                assert_eq!(report.kind, crate::FailureKind::ProfileCorrupted);
                assert_eq!(report.target_path, Some(completion_dir.join("tool")));
                assert_eq!(report.file_change, Some(FileChange::Created));
                let activation = report.activation.expect("activation context should exist");
                assert_eq!(activation.mode, ActivationMode::Manual);
                assert_eq!(activation.availability, Availability::ManualActionRequired);
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

    #[test]
    fn install_returns_unchanged_when_completion_file_matches() {
        let temp_root = crate::tests::temp_dir("install-unchanged");
        let target = temp_root.join("custom").join("tool.bash");
        let env = Environment::test();
        let request = InstallRequest {
            shell: Shell::Bash,
            program_name: "tool",
            script: b"complete -F _tool tool\n",
            path_override: Some(target),
        };

        let first = execute(&env, request.clone()).expect("first install should succeed");
        let second = execute(&env, request).expect("second install should succeed");

        assert_eq!(first.file_change, FileChange::Created);
        assert_eq!(second.file_change, FileChange::Unchanged);
    }
}
