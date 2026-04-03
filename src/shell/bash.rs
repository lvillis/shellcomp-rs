use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::{
    env::Environment,
    fs,
    managed_block::{self, ManagedBlock},
    paths,
};
use crate::model::LegacyManagedBlock;
use crate::model::{ActivationMode, ActivationReport, Availability, CleanupReport, Shell};
use crate::shell::{ActivationOutcome, CleanupOutcome, MigrationOutcome, migrate_profile_blocks};

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
    WiredInStartup(PathBuf),
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
        Some(loader_status(env)?)
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
            LoaderStatus::WiredInStartup(startup_path) => {
                return Ok(ActivationOutcome {
                    report: ActivationReport {
                        mode: ActivationMode::SystemLoader,
                        availability: Availability::AvailableAfterNewShell,
                        location: Some(startup_path.clone()),
                        reason: Some(format!(
                            "Detected startup-file wiring for a system bash-completion loader in `{}`.",
                            startup_path.display()
                        )),
                        next_step: Some(
                            "Start a new Bash session to ensure the completion script is loaded."
                                .to_owned(),
                        ),
                    },
                    affected_locations: vec![startup_path.clone()],
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
                Some(LoaderStatus::Absent | LoaderStatus::ActiveNow | LoaderStatus::WiredInStartup(_)) => {
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
                shell_quote(&rc_path)?
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
    let can_use_system_loader = target_uses_system_loader_path(env, program_name, target_path);
    let loader_status = if can_use_system_loader {
        Some(loader_status(env)?)
    } else {
        None
    };
    let rc_change = managed_block::remove(&rc_path, &block)?;

    let (mode, location, reason, mut affected_locations) = match (&rc_change, loader_status) {
        (crate::FileChange::Absent, Some(LoaderStatus::ActiveNow)) => (
            ActivationMode::SystemLoader,
            None,
            "No shellcomp-managed Bash activation block was present; Bash completion relied on an active system loader."
                .to_owned(),
            vec![rc_path.clone()],
        ),
        (crate::FileChange::Absent, Some(LoaderStatus::WiredInStartup(startup_path))) => (
            ActivationMode::SystemLoader,
            Some(startup_path.clone()),
            format!(
                "No shellcomp-managed Bash activation block was present; Bash completion was wired through the system loader in `{}`.",
                startup_path.display()
            ),
            vec![rc_path.clone(), startup_path],
        ),
        _ => (
            ActivationMode::ManagedRcBlock,
            Some(rc_path.clone()),
            match rc_change {
                crate::FileChange::Removed => {
                    "Removed the managed Bash activation block from ~/.bashrc.".to_owned()
                }
                crate::FileChange::Absent => {
                    "No managed Bash activation block was present in ~/.bashrc.".to_owned()
                }
                _ => "Bash activation cleanup completed.".to_owned(),
            },
            vec![rc_path.clone()],
        ),
    };

    Ok(CleanupOutcome {
        cleanup: CleanupReport {
            mode,
            change: rc_change,
            location,
            reason: Some(reason),
            next_step: None,
        },
        affected_locations: {
            affected_locations.shrink_to_fit();
            affected_locations
        },
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
        Some(loader_status(env)?)
    } else {
        None
    };
    let block = managed_block(program_name, target_path)?;
    let wired = managed_block::matches(&rc_path, &block)?;

    if !fs::file_exists(target_path) {
        let mode = if matches!(
            loader_status,
            Some(LoaderStatus::ActiveNow | LoaderStatus::WiredInStartup(_))
        ) {
            ActivationMode::SystemLoader
        } else {
            ActivationMode::ManagedRcBlock
        };
        return Ok(ActivationReport {
            mode,
            availability: Availability::ManualActionRequired,
            location: Some(if matches!(mode, ActivationMode::ManagedRcBlock) && wired {
                rc_path.clone()
            } else {
                target_path.to_path_buf()
            }),
            reason: Some(if matches!(mode, ActivationMode::ManagedRcBlock) && wired {
                format!(
                    "Managed Bash activation block is present in ~/.bashrc, but completion file `{}` is not installed.",
                    target_path.display()
                )
            } else {
                format!(
                    "Completion file `{}` is not installed.",
                    target_path.display()
                )
            }),
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
            LoaderStatus::WiredInStartup(startup_path) => {
                return Ok(ActivationReport {
                    mode: ActivationMode::SystemLoader,
                    availability: Availability::AvailableAfterNewShell,
                    location: Some(startup_path.clone()),
                    reason: Some(format!(
                        "Detected startup-file wiring for a system bash-completion loader in `{}`.",
                        startup_path.display()
                    )),
                    next_step: Some(
                        "Start a new Bash session if completions are not available yet.".to_owned(),
                    ),
                });
            }
            LoaderStatus::PresentButUnwired | LoaderStatus::Absent => {}
        }
    }

    let quoted_rc_path = shell_quote(&rc_path)?;
    let quoted_target_path = shell_quote(target_path)?;

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
            format!("Run `source {quoted_rc_path}` or start a new Bash session.")
        } else {
            format!(
                "Re-run installation or source {quoted_target_path} from {quoted_rc_path} manually."
            )
        }),
    })
}

pub(crate) fn migrate(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
    legacy_blocks: &[LegacyManagedBlock],
) -> Result<MigrationOutcome> {
    let rc_path = bashrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    let (legacy_change, managed_change) = migrate_profile_blocks(&rc_path, legacy_blocks, &block)?;

    Ok(MigrationOutcome {
        location: Some(rc_path.clone()),
        managed_change,
        legacy_change,
        affected_locations: vec![rc_path],
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

fn loader_status(env: &Environment) -> Result<LoaderStatus> {
    if loader_active_now(env) {
        return Ok(LoaderStatus::ActiveNow);
    }

    if let Some(startup_path) = startup_file_wiring(env)? {
        return Ok(LoaderStatus::WiredInStartup(startup_path));
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

fn startup_file_wiring(env: &Environment) -> Result<Option<PathBuf>> {
    for startup_path in startup_files(env)? {
        if let Some(wired_path) = file_reaches_loader(env, &startup_path, &mut BTreeSet::new())? {
            return Ok(Some(wired_path));
        }
    }

    Ok(None)
}

fn startup_files(env: &Environment) -> Result<Vec<PathBuf>> {
    let mut files = vec![PathBuf::from("/etc/bash.bashrc")];
    if let Ok(home) = env.home_dir() {
        push_unique_path(&mut files, home.join(".bashrc"));
    }

    push_unique_path(&mut files, PathBuf::from("/etc/profile"));
    if let Ok(home) = env.home_dir()
        && let Some(login_file) = first_existing_login_startup_file(env, &home)?
    {
        push_unique_path(&mut files, login_file);
    }

    Ok(files)
}

fn first_existing_login_startup_file(env: &Environment, home: &Path) -> Result<Option<PathBuf>> {
    for candidate in [
        home.join(".bash_profile"),
        home.join(".bash_login"),
        home.join(".profile"),
    ] {
        if env.read_file_if_exists(&candidate)?.is_some() {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn file_reaches_loader(
    env: &Environment,
    startup_path: &Path,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<Option<PathBuf>> {
    if !visited.insert(startup_path.to_path_buf()) {
        return Ok(None);
    }

    let Some(contents) = read_utf8_file_if_exists(env, startup_path)? else {
        return Ok(None);
    };

    if BASH_LOADER_PATHS
        .iter()
        .filter(|path| env.path_exists(Path::new(path)))
        .any(|path| contents.lines().any(|line| line_sources_loader(line, path)))
    {
        return Ok(Some(startup_path.to_path_buf()));
    }

    for profile_dir in [
        Path::new("/etc/profile.d"),
        Path::new("/usr/local/etc/profile.d"),
        Path::new("/opt/homebrew/etc/profile.d"),
    ] {
        for target in direct_profile_script_targets(&contents, profile_dir) {
            if let Some(wired_path) = file_reaches_loader(env, &target, visited)? {
                return Ok(Some(wired_path));
            }
        }

        let walk_mode = contents
            .lines()
            .find_map(|line| line_walks_profile_dir(line, profile_dir));
        if let Some(walk_mode) = walk_mode {
            let mut entries = env.read_dir_entries(profile_dir)?;
            entries.sort();
            for entry in entries {
                if !profile_dir_entry_matches_walk_mode(&entry, walk_mode) {
                    continue;
                }

                if let Some(wired_path) = file_reaches_loader(env, &entry, visited)? {
                    return Ok(Some(wired_path));
                }
            }
        }
    }

    for target in contents
        .lines()
        .flat_map(line_source_targets)
        .filter_map(|target| resolve_sourced_path(env, target))
    {
        if let Some(wired_path) = file_reaches_loader(env, &target, visited)? {
            return Ok(Some(wired_path));
        }
    }

    Ok(None)
}

fn resolve_sourced_path(env: &Environment, target: &str) -> Option<PathBuf> {
    if Path::new(target).is_absolute() {
        return Some(PathBuf::from(target));
    }

    let home = env.home_dir().ok()?;
    if let Some(path) = target.strip_prefix("~/") {
        return Some(home.join(path));
    }
    if let Some(path) = target.strip_prefix("$HOME/") {
        return Some(home.join(path));
    }
    if let Some(path) = target.strip_prefix("${HOME}/") {
        return Some(home.join(path));
    }

    None
}

fn line_sources_loader(line: &str, loader_path: &str) -> bool {
    line_source_targets(line).contains(&loader_path)
}

fn direct_profile_script_targets(contents: &str, profile_dir: &Path) -> Vec<PathBuf> {
    contents
        .lines()
        .flat_map(line_source_targets)
        .map(PathBuf::from)
        .filter(|target| target.starts_with(profile_dir))
        .collect()
}

#[derive(Clone, Copy)]
enum ProfileDirWalkMode {
    GlobSh,
    RunParts,
}

fn line_walks_profile_dir(line: &str, profile_dir: &Path) -> Option<ProfileDirWalkMode> {
    let profile_dir = profile_dir.to_str()?;
    let glob = format!("{profile_dir}/*.sh");
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    for segment in trimmed.split("&&").flat_map(|segment| segment.split(';')) {
        let command = normalize_shell_command(segment);
        let tokens: Vec<_> = command
            .split_whitespace()
            .map(unquote_shell_token)
            .collect();
        let looks_like_for_glob_walk = tokens.first().is_some_and(|token| *token == "for")
            && tokens.contains(&"in")
            && tokens.iter().any(|token| *token == glob);
        if looks_like_for_glob_walk {
            return Some(ProfileDirWalkMode::GlobSh);
        }
        if tokens.first().is_some_and(|token| *token == "run-parts")
            && tokens.iter().skip(1).any(|token| *token == profile_dir)
        {
            return Some(ProfileDirWalkMode::RunParts);
        }
    }

    None
}

fn profile_dir_entry_matches_walk_mode(path: &Path, mode: ProfileDirWalkMode) -> bool {
    match mode {
        ProfileDirWalkMode::GlobSh => {
            path.extension().and_then(|value| value.to_str()) == Some("sh")
        }
        ProfileDirWalkMode::RunParts => path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| !value.contains('.')),
    }
}

fn line_source_targets(line: &str) -> Vec<&str> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Vec::new();
    }

    trimmed
        .split("&&")
        .flat_map(|segment| segment.split(';'))
        .map(normalize_shell_command)
        .filter_map(command_source_target)
        .collect()
}

fn command_source_target(command: &str) -> Option<&str> {
    let mut tokens = command.split_whitespace();
    match tokens.next()? {
        "source" | "." => tokens.next().map(unquote_shell_token),
        _ => None,
    }
    .filter(|value| !value.is_empty())
}

fn normalize_shell_command(command: &str) -> &str {
    let command = command.trim();
    let command = strip_shell_keyword(command, "then");
    strip_shell_keyword(command, "do")
}

fn strip_shell_keyword<'a>(command: &'a str, keyword: &str) -> &'a str {
    let Some(remainder) = command.strip_prefix(keyword) else {
        return command;
    };
    let Some(next) = remainder.chars().next() else {
        return command;
    };
    if next.is_whitespace() {
        remainder.trim_start()
    } else {
        command
    }
}

fn unquote_shell_token(token: &str) -> &str {
    token
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            token
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(token)
}

fn read_utf8_file_if_exists(env: &Environment, path: &Path) -> Result<Option<String>> {
    match env.read_file_if_exists(path)? {
        Some(contents) => {
            String::from_utf8(contents)
                .map(Some)
                .map_err(|_| Error::InvalidUtf8File {
                    path: path.to_path_buf(),
                })
        }
        None => Ok(None),
    }
}

fn loader_active_now(env: &Environment) -> bool {
    env.var_os("BASH_COMPLETION_VERSINFO").is_some()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{LoaderStatus, detect, install, loader_active_now, loader_status};
    use crate::infra::env::Environment;
    use crate::model::{ActivationMode, Availability};

    #[test]
    fn loader_status_is_active_when_env_hint_is_present() {
        let env = Environment::test().with_var("BASH_COMPLETION_VERSINFO", "2");
        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::ActiveNow);
        assert!(loader_active_now(&env));
    }

    #[test]
    fn loader_status_is_present_but_unwired_when_only_loader_file_exists() {
        let temp_root = crate::tests::temp_dir("bash-loader-present-but-unwired");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();
        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_is_wired_when_bash_profile_sources_known_loader() {
        let temp_root = crate::tests::temp_dir("bash-loader-bash-profile");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bash_profile"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".bash_profile should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();
        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(
            status,
            LoaderStatus::WiredInStartup(home.join(".bash_profile"))
        );
    }

    #[test]
    fn loader_status_accepts_tab_after_source_keyword() {
        let temp_root = crate::tests::temp_dir("bash-loader-source-tab");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bash_profile"),
            "source\t/usr/share/bash-completion/bash_completion\n",
        )
        .expect(".bash_profile should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();
        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(
            status,
            LoaderStatus::WiredInStartup(home.join(".bash_profile"))
        );
    }

    #[test]
    fn loader_status_accepts_tab_separated_then_dot_bashrc_chain() {
        let temp_root = crate::tests::temp_dir("bash-loader-then-dot-tab-chain");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bash_profile"),
            "if [ -f \"$HOME/.bashrc\" ]; then\t.\t\"$HOME/.bashrc\"; fi\n",
        )
        .expect(".bash_profile should be writable");
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

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::WiredInStartup(home.join(".bashrc")));
    }

    #[test]
    fn loader_status_follows_tilde_sourced_bashrc_chain() {
        let temp_root = crate::tests::temp_dir("bash-loader-tilde-chain");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bash_profile"),
            "if [ -f ~/.bashrc ]; then\n  . ~/.bashrc\nfi\n",
        )
        .expect(".bash_profile should be writable");
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

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::WiredInStartup(home.join(".bashrc")));
    }

    #[test]
    fn loader_status_follows_home_expanded_bashrc_chain() {
        let temp_root = crate::tests::temp_dir("bash-loader-home-chain");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bash_profile"),
            "if [ -f \"$HOME/.bashrc\" ]; then\n  source \"$HOME/.bashrc\"\nfi\n",
        )
        .expect(".bash_profile should be writable");
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

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::WiredInStartup(home.join(".bashrc")));
    }

    #[test]
    fn loader_status_is_wired_when_bashrc_sources_etc_bashrc_that_sources_loader() {
        let temp_root = crate::tests::temp_dir("bash-loader-via-etc-bashrc");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(home.join(".bashrc"), "source /etc/bashrc\n")
            .expect(".bashrc should be writable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents(
                "/etc/bashrc",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(
            status,
            LoaderStatus::WiredInStartup(PathBuf::from("/etc/bashrc"))
        );
    }

    #[test]
    fn loader_status_does_not_assume_etc_bashrc_is_reachable_without_startup_wiring() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents(
                "/etc/bashrc",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_is_wired_when_profile_d_script_sources_known_loader() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents(
                "/etc/profile",
                "for i in /etc/profile.d/*.sh; do\n  [ -r \"$i\" ] && . \"$i\"\ndone\n",
            )
            .with_dir_entries(
                "/etc/profile.d",
                [PathBuf::from("/etc/profile.d/bash-completion.sh")],
            )
            .with_file_contents(
                "/etc/profile.d/bash-completion.sh",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(
            status,
            LoaderStatus::WiredInStartup(PathBuf::from("/etc/profile.d/bash-completion.sh"))
        );
    }

    #[test]
    fn loader_status_does_not_assume_profile_d_is_reachable_without_startup_wiring() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_dir_entries(
                "/etc/profile.d",
                [PathBuf::from("/etc/profile.d/bash-completion.sh")],
            )
            .with_file_contents(
                "/etc/profile.d/bash-completion.sh",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_does_not_treat_unrelated_profile_d_source_as_loader_wiring() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents("/etc/profile", "source /etc/profile.d/other.sh\n")
            .with_dir_entries(
                "/etc/profile.d",
                [
                    PathBuf::from("/etc/profile.d/bash-completion.sh"),
                    PathBuf::from("/etc/profile.d/other.sh"),
                ],
            )
            .with_file_contents(
                "/etc/profile.d/bash-completion.sh",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .with_file_contents("/etc/profile.d/other.sh", "echo unrelated\n")
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_is_wired_when_profile_directly_sources_non_sh_profile_d_script() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents("/etc/profile", "source /etc/profile.d/bash_completion\n")
            .with_file_contents(
                "/etc/profile.d/bash_completion",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(
            status,
            LoaderStatus::WiredInStartup(PathBuf::from("/etc/profile.d/bash_completion"))
        );
    }

    #[test]
    fn loader_status_is_wired_when_run_parts_loads_non_dotted_script() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents("/etc/profile", "run-parts /etc/profile.d\n")
            .with_dir_entries(
                "/etc/profile.d",
                [PathBuf::from("/etc/profile.d/bash_completion")],
            )
            .with_file_contents(
                "/etc/profile.d/bash_completion",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(
            status,
            LoaderStatus::WiredInStartup(PathBuf::from("/etc/profile.d/bash_completion"))
        );
    }

    #[test]
    fn loader_status_does_not_treat_different_profile_d_prefix_as_reachable() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents("/etc/profile", "for i in /etc/profile.d-custom/*.sh; do . \"$i\"; done\nrun-parts /etc/profile.d-custom\n")
            .with_dir_entries(
                "/etc/profile.d",
                [PathBuf::from("/etc/profile.d/bash-completion.sh")],
            )
            .with_file_contents(
                "/etc/profile.d/bash-completion.sh",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_does_not_treat_echoed_profile_d_glob_as_wiring() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/test-home")
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .with_file_contents("/etc/profile", "echo /etc/profile.d/*.sh >/dev/null\n")
            .with_dir_entries(
                "/etc/profile.d",
                [PathBuf::from("/etc/profile.d/bash-completion.sh")],
            )
            .with_file_contents(
                "/etc/profile.d/bash-completion.sh",
                "source /usr/share/bash-completion/bash_completion\n",
            )
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_respects_login_file_precedence() {
        let temp_root = crate::tests::temp_dir("bash-loader-login-precedence");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".bash_profile"),
            "export PATH=\"$HOME/bin:$PATH\"\n",
        )
        .expect(".bash_profile should be writable");
        fs::write(
            home.join(".bash_login"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".bash_login should be writable");
        fs::write(
            home.join(".profile"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".profile should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

        assert_eq!(status, LoaderStatus::PresentButUnwired);
    }

    #[test]
    fn loader_status_ignores_comments_and_plain_strings() {
        let temp_root = crate::tests::temp_dir("bash-loader-ignore-comments");
        let home = temp_root.join("home");
        fs::create_dir_all(&home).expect("home should be creatable");
        fs::write(
            home.join(".profile"),
            "# source /usr/share/bash-completion/bash_completion\nBASH_LOADER=/usr/share/bash-completion/bash_completion\necho \"source /usr/share/bash-completion/bash_completion\"\n",
        )
        .expect(".profile should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let status = loader_status(&env).expect("status should resolve");

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
    fn install_quotes_bashrc_path_in_next_step_when_home_has_spaces() {
        let temp_root = crate::tests::temp_dir("bash-loader-install-next-step");
        let home = temp_root.join("home with space");
        let target = home.join(".local/share/bash-completion/completions/tool");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = install(&env, "tool", &target).expect("install should succeed");

        let next_step = report.report.next_step.expect("next_step should exist");
        assert!(next_step.contains("source '"));
        assert!(next_step.contains("home with space/.bashrc"));
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
            home.join(".profile"),
            "source /usr/share/bash-completion/bash_completion\n",
        )
        .expect(".profile should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .with_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = detect(&env, "tool", &completion_path).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::SystemLoader);
        assert_eq!(report.availability, Availability::AvailableAfterNewShell);
        assert_eq!(report.location, Some(home.join(".profile")));
    }

    #[test]
    fn detect_unwired_guidance_uses_actual_bashrc_and_completion_paths() {
        let temp_root = crate::tests::temp_dir("bash-detect-unwired-guidance");
        let home = temp_root.join("home with space");
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
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let report = detect(&env, "tool", &completion_path).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::ManualActionRequired);
        let next_step = report.next_step.expect("next_step should exist");
        assert!(next_step.contains("source '"));
        assert!(next_step.contains("home with space/.bashrc"));
        assert!(
            next_step.contains("home with space/.local/share/bash-completion/completions/tool")
        );
    }

    #[test]
    fn detect_reports_corruption_when_duplicate_managed_block_is_malformed() {
        let temp_root = crate::tests::temp_dir("bash-detect-corrupt-duplicate");
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
            "# >>> shellcomp bash tool >>>\nif [ -f '/tmp/tool' ]; then\n  . '/tmp/tool'\nfi\n# <<< shellcomp bash tool <<<\n# >>> shellcomp bash tool >>>\n. '/tmp/other'\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .without_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let error = detect(&env, "tool", &completion_path).expect_err("detect should fail");

        assert!(matches!(error, crate::Error::ManagedBlockMissingEnd { .. }));
    }

    #[test]
    fn detect_reports_manual_action_when_duplicate_managed_blocks_exist() {
        let temp_root = crate::tests::temp_dir("bash-detect-duplicate-managed");
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
            "# >>> shellcomp bash tool >>>\nif [ -f '/tmp/tool' ]; then\n  . '/tmp/tool'\nfi\n# <<< shellcomp bash tool <<<\n# >>> shellcomp bash tool >>>\nif [ -f '/tmp/tool' ]; then\n  . '/tmp/tool'\nfi\n# <<< shellcomp bash tool <<<\n",
        )
        .expect(".bashrc should be writable");

        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("BASH_COMPLETION_VERSINFO")
            .without_existing_path("/usr/share/bash-completion/bash_completion")
            .without_real_path_lookups();

        let report = detect(&env, "tool", &completion_path).expect("detect should succeed");

        assert_eq!(report.mode, ActivationMode::ManagedRcBlock);
        assert_eq!(report.availability, Availability::ManualActionRequired);
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
