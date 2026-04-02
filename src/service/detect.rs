use std::path::Path;

use crate::error::Result;
use crate::infra::{env::Environment, fs, paths};
use crate::model::{
    ActivationPolicy, ActivationReport, Availability, FailureKind, Operation, Shell,
};
use crate::service::{
    FailureContext, failure, manual_activation_report, zsh_target_is_autoloadable,
};
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

pub(crate) fn execute_at_path(
    env: &Environment,
    shell: Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationReport> {
    paths::validate_program_name(program_name)?;
    match shell {
        Shell::Fish => {
            if path_matches_default_target(env, &Shell::Fish, program_name, target_path) {
                shell::detect_default(env, &shell, program_name, target_path)
                    .map_err(|error| map_detect_error(&shell, target_path, error))
            } else {
                manual_custom_detection_report(&shell, program_name, target_path)
                    .map_err(|error| map_detect_error(&shell, target_path, error))
            }
        }
        Shell::Bash => {
            detect_custom_path_with_managed_fallback(env, &shell, program_name, target_path)
        }
        Shell::Zsh => {
            if !zsh_target_is_autoloadable(program_name, target_path) {
                return manual_custom_detection_report(&shell, program_name, target_path)
                    .map_err(|error| map_detect_error(&shell, target_path, error));
            }

            detect_custom_path_with_managed_fallback(env, &shell, program_name, target_path)
        }
        Shell::Powershell | Shell::Elvish => {
            detect_custom_path_with_managed_fallback(env, &shell, program_name, target_path)
        }
        _ => shell::detect_default(env, &shell, program_name, target_path)
            .map_err(|error| map_detect_error(&shell, target_path, error)),
    }
}

fn detect_custom_path_with_managed_fallback(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationReport> {
    let treat_as_default = path_matches_default_target(env, shell, program_name, target_path);
    let installed = fs::file_exists(target_path);
    match shell::detect_default(env, shell, program_name, target_path) {
        Ok(report) => {
            if treat_as_default
                || !installed
                || report.availability != Availability::ManualActionRequired
            {
                Ok(report)
            } else {
                manual_custom_detection_report(shell, program_name, target_path)
                    .map_err(|error| map_detect_error(shell, target_path, error))
            }
        }
        Err(error) if !treat_as_default && can_fallback_to_manual_custom_detect(&error) => {
            manual_custom_detection_report(shell, program_name, target_path)
                .map_err(|fallback_error| map_detect_error(shell, target_path, fallback_error))
        }
        Err(error) => Err(map_detect_error(shell, target_path, error)),
    }
}

fn can_fallback_to_manual_custom_detect(error: &Error) -> bool {
    matches!(
        error,
        Error::MissingHome | Error::Io { .. } | Error::InvalidUtf8File { .. }
    )
}

fn path_matches_default_target(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> bool {
    paths::default_install_path(env, shell, program_name)
        .map(|default_path| default_path == target_path)
        .unwrap_or(false)
}

fn manual_custom_detection_report(
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationReport> {
    let installed = fs::file_exists(target_path);
    let mut report = manual_activation_report(
        shell,
        program_name,
        target_path,
        true,
        ActivationPolicy::Manual,
    )?;
    report.availability = if installed {
        Availability::Unknown
    } else {
        Availability::ManualActionRequired
    };
    report.reason = Some(if installed {
        format!(
            "Completion file `{}` is installed at a custom path, but shellcomp could not confirm managed activation for it.",
            target_path.display()
        )
    } else {
        format!(
            "Completion file `{}` is not installed.",
            target_path.display()
        )
    });
    Ok(report)
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
        Error::NonUtf8Path { path } => failure(
            FailureContext {
                operation: Operation::DetectActivation,
                shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path],
                kind: FailureKind::InvalidTargetPath,
            },
            "The requested completion path could not be represented safely as UTF-8 for activation detection.",
            Some(
                "Move the completion file to a UTF-8 path or choose a UTF-8 path before asking shellcomp to inspect activation."
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

    use super::{execute, execute_at_path};
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

    #[test]
    fn detect_at_path_reports_unknown_for_custom_bash_path_without_managed_wiring() {
        let temp_root = crate::tests::temp_dir("detect-custom-bash-manual");
        let home = temp_root.join("home");
        let target = temp_root.join("custom").join("tool.bash");
        fs::create_dir_all(target.parent().expect("target should have a parent"))
            .expect("target dir should be creatable");
        fs::write(&target, "complete -F _tool tool\n").expect("target should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report =
            execute_at_path(&env, Shell::Bash, "tool", &target).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::Manual);
        assert_eq!(report.availability, Availability::Unknown);
    }

    #[test]
    fn detect_at_path_keeps_reinstall_guidance_for_missing_custom_managed_bash_script() {
        let temp_root = crate::tests::temp_dir("detect-custom-bash-missing-script");
        let home = temp_root.join("home");
        let target = temp_root.join("custom").join("tool.bash");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bashrc"),
            format!(
                "# >>> shellcomp bash tool >>>\nif [ -f '{}' ]; then\n  . '{}'\nfi\n# <<< shellcomp bash tool <<<\n",
                target.display(),
                target.display()
            ),
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report =
            execute_at_path(&env, Shell::Bash, "tool", &target).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::ManualActionRequired);
        assert!(
            report
                .next_step
                .as_deref()
                .is_some_and(|text| text.contains("install command") || text.contains("install"))
        );
    }

    #[test]
    fn detect_at_path_reports_profile_corruption_for_custom_bash_path() {
        let temp_root = crate::tests::temp_dir("detect-custom-bash-corrupted-profile");
        let home = temp_root.join("home");
        let target = temp_root.join("custom").join("tool.bash");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::create_dir_all(target.parent().expect("target should have a parent"))
            .expect("target dir should be creatable");
        fs::write(&target, "complete -F _tool tool\n").expect("target should be writable");
        fs::write(
            home.join(".bashrc"),
            "# >>> shellcomp bash tool >>>\n. '/tmp/tool'\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error =
            execute_at_path(&env, Shell::Bash, "tool", &target).expect_err("detect should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::DetectActivation);
                assert_eq!(report.kind, crate::FailureKind::ProfileCorrupted);
                assert_eq!(report.target_path, Some(target));
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn detect_at_path_reports_manual_for_non_autoloadable_zsh_target() {
        let temp_root = crate::tests::temp_dir("detect-custom-zsh-manual");
        let home = temp_root.join("home");
        let target = temp_root.join("custom").join("tool.zsh");
        fs::create_dir_all(target.parent().expect("target should have a parent"))
            .expect("target dir should be creatable");
        fs::write(&target, "#compdef tool\n").expect("target should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();

        let report =
            execute_at_path(&env, Shell::Zsh, "tool", &target).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::Manual);
        assert_eq!(report.availability, Availability::Unknown);
    }

    #[cfg(unix)]
    #[test]
    fn detect_at_path_returns_structured_failure_for_non_utf8_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp_root = crate::tests::temp_dir("detect-non-utf8-path");
        let target = temp_root.join(OsString::from_vec(b"tool-\xff.fish".to_vec()));
        std::fs::write(&target, "complete -c tool -f\n").expect("target should be writable");

        let env = Environment::test().without_real_path_lookups();

        let error = execute_at_path(&env, Shell::Fish, "tool", &target)
            .expect_err("detect should fail structurally");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::DetectActivation);
                assert_eq!(report.kind, crate::FailureKind::InvalidTargetPath);
                assert_eq!(report.target_path, Some(target));
                assert!(report.next_step.is_some());
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn detect_at_path_does_not_require_home_for_custom_powershell_path() {
        let temp_root = crate::tests::temp_dir("detect-custom-powershell-no-home");
        let target = temp_root.join("custom").join("tool.ps1");
        fs::create_dir_all(target.parent().expect("target should have a parent"))
            .expect("target dir should be creatable");
        fs::write(&target, "# powershell completion\n").expect("target should be writable");

        let env = Environment::test()
            .without_var("HOME")
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = execute_at_path(&env, Shell::Powershell, "tool", &target)
            .expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::Manual);
        assert_eq!(report.availability, Availability::Unknown);
    }
}
