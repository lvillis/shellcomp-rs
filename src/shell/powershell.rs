use std::path::Path;

use crate::error::{Error, Result};
use crate::infra::fs;
use crate::model::{ActivationMode, ActivationReport, Availability, CleanupReport, FileChange};
use crate::shell::{ActivationOutcome, CleanupOutcome};

pub(crate) fn install(_program_name: &str, target_path: &Path) -> Result<ActivationOutcome> {
    Ok(ActivationOutcome {
        report: ActivationReport {
            mode: ActivationMode::Manual,
            availability: Availability::ManualActionRequired,
            location: Some(target_path.to_path_buf()),
            reason: Some(
                "Installed the PowerShell completion script, but shellcomp does not modify PowerShell profiles automatically."
                    .to_owned(),
            ),
            next_step: Some(format!(
                "Add `. {}` to `$PROFILE.CurrentUserAllHosts` or another PowerShell profile.",
                powershell_quote(target_path)?
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
                "shellcomp does not remove PowerShell profile activation automatically.".to_owned(),
            ),
            next_step: None,
        },
        affected_locations: Vec::new(),
    })
}

pub(crate) fn detect(_program_name: &str, target_path: &Path) -> Result<ActivationReport> {
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
            "Completion file is installed, but shellcomp cannot verify whether any PowerShell profile already dot-sources it.".to_owned()
        } else {
            format!(
                "Completion file `{}` is not installed.",
                target_path.display()
            )
        }),
        next_step: Some(format!(
            "Add `. {}` to `$PROFILE.CurrentUserAllHosts` or another PowerShell profile after installing the script.",
            powershell_quote(target_path)?
        )),
    })
}

fn powershell_quote(path: &Path) -> Result<String> {
    let value = path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: path.to_path_buf(),
    })?;
    Ok(format!("'{}'", value.replace('\'', "''")))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{detect, install, uninstall};
    use crate::model::{ActivationMode, Availability, FileChange};

    #[test]
    fn install_reports_manual_activation_guidance() {
        let report =
            install("tool", std::path::Path::new("/tmp/tool.ps1")).expect("install should work");

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
                .is_some_and(|text| text.contains("$PROFILE.CurrentUserAllHosts"))
        );
    }

    #[test]
    fn detect_reports_manual_when_script_exists() {
        let temp_root = crate::tests::temp_dir("powershell-detect");
        let target = temp_root.join("tool.ps1");
        fs::write(&target, "# powershell completion").expect("script should be writable");

        let report = detect("tool", &target).expect("detect should work");

        assert_eq!(report.mode, ActivationMode::Manual);
        assert_eq!(report.availability, Availability::Unknown);
    }

    #[test]
    fn uninstall_reports_no_profile_cleanup() {
        let report = uninstall("tool", std::path::Path::new("/tmp/tool.ps1"))
            .expect("uninstall should work");

        assert_eq!(report.cleanup.mode, ActivationMode::Manual);
        assert_eq!(report.cleanup.change, FileChange::Absent);
    }
}
