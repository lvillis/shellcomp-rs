use crate::error::Result;
use crate::infra::{env::Environment, paths};
use crate::model::{ActivationReport, FailureKind, Operation, Shell};
use crate::service::{FailureContext, failure};
use crate::{Error, shell};

pub(crate) fn execute(
    env: &Environment,
    shell: Shell,
    program_name: &str,
) -> Result<ActivationReport> {
    paths::validate_program_name(program_name)?;
    let target_path = paths::default_install_path(env, &shell, program_name)
        .map_err(|error| map_resolve_error(&shell, error))?;
    shell::detect_default(env, &shell, program_name, &target_path)
        .map_err(|error| map_detect_error(&shell, &target_path, error))
}

fn map_resolve_error(shell: &Shell, error: Error) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::DetectActivation,
                shell,
                target_path: None,
                affected_locations: Vec::new(),
                kind: FailureKind::MissingHome,
            },
            "Could not resolve the managed completion path because HOME is not set.",
            Some(
                "Set HOME for the current process so shellcomp can resolve the default managed path."
                    .to_owned(),
            ),
        ),
        Error::UnsupportedShell(unsupported) => failure(
            FailureContext {
                operation: Operation::DetectActivation,
                shell: &unsupported,
                target_path: None,
                affected_locations: Vec::new(),
                kind: FailureKind::UnsupportedShell,
            },
            format!(
                "Shell `{unsupported}` is not implemented in the current production support set."
            ),
            None,
        ),
        other => other,
    }
}

fn map_detect_error(shell: &Shell, target_path: &std::path::Path, error: Error) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::DetectActivation,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: FailureKind::MissingHome,
            },
            format!(
                "Could not resolve the managed {} startup file because HOME is not set.",
                shell
            ),
            Some(
                "Set HOME for the current process or inspect activation manually for the target completion file."
                    .to_owned(),
            ),
        ),
        Error::Io { path, .. } | Error::InvalidUtf8File { path } => failure(
            FailureContext {
                operation: Operation::DetectActivation,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileUnavailable,
            },
            format!("Could not inspect the managed {} activation state.", shell),
            Some(
                "Review the relevant shell startup file manually, or re-run install to restore managed wiring."
                    .to_owned(),
            ),
        ),
        Error::ManagedBlockMissingEnd { path, .. } => failure(
            FailureContext {
                operation: Operation::DetectActivation,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileCorrupted,
            },
            format!(
                "The managed {} activation block is malformed and could not be inspected safely.",
                shell
            ),
            Some(
                "Repair or remove the malformed managed block manually, then re-run install."
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
    use crate::model::{ActivationMode, Availability, InstallRequest, Operation, Shell};
    use crate::service::install;

    #[test]
    fn detect_reports_missing_completion() {
        let temp_root = crate::tests::temp_dir("detect-missing");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_CONFIG_HOME")
            .without_real_path_lookups();

        let report = execute(&env, Shell::Fish, "tool").expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::NativeDirectory);
        assert_eq!(report.availability, Availability::ManualActionRequired);
    }

    #[test]
    fn detect_reports_installed_zsh_completion() {
        let temp_root = crate::tests::temp_dir("detect-zsh");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();

        install::execute(
            &env,
            InstallRequest {
                shell: Shell::Zsh,
                program_name: "tool",
                script: b"#compdef tool\n",
                path_override: None,
            },
        )
        .expect("install should succeed");

        let report = execute(&env, Shell::Zsh, "tool").expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::AvailableAfterSource);
    }

    #[test]
    fn detect_fails_without_home_for_default_paths() {
        let env = Environment::test()
            .without_var("HOME")
            .without_var("ZDOTDIR")
            .without_real_path_lookups();

        let error = execute(&env, Shell::Zsh, "tool").expect_err("detect should fail");

        assert!(matches!(
            error,
            crate::Error::Failure(report) if report.kind == crate::FailureKind::MissingHome
        ));
    }

    #[test]
    fn detect_returns_profile_corrupted_for_malformed_zsh_block() {
        let temp_root = crate::tests::temp_dir("detect-zsh-corrupted");
        let home = temp_root.join("home");
        let completion_dir = home.join(".zfunc");
        fs::create_dir_all(&completion_dir).expect("completion dir should be creatable");
        fs::write(completion_dir.join("_tool"), b"#compdef tool\n")
            .expect("completion file should be writable");
        fs::write(
            home.join(".zshrc"),
            "# >>> shellcomp zsh tool >>>\nfpath=(~/.zfunc $fpath)\n",
        )
        .expect(".zshrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();

        let error = execute(&env, Shell::Zsh, "tool").expect_err("detect should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::DetectActivation);
                assert_eq!(report.kind, crate::FailureKind::ProfileCorrupted);
                assert_eq!(report.target_path, Some(completion_dir.join("_tool")));
                assert!(
                    report
                        .affected_locations
                        .iter()
                        .any(|path| path.ends_with(".zshrc"))
                );
                assert!(report.next_step.is_some());
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }
}
