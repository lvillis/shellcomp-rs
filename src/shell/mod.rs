mod bash;
mod elvish;
mod fish;
mod powershell;
mod zsh;

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::infra::env::Environment;
use crate::model::{ActivationReport, CleanupReport, Shell};

#[derive(Debug)]
pub(crate) struct ActivationOutcome {
    pub(crate) report: ActivationReport,
    pub(crate) affected_locations: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct CleanupOutcome {
    pub(crate) cleanup: CleanupReport,
    pub(crate) affected_locations: Vec<PathBuf>,
}

pub(crate) fn install_default(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationOutcome> {
    match shell {
        Shell::Bash => bash::install(env, program_name, target_path),
        Shell::Zsh => zsh::install(env, program_name, target_path),
        Shell::Fish => fish::install(program_name, target_path),
        Shell::Powershell => powershell::install(program_name, target_path),
        Shell::Elvish => elvish::install(env, program_name, target_path),
        unsupported => Err(crate::Error::UnsupportedShell(unsupported.clone())),
    }
}

pub(crate) fn uninstall_default(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<CleanupOutcome> {
    match shell {
        Shell::Bash => bash::uninstall(env, program_name, target_path),
        Shell::Zsh => zsh::uninstall(env, program_name, target_path),
        Shell::Fish => fish::uninstall(program_name, target_path),
        Shell::Powershell => powershell::uninstall(program_name, target_path),
        Shell::Elvish => elvish::uninstall(program_name, target_path),
        unsupported => Err(crate::Error::UnsupportedShell(unsupported.clone())),
    }
}

pub(crate) fn detect_default(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<ActivationReport> {
    match shell {
        Shell::Bash => bash::detect(env, program_name, target_path),
        Shell::Zsh => zsh::detect(env, program_name, target_path),
        Shell::Fish => fish::detect(program_name, target_path),
        Shell::Powershell => powershell::detect(program_name, target_path),
        Shell::Elvish => elvish::detect(env, program_name, target_path),
        unsupported => Err(crate::Error::UnsupportedShell(unsupported.clone())),
    }
}
