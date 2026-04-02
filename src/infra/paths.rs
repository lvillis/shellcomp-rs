use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::infra::env::Environment;
use crate::model::Shell;

pub(crate) fn validate_program_name(program_name: &str) -> Result<()> {
    if program_name.is_empty() {
        return Err(Error::EmptyProgramName);
    }

    let is_safe = program_name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));

    if !is_safe || program_name == "." || program_name == ".." {
        return Err(Error::InvalidProgramName {
            program_name: program_name.to_owned(),
        });
    }

    Ok(())
}

pub(crate) fn default_install_path(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
) -> Result<PathBuf> {
    validate_program_name(program_name)?;

    match shell {
        Shell::Bash => Ok(env
            .xdg_data_home()?
            .join("bash-completion")
            .join("completions")
            .join(program_name)),
        Shell::Zsh => Ok(env
            .zdotdir()?
            .join(".zfunc")
            .join(format!("_{program_name}"))),
        Shell::Fish => Ok(env
            .xdg_config_home()?
            .join("fish")
            .join("completions")
            .join(format!("{program_name}.fish"))),
        unsupported => Err(Error::UnsupportedShell(unsupported.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::{default_install_path, validate_program_name};
    use crate::infra::env::Environment;
    use crate::model::Shell;

    #[test]
    fn resolves_default_paths() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/home")
            .without_var("XDG_DATA_HOME")
            .without_var("XDG_CONFIG_HOME")
            .without_var("ZDOTDIR");

        assert_eq!(
            default_install_path(&env, &Shell::Bash, "tool").expect("bash path should resolve"),
            std::path::PathBuf::from("/tmp/home/.local/share/bash-completion/completions/tool")
        );
        assert_eq!(
            default_install_path(&env, &Shell::Zsh, "tool").expect("zsh path should resolve"),
            std::path::PathBuf::from("/tmp/home/.zfunc/_tool")
        );
        assert_eq!(
            default_install_path(&env, &Shell::Fish, "tool").expect("fish path should resolve"),
            std::path::PathBuf::from("/tmp/home/.config/fish/completions/tool.fish")
        );
    }

    #[test]
    fn honors_xdg_and_zdotdir_overrides() {
        let env = Environment::test()
            .with_var("HOME", "/tmp/home")
            .with_var("XDG_DATA_HOME", "/tmp/data")
            .with_var("XDG_CONFIG_HOME", "/tmp/config")
            .with_var("ZDOTDIR", "/tmp/zdotdir");

        assert_eq!(
            default_install_path(&env, &Shell::Bash, "tool").expect("bash path should resolve"),
            std::path::PathBuf::from("/tmp/data/bash-completion/completions/tool")
        );
        assert_eq!(
            default_install_path(&env, &Shell::Zsh, "tool").expect("zsh path should resolve"),
            std::path::PathBuf::from("/tmp/zdotdir/.zfunc/_tool")
        );
        assert_eq!(
            default_install_path(&env, &Shell::Fish, "tool").expect("fish path should resolve"),
            std::path::PathBuf::from("/tmp/config/fish/completions/tool.fish")
        );
    }

    #[test]
    fn rejects_invalid_program_names() {
        for invalid in [
            "",
            ".",
            "..",
            "dir/tool",
            "dir\\tool",
            "two words",
            "bad\nname",
        ] {
            assert!(validate_program_name(invalid).is_err());
        }
    }
}
