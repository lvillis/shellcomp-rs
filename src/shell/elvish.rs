use std::path::Path;

use crate::error::Result;
use crate::infra::{env::Environment, fs};
use crate::model::{ActivationMode, ActivationReport, Availability, CleanupReport, FileChange};
use crate::shell::{ActivationOutcome, CleanupOutcome};

pub(crate) fn install(
    _env: &Environment,
    _program_name: &str,
    target_path: &Path,
) -> Result<ActivationOutcome> {
    Ok(ActivationOutcome {
        report: ActivationReport {
            mode: ActivationMode::Manual,
            availability: Availability::ManualActionRequired,
            location: Some(target_path.to_path_buf()),
            reason: Some(
                "Installed the Elvish completion script, but shellcomp does not modify rc.elv automatically."
                    .to_owned(),
            ),
            next_step: Some(format!(
                "Evaluate `{}` from your Elvish rc.elv. If you use a command such as `slurp`, make sure the installed path is quoted correctly for Elvish.",
                target_path.display()
            )),
        },
        affected_locations: Vec::new(),
    })
}

pub(crate) fn uninstall(_program_name: &str, _target_path: &Path) -> Result<CleanupOutcome> {
    Ok(CleanupOutcome {
        cleanup: CleanupReport {
            mode: ActivationMode::Manual,
            change: FileChange::Absent,
            location: None,
            reason: Some(
                "shellcomp does not remove Elvish rc.elv activation automatically.".to_owned(),
            ),
            next_step: None,
        },
        affected_locations: Vec::new(),
    })
}

pub(crate) fn detect(
    _env: &Environment,
    _program_name: &str,
    target_path: &Path,
) -> Result<ActivationReport> {
    let installed = fs::file_exists(target_path);

    Ok(ActivationReport {
        mode: ActivationMode::Manual,
        availability: if installed {
            Availability::Unknown
        } else {
            Availability::ManualActionRequired
        },
        location: Some(target_path.to_path_buf()),
        reason: Some(if installed {
            "Completion file is installed, but shellcomp cannot verify whether rc.elv already evaluates it.".to_owned()
        } else {
            format!(
                "Completion file `{}` is not installed.",
                target_path.display()
            )
        }),
        next_step: Some(format!(
            "Evaluate `{}` from your Elvish rc.elv. If you use a command such as `slurp`, make sure the installed path is quoted correctly for Elvish.",
            target_path.display()
        )),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{detect, install, uninstall};
    use crate::infra::env::Environment;
    use crate::model::{ActivationMode, Availability, FileChange};

    #[test]
    fn install_reports_manual_rc_guidance() {
        let temp_root = crate::tests::temp_dir("elvish-install");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_CONFIG_HOME")
            .without_real_path_lookups();

        let report = install(
            &env,
            "tool",
            &home.join(".config/elvish/lib/shellcomp/tool.elv"),
        )
        .expect("install should work");

        assert_eq!(report.report.mode, ActivationMode::Manual);
        assert_eq!(
            report.report.availability,
            Availability::ManualActionRequired
        );
        assert!(
            report
                .report
                .next_step
                .as_deref()
                .is_some_and(|text| text.contains("quoted correctly for Elvish"))
        );
    }

    #[test]
    fn detect_reports_manual_when_script_exists() {
        let temp_root = crate::tests::temp_dir("elvish-detect");
        let home = temp_root.join("home");
        let env = Environment::test()
            .with_var("HOME", &home)
            .without_var("XDG_CONFIG_HOME")
            .without_real_path_lookups();
        let target = home.join(".config/elvish/lib/shellcomp/tool.elv");
        fs::create_dir_all(target.parent().expect("target should have a parent"))
            .expect("target dir should be creatable");
        fs::write(&target, "# elvish completion").expect("script should be writable");

        let report = detect(&env, "tool", &target).expect("detect should work");

        assert_eq!(report.mode, ActivationMode::Manual);
        assert_eq!(report.availability, Availability::Unknown);
        assert_eq!(report.location, Some(target));
    }

    #[test]
    fn uninstall_reports_no_rc_cleanup() {
        let report = uninstall("tool", std::path::Path::new("/tmp/tool.elv"))
            .expect("uninstall should work");

        assert_eq!(report.cleanup.mode, ActivationMode::Manual);
        assert_eq!(report.cleanup.change, FileChange::Absent);
    }
}
