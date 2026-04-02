use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::{
    env::Environment,
    fs,
    managed_block::{self, ManagedBlock},
    paths,
};
use crate::model::{ActivationMode, ActivationReport, Availability, CleanupReport, Shell};
use crate::shell::{ActivationOutcome, CleanupOutcome};

const BASH_LOADER_PATHS: &[&str] = &[
    "/usr/share/bash-completion/bash_completion",
    "/etc/bash_completion",
    "/usr/local/share/bash-completion/bash_completion",
    "/usr/local/etc/profile.d/bash_completion.sh",
    "/opt/homebrew/etc/profile.d/bash_completion.sh",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoaderStatus {
    ActiveNow,
    WiredInBashrc,
    PresentButUnwired,
    Absent,
}

pub(crate) fn install(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationOutcome> {
    let rc_path = bashrc_path(env)?;
    let can_use_system_loader = target_uses_system_loader_path(env, program_name, target_path);

    let loader_status = if can_use_system_loader {
        Some(loader_status(env, &rc_path)?)
    } else {
        None
    };

    if let Some(loader_status) = &loader_status {
        match loader_status {
            LoaderStatus::ActiveNow => {
                return Ok(ActivationOutcome {
                    report: ActivationReport {
                        mode: ActivationMode::SystemLoader,
                        availability: Availability::ActiveNow,
                        location: Some(target_path.to_path_buf()),
                        reason: Some(
                            "Detected an active bash-completion loader in the current shell."
                                .to_owned(),
                        ),
                        next_step: None,
                    },
                    affected_locations: Vec::new(),
                });
            }
            LoaderStatus::WiredInBashrc => {
                return Ok(ActivationOutcome {
                    report: ActivationReport {
                        mode: ActivationMode::SystemLoader,
                        availability: Availability::AvailableAfterNewShell,
                        location: Some(rc_path.clone()),
                        reason: Some(
                            "Detected ~/.bashrc wiring for a system bash-completion loader."
                                .to_owned(),
                        ),
                        next_step: Some(
                            "Start a new Bash session to ensure the completion script is loaded."
                                .to_owned(),
                        ),
                    },
                    affected_locations: vec![rc_path],
                });
            }
            LoaderStatus::PresentButUnwired | LoaderStatus::Absent => {}
        }
    }

    let block = managed_block(program_name, target_path)?;
    managed_block::upsert(&rc_path, &block)?;

    Ok(ActivationOutcome {
        report: ActivationReport {
            mode: ActivationMode::ManagedRcBlock,
            availability: Availability::AvailableAfterSource,
            location: Some(rc_path.clone()),
            reason: Some(match loader_status {
                Some(LoaderStatus::PresentButUnwired) => {
                    "A known bash-completion loader file exists on disk, but ~/.bashrc does not appear to source it, so shellcomp added a managed block to ~/.bashrc.".to_owned()
                }
                Some(LoaderStatus::Absent | LoaderStatus::ActiveNow | LoaderStatus::WiredInBashrc) => {
                    "No system bash-completion loader was detected, so shellcomp added a managed block to ~/.bashrc."
                        .to_owned()
                }
                None => {
                    "Installed to a custom Bash completion path, so shellcomp added a managed block to ~/.bashrc to source it directly."
                        .to_owned()
                }
            }),
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
    let rc_path = bashrc_path(env)?;
    let can_use_system_loader = target_uses_system_loader_path(env, program_name, target_path);
    let loader_status = if can_use_system_loader {
        Some(loader_status(env, &rc_path)?)
    } else {
        None
    };

    if !fs::file_exists(target_path) {
        let mode = if matches!(
            loader_status,
            Some(LoaderStatus::ActiveNow | LoaderStatus::WiredInBashrc)
        ) {
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

    if let Some(loader_status) = loader_status {
        match loader_status {
            LoaderStatus::ActiveNow => {
                return Ok(ActivationReport {
                    mode: ActivationMode::SystemLoader,
                    availability: Availability::ActiveNow,
                    location: Some(target_path.to_path_buf()),
                    reason: Some(
                        "Detected an active bash-completion loader in the current shell."
                            .to_owned(),
                    ),
                    next_step: None,
                });
            }
            LoaderStatus::WiredInBashrc => {
                return Ok(ActivationReport {
                    mode: ActivationMode::SystemLoader,
                    availability: Availability::AvailableAfterNewShell,
                    location: Some(rc_path.clone()),
                    reason: Some(
                        "Detected ~/.bashrc wiring for a system bash-completion loader.".to_owned(),
                    ),
                    next_step: Some(
                        "Start a new Bash session if completions are not available yet.".to_owned(),
                    ),
                });
            }
            LoaderStatus::PresentButUnwired | LoaderStatus::Absent => {}
        }
    }

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

fn target_uses_system_loader_path(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
) -> bool {
    paths::default_install_path(env, &Shell::Bash, program_name)
        .map(|default_path| default_path == target_path)
        .unwrap_or(false)
}

fn loader_status(env: &Environment, bashrc_path: &Path) -> Result<LoaderStatus> {
    if loader_active_now(env) {
        return Ok(LoaderStatus::ActiveNow);
    }

    if bashrc_sources_loader(env, bashrc_path)? {
        return Ok(LoaderStatus::WiredInBashrc);
    }

    if loader_file_present(env) {
        return Ok(LoaderStatus::PresentButUnwired);
    }

    Ok(LoaderStatus::Absent)
}

fn loader_file_present(env: &Environment) -> bool {
    BASH_LOADER_PATHS
        .iter()
        .any(|path| env.path_exists(Path::new(path)))
}

fn bashrc_sources_loader(env: &Environment, bashrc_path: &Path) -> Result<bool> {
    let Some(contents) = read_utf8_file_if_exists(bashrc_path)? else {
        return Ok(false);
    };

    Ok(BASH_LOADER_PATHS
        .iter()
        .filter(|path| env.path_exists(Path::new(path)))
        .any(|path| contents.lines().any(|line| line_sources_loader(line, path))))
}

fn line_sources_loader(line: &str, loader_path: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }

    trimmed
        .split("&&")
        .flat_map(|segment| segment.split(';'))
        .map(str::trim)
        .map(|command| command.strip_prefix("then ").unwrap_or(command))
        .map(|command| command.strip_prefix("do ").unwrap_or(command))
        .any(|command| command_sources_loader(command, loader_path))
}

fn command_sources_loader(command: &str, loader_path: &str) -> bool {
    let Some(remainder) = command
        .strip_prefix("source ")
        .or_else(|| command.strip_prefix(". "))
    else {
        return false;
    };

    let Some(token) = remainder.split_whitespace().next() else {
        return false;
    };

    token == loader_path
        || token
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            == Some(loader_path)
        || token
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            == Some(loader_path)
}

fn read_utf8_file_if_exists(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(contents) => String::from_utf8(contents)
            .map(Some)
            .map_err(|_| Error::InvalidUtf8File {
                path: path.to_path_buf(),
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::io("read file", path, source)),
    }
}

fn loader_active_now(env: &Environment) -> bool {
    env.var_os("BASH_COMPLETION_VERSINFO").is_some()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{LoaderStatus, detect, install, loader_active_now, loader_status};
    use crate::infra::env::Environment;
    use crate::model::{ActivationMode, Availability};

    #[test]
    fn loader_status_is_active_when_env_hint_is_present() {
        let env = Environment::test().with_var("BASH_COMPLETION_VERSINFO", "2");
        let status =
            loader_status(&env, Path::new("/tmp/ignored-bashrc")).expect("status should resolve");

        assert_eq!(status, LoaderStatus::ActiveNow);
        assert!(loader_active_now(&env));
    }

    #[test]
    fn loader_status_is_present_but_unwired_when_only_loader_file_exists() {
        let env = Environment::test()
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion");
        let status =
            loader_status(&env, Path::new("/tmp/ignored-bashrc")).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_is_wired_when_bashrc_sources_known_loader() {
        let temp_root = crate::tests::temp_dir("bash-loader-wired");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bashrc"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();
        let status = loader_status(&env, &home.join(".bashrc")).expect("status should resolve");

        assert_eq!(status, LoaderStatus::WiredInBashrc);
    }

    #[test]
    fn loader_status_ignores_comments_and_plain_strings() {
        let temp_root = crate::tests::temp_dir("bash-loader-ignore-comments");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bashrc"),
            "# source /usr/share/bash-completion/bash_completion\nBASH_LOADER=/usr/share/bash-completion/bash_completion\necho \"source /usr/share/bash-completion/bash_completion\"\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let status = loader_status(&env, &home.join(".bashrc")).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn install_uses_system_loader_only_when_bashrc_wiring_is_detected() {
        let temp_root = crate::tests::temp_dir("bash-loader-install-wired");
        let home = temp_root.join("home");
        let target = home.join(".local/share/bash-completion/completions/tool");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bashrc"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".bashrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = install(&env, "tool", &target).expect("install should succeed");

        assert_eq!(report.report.mode, ActivationMode::SystemLoader);
        assert_eq!(
            report.report.availability,
            Availability::AvailableAfterNewShell
        );
        assert_eq!(report.affected_locations, vec![home.join(".bashrc")]);
    }

    #[test]
    fn install_falls_back_to_managed_block_when_loader_file_is_not_wired() {
        let temp_root = crate::tests::temp_dir("bash-loader-install-fallback");
        let home = temp_root.join("home");
        let target = home.join(".local/share/bash-completion/completions/tool");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = install(&env, "tool", &target).expect("install should succeed");

        assert_eq!(report.report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(
            report.report.availability,
            Availability::AvailableAfterSource
        );
        let bashrc = fs::read_to_string(home.join(".bashrc")).expect(".bashrc should be created");
        assert!(bashrc.contains("shellcomp bash tool"));
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
    fn detect_does_not_assume_system_loader_from_loader_file_alone() {
        let temp_root = crate::tests::temp_dir("bash-loader-detect-fallback");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = detect(
            &env,
            "tool",
            &home.join(".local/share/bash-completion/completions/tool"),
        )
        .expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::ManualActionRequired);
    }

    #[test]
    fn detect_existing_completion_does_not_assume_system_loader_from_loader_file_alone() {
        let temp_root = crate::tests::temp_dir("bash-loader-detect-existing-fallback");
        let home = temp_root.join("home");
        let completion_path = home.join(".local/share/bash-completion/completions/tool");
        fs::create_dir_all(
            completion_path
                .parent()
                .expect("completion path should have a parent"),
        )
        .expect("completion dir should be creatable");
        fs::write(&completion_path, "complete -F _tool tool\n")
            .expect("completion file should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = detect(&env, "tool", &completion_path).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::ManualActionRequired);
    }

    #[test]
    fn detect_reports_system_loader_when_bashrc_wires_loader_for_new_shells() {
        let temp_root = crate::tests::temp_dir("bash-loader-detect-wired");
        let home = temp_root.join("home");
        let completion_path = home.join(".local/share/bash-completion/completions/tool");
        fs::create_dir_all(
            completion_path
                .parent()
                .expect("completion path should have a parent"),
        )
        .expect("completion dir should be creatable");
        fs::write(&completion_path, "complete -F _tool tool\n")
            .expect("completion file should be writable");
        fs::write(
            home.join(".bashrc"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = detect(&env, "tool", &completion_path).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::SystemLoader);
        assert_eq!(report.availability, Availability::AvailableAfterNewShell);
        assert_eq!(report.location, Some(home.join(".bashrc")));
    }

    #[test]
    fn install_reports_active_now_when_loader_is_active() {
        let temp_root = crate::tests::temp_dir("bash-loader-install-active");
        let home = temp_root.join("home");
        let target = home.join(".local/share/bash-completion/completions/tool");
        let env = Environment::test()
            .with_var("HOME", &home)
            .with_var("BASH_COMPLETION_VERSINFO", "2")
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = install(&env, "tool", &target).expect("install should succeed");

        assert_eq!(report.report.mode, ActivationMode::SystemLoader);
        assert_eq!(report.report.availability, Availability::ActiveNow);
        assert!(report.report.next_step.is_none());
    }

    #[test]
    fn custom_bash_path_uses_managed_block_even_when_loader_is_active() {
        let temp_root = crate::tests::temp_dir("bash-custom-path-managed");
        let home = temp_root.join("home");
        let target = temp_root.join("custom").join("tool.bash");
        let env = Environment::test()
            .with_var("HOME", &home)
            .with_var("BASH_COMPLETION_VERSINFO", "2")
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = install(&env, "tool", &target).expect("install should succeed");

        assert_eq!(report.report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(
            report.report.availability,
            Availability::AvailableAfterSource
        );
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
