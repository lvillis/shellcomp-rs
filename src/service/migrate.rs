use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::infra::{env::Environment, paths};
use crate::model::{
    FailureKind, MigrateManagedBlocksReport, MigrateManagedBlocksRequest, Operation,
};
use crate::service::{FailureContext, failure, push_unique, zsh_target_is_autoloadable};
use crate::shell;

pub(crate) fn execute(
    env: &Environment,
    request: MigrateManagedBlocksRequest<'_>,
) -> Result<MigrateManagedBlocksReport> {
    paths::validate_program_name(request.program_name)?;
    let target_path =
        resolve_target_path(env, &request).map_err(|error| map_resolve_error(&request, error))?;
    validate_migration_target(&request, &target_path)?;

    let mut affected_locations = Vec::new();
    push_unique(&mut affected_locations, target_path.clone());

    let outcome = shell::migrate_managed_blocks(
        env,
        &request.shell,
        request.program_name,
        &target_path,
        &request.legacy_blocks,
    )
    .map_err(|error| map_migration_error(&request, &target_path, error))?;

    for path in outcome.affected_locations {
        push_unique(&mut affected_locations, path);
    }

    Ok(MigrateManagedBlocksReport {
        shell: request.shell,
        target_path,
        location: outcome.location,
        legacy_change: outcome.legacy_change,
        managed_change: outcome.managed_change,
        affected_locations,
    })
}

fn resolve_target_path(
    env: &Environment,
    request: &MigrateManagedBlocksRequest<'_>,
) -> Result<PathBuf> {
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

fn map_resolve_error(request: &MigrateManagedBlocksRequest<'_>, error: Error) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: request.path_override.as_deref(),
                affected_locations: Vec::new(),
                kind: FailureKind::MissingHome,
            },
            "Could not resolve the managed completion path for block migration because HOME is not set.",
            Some(
                "Set HOME for the current process or pass `path_override` so shellcomp can resolve the target completion path."
                    .to_owned(),
            ),
        ),
        Error::PathHasNoParent { path } => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: Some(&path),
                affected_locations: vec![path.clone()],
                kind: FailureKind::InvalidTargetPath,
            },
            format!(
                "The requested migration path `{}` does not have a parent directory.",
                path.display()
            ),
            Some(
                "Pass a file path with a real parent directory so shellcomp can build the replacement managed block."
                    .to_owned(),
            ),
        ),
        Error::UnsupportedShell(shell) => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
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

fn map_migration_error(
    request: &MigrateManagedBlocksRequest<'_>,
    target_path: &std::path::Path,
    error: Error,
) -> Error {
    match error {
        Error::MissingHome => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: FailureKind::MissingHome,
            },
            format!(
                "Could not resolve the managed {} startup file during block migration because HOME is not set.",
                request.shell
            ),
            Some(
                "Set HOME for the current process or rewrite the startup block manually."
                    .to_owned(),
            ),
        ),
        Error::Io { path, .. } | Error::InvalidUtf8File { path } => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileUnavailable,
            },
            format!(
                "Could not rewrite the managed {} startup file during block migration.",
                request.shell
            ),
            Some(
                "Review the relevant shell startup file manually and remove or replace the legacy block yourself."
                    .to_owned(),
            ),
        ),
        Error::ManagedBlockMissingEnd { path, .. } => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path.clone()],
                kind: FailureKind::ProfileCorrupted,
            },
            format!(
                "Could not rewrite the managed {} startup file because a managed block is malformed.",
                request.shell
            ),
            Some(
                "Repair or remove the malformed block manually, then re-run migration."
                    .to_owned(),
            ),
        ),
        Error::NonUtf8Path { path } => failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf(), path],
                kind: FailureKind::InvalidTargetPath,
            },
            "The target completion path could not be represented safely for managed block migration.",
            Some(
                "Move the completion file to a UTF-8 path before migrating managed shell blocks."
                    .to_owned(),
            ),
        ),
        other => other,
    }
}

fn validate_migration_target(
    request: &MigrateManagedBlocksRequest<'_>,
    target_path: &std::path::Path,
) -> Result<()> {
    if matches!(request.shell, crate::Shell::Zsh)
        && !zsh_target_is_autoloadable(request.program_name, target_path)
    {
        let expected = format!("_{}", request.program_name);
        return Err(failure(
            FailureContext {
                operation: Operation::MigrateManagedBlocks,
                shell: &request.shell,
                target_path: Some(target_path),
                affected_locations: vec![target_path.to_path_buf()],
                kind: FailureKind::InvalidTargetPath,
            },
            format!(
                "The requested zsh migration target `{}` is not autoloadable because its file name is not `{expected}`.",
                target_path.display()
            ),
            Some(format!(
                "Rename the completion file to `{expected}` or choose an autoloadable target path before migrating managed zsh blocks."
            )),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::execute;
    use crate::infra::env::Environment;
    use crate::model::{
        FileChange, LegacyManagedBlock, MigrateManagedBlocksRequest, Operation, Shell,
    };

    #[test]
    fn migrate_rewrites_legacy_bash_block() {
        let temp_root = crate::tests::temp_dir("migrate-bash-legacy");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bashrc"),
            "# >>> legacy bash >>>\n. '/tmp/tool'\n# <<< legacy bash <<<\n",
        )
        .expect(".bashrc should be writable");

        let report = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: None,
                legacy_blocks: vec![LegacyManagedBlock {
                    start_marker: "# >>> legacy bash >>>".to_owned(),
                    end_marker: "# <<< legacy bash <<<".to_owned(),
                }],
            },
        )
        .expect("migration should succeed");

        assert_eq!(report.legacy_change, FileChange::Removed);
        assert!(matches!(
            report.location.as_deref(),
            Some(path) if path.ends_with(".bashrc")
        ));
        let bashrc =
            fs::read_to_string(home.join(".bashrc")).expect(".bashrc should remain readable");
        assert!(bashrc.contains("shellcomp bash tool"));
        assert!(!bashrc.contains("legacy bash"));
    }

    #[test]
    fn migrate_returns_noop_for_fish() {
        let temp_root = crate::tests::temp_dir("migrate-fish-noop");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_CONFIG_HOME")
            .without_real_path_lookups();

        let report = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Fish,
                program_name: "tool",
                path_override: None,
                legacy_blocks: vec![LegacyManagedBlock {
                    start_marker: "# >>> legacy fish >>>".to_owned(),
                    end_marker: "# <<< legacy fish <<<".to_owned(),
                }],
            },
        )
        .expect("migration should succeed");

        assert_eq!(report.legacy_change, FileChange::Absent);
        assert_eq!(report.managed_change, FileChange::Absent);
        assert!(report.location.is_none());
    }

    #[test]
    fn migrate_returns_structured_failure_for_invalid_path() {
        let error = execute(
            &Environment::test().without_real_path_lookups(),
            MigrateManagedBlocksRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: Some(PathBuf::from("/")),
                legacy_blocks: Vec::new(),
            },
        )
        .expect_err("migration should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::MigrateManagedBlocks);
                assert_eq!(report.kind, crate::FailureKind::InvalidTargetPath);
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn migrate_rejects_non_autoloadable_zsh_target_without_rewriting_legacy_block() {
        let temp_root = crate::tests::temp_dir("migrate-zsh-non-autoloadable");
        let home = temp_root.join("home");
        let zshrc = home.join(".zshrc");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            &zshrc,
            "# >>> legacy zsh >>>\nfpath=(/tmp/tool $fpath)\n# <<< legacy zsh <<<\n",
        )
        .expect(".zshrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();
        let target = temp_root.join("custom").join("tool.zsh");

        let error = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Zsh,
                program_name: "tool",
                path_override: Some(target.clone()),
                legacy_blocks: vec![LegacyManagedBlock {
                    start_marker: "# >>> legacy zsh >>>".to_owned(),
                    end_marker: "# <<< legacy zsh <<<".to_owned(),
                }],
            },
        )
        .expect_err("migration should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::MigrateManagedBlocks);
                assert_eq!(report.kind, crate::FailureKind::InvalidTargetPath);
                assert_eq!(report.target_path, Some(target));
            }
            other => panic!("unexpected error variant: {other}"),
        }

        let rendered = fs::read_to_string(zshrc).expect(".zshrc should remain readable");
        assert!(rendered.contains("legacy zsh"));
        assert!(!rendered.contains("shellcomp zsh tool"));
    }

    #[cfg(unix)]
    #[test]
    fn migrate_keeps_legacy_bash_block_when_target_path_is_non_utf8() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp_root = crate::tests::temp_dir("migrate-bash-non-utf8-target");
        let home = temp_root.join("home");
        let bashrc = home.join(".bashrc");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            &bashrc,
            "# >>> legacy bash >>>\n. '/tmp/tool'\n# <<< legacy bash <<<\n",
        )
        .expect(".bashrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();
        let target = temp_root.join(OsString::from_vec(b"tool-\xff".to_vec()));

        let error = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: Some(target.clone()),
                legacy_blocks: vec![LegacyManagedBlock {
                    start_marker: "# >>> legacy bash >>>".to_owned(),
                    end_marker: "# <<< legacy bash <<<".to_owned(),
                }],
            },
        )
        .expect_err("migration should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::MigrateManagedBlocks);
                assert_eq!(report.kind, crate::FailureKind::InvalidTargetPath);
                assert_eq!(report.target_path, Some(target));
            }
            other => panic!("unexpected error variant: {other}"),
        }

        let rendered = fs::read_to_string(bashrc).expect(".bashrc should remain readable");
        assert!(rendered.contains("legacy bash"));
        assert!(!rendered.contains("shellcomp bash tool"));
    }

    #[cfg(unix)]
    #[test]
    fn migrate_keeps_legacy_zsh_block_when_parent_directory_is_non_utf8() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp_root = crate::tests::temp_dir("migrate-zsh-non-utf8-parent");
        let home = temp_root.join("home");
        let zshrc = home.join(".zshrc");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            &zshrc,
            "# >>> legacy zsh >>>\nfpath=(/tmp/tool $fpath)\n# <<< legacy zsh <<<\n",
        )
        .expect(".zshrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();
        let target = temp_root
            .join(OsString::from_vec(b"zfunc-\xff".to_vec()))
            .join("_tool");

        let error = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Zsh,
                program_name: "tool",
                path_override: Some(target.clone()),
                legacy_blocks: vec![LegacyManagedBlock {
                    start_marker: "# >>> legacy zsh >>>".to_owned(),
                    end_marker: "# <<< legacy zsh <<<".to_owned(),
                }],
            },
        )
        .expect_err("migration should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::MigrateManagedBlocks);
                assert_eq!(report.kind, crate::FailureKind::InvalidTargetPath);
                assert_eq!(report.target_path, Some(target));
            }
            other => panic!("unexpected error variant: {other}"),
        }

        let rendered = fs::read_to_string(zshrc).expect(".zshrc should remain readable");
        assert!(rendered.contains("legacy zsh"));
        assert!(!rendered.contains("shellcomp zsh tool"));
    }

    #[test]
    fn migrate_does_not_partially_remove_legacy_blocks_when_a_later_one_is_malformed() {
        let temp_root = crate::tests::temp_dir("migrate-bash-legacy-atomic");
        let home = temp_root.join("home");
        let bashrc = home.join(".bashrc");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            &bashrc,
            "# >>> legacy one >>>\n. '/tmp/one'\n# <<< legacy one <<<\n# >>> legacy two >>>\n. '/tmp/two'\n",
        )
        .expect(".bashrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: None,
                legacy_blocks: vec![
                    LegacyManagedBlock {
                        start_marker: "# >>> legacy one >>>".to_owned(),
                        end_marker: "# <<< legacy one <<<".to_owned(),
                    },
                    LegacyManagedBlock {
                        start_marker: "# >>> legacy two >>>".to_owned(),
                        end_marker: "# <<< legacy two <<<".to_owned(),
                    },
                ],
            },
        )
        .expect_err("migration should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::MigrateManagedBlocks);
                assert_eq!(report.kind, crate::FailureKind::ProfileCorrupted);
            }
            other => panic!("unexpected error variant: {other}"),
        }

        let rendered = fs::read_to_string(bashrc).expect(".bashrc should remain readable");
        assert!(rendered.contains("legacy one"));
        assert!(rendered.contains("legacy two"));
        assert!(!rendered.contains("shellcomp bash tool"));
    }

    #[test]
    fn migrate_does_not_remove_legacy_block_when_existing_shellcomp_block_is_malformed() {
        let temp_root = crate::tests::temp_dir("migrate-bash-existing-shellcomp-corrupt");
        let home = temp_root.join("home");
        let bashrc = home.join(".bashrc");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            &bashrc,
            "# >>> legacy bash >>>\n. '/tmp/tool'\n# <<< legacy bash <<<\n# >>> shellcomp bash tool >>>\nif [ -f '/tmp/bad' ]; then\n  . '/tmp/bad'\nfi\n",
        )
        .expect(".bashrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error = execute(
            &env,
            MigrateManagedBlocksRequest {
                shell: Shell::Bash,
                program_name: "tool",
                path_override: None,
                legacy_blocks: vec![LegacyManagedBlock {
                    start_marker: "# >>> legacy bash >>>".to_owned(),
                    end_marker: "# <<< legacy bash <<<".to_owned(),
                }],
            },
        )
        .expect_err("migration should fail");

        match error {
            crate::Error::Failure(report) => {
                assert_eq!(report.operation, Operation::MigrateManagedBlocks);
                assert_eq!(report.kind, crate::FailureKind::ProfileCorrupted);
            }
            other => panic!("unexpected error variant: {other}"),
        }

        let rendered = fs::read_to_string(bashrc).expect(".bashrc should remain readable");
        assert!(rendered.contains("legacy bash"));
        assert!(rendered.contains("shellcomp bash tool"));
        assert!(!rendered.contains(".local/share/bash-completion/completions/tool"));
    }
}
