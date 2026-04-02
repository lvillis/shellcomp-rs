use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::infra::{
    env::Environment,
    fs,
    managed_block::{self, ManagedBlock},
};
use crate::model::{
    ActivationMode, ActivationReport, Availability, CleanupReport, LegacyManagedBlock,
};
use crate::shell::{ActivationOutcome, CleanupOutcome, MigrationOutcome, migrate_profile_blocks};

pub(crate) fn install(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationOutcome> {
    let rc_path = zshrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    managed_block::upsert(&rc_path, &block)?;

    Ok(ActivationOutcome {
        report: ActivationReport {
            mode: ActivationMode::ManagedRcBlock,
            availability: Availability::AvailableAfterSource,
            location: Some(rc_path.clone()),
            reason: Some(
                "shellcomp added a managed block to update `fpath` and run `compinit -i` when needed."
                    .to_owned(),
            ),
            next_step: Some(format!(
                "Run `source {}` or start a new Zsh session.",
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
    let rc_path = zshrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    let rc_change = managed_block::remove(&rc_path, &block)?;

    Ok(CleanupOutcome {
        cleanup: CleanupReport {
            mode: ActivationMode::ManagedRcBlock,
            change: rc_change,
            location: Some(rc_path.clone()),
            reason: Some(match rc_change {
                crate::FileChange::Removed => {
                    "Removed the managed Zsh activation block from .zshrc.".to_owned()
                }
                crate::FileChange::Absent => {
                    "No managed Zsh activation block was present in .zshrc.".to_owned()
                }
                _ => "Zsh activation cleanup completed.".to_owned(),
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
    let rc_path = zshrc_path(env)?;
    let block = managed_block(program_name, target_path)?;
    let wired = managed_block::matches(&rc_path, &block)?;

    if !fs::file_exists(target_path) {
        return Ok(ActivationReport {
            mode: ActivationMode::ManagedRcBlock,
            availability: Availability::ManualActionRequired,
            location: Some(target_path.to_path_buf()),
            reason: Some(format!(
                "Completion file `{}` is not installed.",
                target_path.display()
            )),
            next_step: Some(
                "Run your CLI's completion install command or place the file into the managed zsh directory."
                    .to_owned(),
            ),
        });
    }

    Ok(ActivationReport {
        mode: ActivationMode::ManagedRcBlock,
        availability: if wired {
            Availability::AvailableAfterSource
        } else {
            Availability::ManualActionRequired
        },
        location: Some(rc_path),
        reason: Some(if wired {
            "Managed zsh activation block is present.".to_owned()
        } else {
            "Completion file exists, but shellcomp did not find the managed zsh activation block."
                .to_owned()
        }),
        next_step: Some(if wired {
            "Run `source ~/.zshrc` or start a new Zsh session.".to_owned()
        } else {
            "Re-run installation or add the completion directory to `fpath` and run `compinit -i`."
                .to_owned()
        }),
    })
}

pub(crate) fn migrate(
    env: &Environment,
    program_name: &str,
    target_path: &Path,
    legacy_blocks: &[LegacyManagedBlock],
) -> Result<MigrationOutcome> {
    let rc_path = zshrc_path(env)?;
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
    let completion_dir = target_path.parent().ok_or_else(|| Error::PathHasNoParent {
        path: target_path.to_path_buf(),
    })?;
    let quoted = shell_quote(completion_dir)?;

    Ok(ManagedBlock {
        start_marker: format!("# >>> shellcomp zsh {program_name} >>>"),
        end_marker: format!("# <<< shellcomp zsh {program_name} <<<"),
        body: format!(
            "shellcomp_zfunc_dir={quoted}\nshellcomp_zfunc_changed=0\nif (( ${{fpath[(Ie)$shellcomp_zfunc_dir]}} == 0 )); then\n  fpath=(\"$shellcomp_zfunc_dir\" $fpath)\n  shellcomp_zfunc_changed=1\nfi\nautoload -Uz compinit\nif ! type compdef >/dev/null 2>&1 || (( shellcomp_zfunc_changed == 1 )); then\n  compinit -i\nfi\nunset shellcomp_zfunc_changed\nunset shellcomp_zfunc_dir"
        ),
    })
}

fn shell_quote(path: &Path) -> Result<String> {
    let value = path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: path.to_path_buf(),
    })?;
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn zshrc_path(env: &Environment) -> Result<PathBuf> {
    Ok(env.zdotdir()?.join(".zshrc"))
}

#[cfg(test)]
mod tests {
    use super::install;
    use super::managed_block;
    use crate::infra::env::Environment;
    use std::path::Path;

    #[test]
    fn managed_block_uses_stable_zfunc_variable() {
        let block =
            managed_block("tool", Path::new("/tmp/home/.zfunc/_tool")).expect("block is valid");

        assert!(block.body.contains("shellcomp_zfunc_dir="));
        assert!(block.body.contains("shellcomp_zfunc_changed=0"));
        assert!(block.body.contains("type compdef"));
        assert!(block.body.contains("compinit -i"));
        assert!(block.body.contains("shellcomp_zfunc_changed == 1"));
        assert!(block.body.contains("unset shellcomp_zfunc_changed"));
        assert!(block.body.contains("unset shellcomp_zfunc_dir"));
    }

    #[test]
    fn install_errors_when_zshrc_is_not_writable() {
        let temp_root = crate::tests::temp_dir("install-zsh-manual-fallback");
        let home = temp_root.join("home");
        std::fs::create_dir_all(home.join(".zshrc")).expect("directory should be creatable");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("ZDOTDIR")
            .without_real_path_lookups();

        let error =
            install(&env, "tool", &home.join(".zfunc/_tool")).expect_err("install should fail");

        assert!(matches!(error, crate::Error::Io { .. }));
    }
}
