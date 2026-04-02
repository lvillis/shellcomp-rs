use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::{
    env::Environment,
    fs,
    managed_block::{self, ManagedBlock},
};
use crate::model::{ActivationMode, ActivationReport, Availability, CleanupReport};
use crate::shell::{ActivationOutcome, CleanupOutcome};

const BASH_LOADER_PATHS: &[&str] = &[
    "/usr/share/bash-completion/bash_completion",
    "/etc/bash_completion",
    "/usr/local/share/bash-completion/bash_completion",
    "/usr/local/etc/profile.d/bash_completion.sh",
    "/opt/homebrew/etc/profile.d/bash_completion.sh",
];

pub(crate) fn install(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationOutcome> {
    if has_system_loader(env) {
        let availability = if loader_active_now(env) {
            Availability::ActiveNow
        } else {
            Availability::AvailableAfterNewShell
        };
        return Ok(ActivationOutcome {
            report: ActivationReport {
                mode: ActivationMode::SystemLoader,
                availability,
                location: Some(target_path.to_path_buf()),
                reason: Some(if loader_active_now(env) {
                    "Detected an active bash-completion loader in the current shell.".to_owned()
                } else {
                    "Detected a system bash-completion loader.".to_owned()
                }),
                next_step: match availability {
                    Availability::ActiveNow => None,
                    Availability::AvailableAfterNewShell => Some(
                        "Start a new Bash session to ensure the completion script is loaded."
                            .to_owned(),
                    ),
                    _ => None,
                },
            },
            affected_locations: Vec::new(),
        });
    }

    let rc_path = bashrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    managed_block::upsert(&rc_path, &block)?;

    Ok(ActivationOutcome {
        report: ActivationReport {
            mode: ActivationMode::ManagedRcBlock,
            availability: Availability::AvailableAfterSource,
            location: Some(rc_path.clone()),
            reason: Some(
                "No system bash-completion loader was detected, so shellcomp added a managed block to ~/.bashrc."
                    .to_owned(),
            ),
            next_step: Some(format!(
                "Run `source {}` or start a new Bash session.",
                rc_path.display()
            )),
        },
        affected_locations: vec![rc_path],
    })
}

pub(crate) fn uninstall(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
) -> Result<CleanupOutcome> {
    let rc_path = bashrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    let rc_change = managed_block::remove(&rc_path, &block)?;

    Ok(CleanupOutcome {
        cleanup: CleanupReport {
            mode: ActivationMode::ManagedRcBlock,
            change: rc_change,
            location: Some(rc_path.clone()),
            reason: Some(match rc_change {
                crate::FileChange::Removed => {
                    "Removed the managed Bash activation block from ~/.bashrc.".to_owned()
                }
                crate::FileChange::Absent => {
                    "No managed Bash activation block was present in ~/.bashrc.".to_owned()
                }
                _ => "Bash activation cleanup completed.".to_owned(),
            }),
            next_step: None,
        },
        affected_locations: vec![rc_path],
    })
}

pub(crate) fn detect(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationReport> {
    if !fs::file_exists(target_path) {
        let mode = if has_system_loader(env) {
            ActivationMode::SystemLoader
        } else {
            ActivationMode::ManagedRcBlock
        };
        return Ok(ActivationReport {
            mode,
            availability: Availability::ManualActionRequired,
            location: Some(target_path.to_path_buf()),
            reason: Some(format!(
                "Completion file `{}` is not installed.",
                target_path.display()
            )),
            next_step: Some(
                "Run your CLI's completion install command or install the script manually."
                    .to_owned(),
            ),
        });
    }

    if has_system_loader(env) {
        let availability = if loader_active_now(env) {
            Availability::ActiveNow
        } else {
            Availability::AvailableAfterNewShell
        };
        return Ok(ActivationReport {
            mode: ActivationMode::SystemLoader,
            availability,
            location: Some(target_path.to_path_buf()),
            reason: Some(if loader_active_now(env) {
                "Detected an active bash-completion loader in the current shell.".to_owned()
            } else {
                "Detected a system bash-completion loader.".to_owned()
            }),
            next_step: match availability {
                Availability::ActiveNow => None,
                Availability::AvailableAfterNewShell => Some(
                    "Start a new Bash session if completions are not available yet.".to_owned(),
                ),
                _ => None,
            },
        });
    }

    let rc_path = bashrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    let wired = managed_block::matches(&rc_path, &block)?;

    Ok(ActivationReport {
        mode: ActivationMode::ManagedRcBlock,
        availability: if wired {
            Availability::AvailableAfterSource
        } else {
            Availability::ManualActionRequired
        },
        location: Some(rc_path),
        reason: Some(if wired {
            "Managed Bash activation block is present in ~/.bashrc.".to_owned()
        } else {
            "Completion file exists, but the managed Bash activation block was not found."
                .to_owned()
        }),
        next_step: Some(if wired {
            "Run `source ~/.bashrc` or start a new Bash session.".to_owned()
        } else {
            "Re-run installation or source the completion file from ~/.bashrc manually.".to_owned()
        }),
    })
}

fn managed_block(program_name: &str, target_path: &Path) -> Result<ManagedBlock> {
    let quoted = shell_quote(target_path)?;
    Ok(ManagedBlock {
        start_marker: format!("# >>> shellcomp bash {program_name} >>>"),
        end_marker: format!("# <<< shellcomp bash {program_name} <<<"),
        body: format!("if [ -f {quoted} ]; then\n  . {quoted}\nfi"),
    })
}

fn shell_quote(path: &Path) -> Result<String> {
    let value = path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: path.to_path_buf(),
    })?;
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn bashrc_path(env: &Environment) -> Result<PathBuf> {
    Ok(env.home_dir()?.join(".bashrc"))
}

fn has_system_loader(env: &Environment) -> bool {
    if loader_active_now(env) {
        return true;
    }
    BASH_LOADER_PATHS
        .iter()
        .any(|path| env.path_exists(Path::new(path)))
}

fn loader_active_now(env: &Environment) -> bool {
    env.var_os("BASH_COMPLETION_VERSINFO").is_some()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{detect, has_system_loader, install, loader_active_now};
    use crate::infra::env::Environment;
    use crate::model::{ActivationMode, Availability};

    #[test]
    fn loader_is_detected_from_env_hint() {
        let env = Environment::test().with_var("BASH_COMPLETION_VERSINFO", "2");
        assert!(has_system_loader(&env));
        assert!(loader_active_now(&env));
    }

    #[test]
    fn loader_is_detected_from_known_path_probe() {
        let env = Environment::test()
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion");
        assert!(has_system_loader(&env));
    }

    #[test]
    fn install_uses_system_loader_when_probe_matches() {
        let target = Path::new("/tmp/tool");
        let env = Environment::test()
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion");

        let report = install(&env, "tool", target).expect("install should succeed");

        assert_eq!(report.report.mode, ActivationMode::SystemLoader);
        assert_eq!(
            report.report.availability,
            Availability::AvailableAfterNewShell
        );
        assert!(report.affected_locations.is_empty());
    }

    #[test]
    fn detect_requires_manual_action_when_file_missing() {
        let env = Environment::test()
            .without_var("BASH_COMPLETION_VERSINFO")
            .without_existing_path("/usr/share/bash-completion/bash_completion")
            .with_var("HOME", "/tmp/test-home")
            .without_real_path_lookups();

        let report =
            detect(&env, "tool", Path::new("/tmp/missing")).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::ManualActionRequired);
    }

    #[test]
    fn install_reports_active_now_when_loader_is_active() {
        let target = Path::new("/tmp/tool");
        let env = Environment::test().with_var("BASH_COMPLETION_VERSINFO", "2");

        let report = install(&env, "tool", target).expect("install should succeed");

        assert_eq!(report.report.mode, ActivationMode::SystemLoader);
        assert_eq!(report.report.availability, Availability::ActiveNow);
        assert!(report.report.next_step.is_none());
    }

    #[test]
    fn install_errors_when_bashrc_is_not_writable() {
        let temp_root = crate::tests::temp_dir("install-bash-manual-fallback");
        let home = temp_root.join("home");
        std::fs::create_dir_all(home.join(".bashrc")).expect("directory should be creatable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let error = install(
            &env,
            "tool",
            &home.join(".local/share/bash-completion/completions/tool"),
        )
        .expect_err("install should return an error");

        assert!(matches!(error, crate::Error::Io { .. }));
    }
}
