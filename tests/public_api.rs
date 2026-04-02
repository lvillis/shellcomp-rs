use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use shellcomp::{
    ActivationMode, Availability, Error, FailureKind, FileChange, InstallRequest, Shell,
    UninstallRequest, default_install_path, install, uninstall,
};

fn temp_dir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("shellcomp-it-{label}-{unique}"));
    std::fs::create_dir_all(&path).expect("temp dir should be creatable");
    path
}

#[test]
fn install_and_uninstall_roundtrip_via_public_api_with_path_override() {
    let temp_root = temp_dir("roundtrip");
    let target = temp_root.join("completions").join("demo.bash");
    let script = b"complete -F _demo demo\n";

    let first_install = install(InstallRequest {
        shell: Shell::Bash,
        program_name: "demo",
        script,
        path_override: Some(target.clone()),
    })
    .expect("first install should succeed");

    assert_eq!(first_install.shell, Shell::Bash);
    assert_eq!(first_install.target_path, target);
    assert_eq!(first_install.file_change, FileChange::Created);
    assert_eq!(first_install.activation.mode, ActivationMode::Manual);
    assert_eq!(
        first_install.activation.availability,
        Availability::ManualActionRequired
    );
    assert_eq!(
        std::fs::read(&first_install.target_path).expect("installed file should exist"),
        script
    );

    let second_install = install(InstallRequest {
        shell: Shell::Bash,
        program_name: "demo",
        script,
        path_override: Some(first_install.target_path.clone()),
    })
    .expect("second install should succeed");

    assert_eq!(second_install.file_change, FileChange::Unchanged);

    let first_uninstall = uninstall(UninstallRequest {
        shell: Shell::Bash,
        program_name: "demo",
        path_override: Some(second_install.target_path.clone()),
    })
    .expect("first uninstall should succeed");

    assert_eq!(first_uninstall.file_change, FileChange::Removed);
    assert_eq!(first_uninstall.cleanup.mode, ActivationMode::Manual);
    assert_eq!(first_uninstall.cleanup.change, FileChange::Absent);
    assert!(!first_uninstall.target_path.exists());

    let second_uninstall = uninstall(UninstallRequest {
        shell: Shell::Bash,
        program_name: "demo",
        path_override: Some(first_uninstall.target_path.clone()),
    })
    .expect("second uninstall should succeed");

    assert_eq!(second_uninstall.file_change, FileChange::Absent);
    assert_eq!(second_uninstall.cleanup.mode, ActivationMode::Manual);
    assert_eq!(second_uninstall.cleanup.change, FileChange::Absent);
}

#[test]
fn install_rejects_invalid_program_name_via_public_api() {
    let target = temp_dir("invalid-name").join("demo.bash");

    let error = install(InstallRequest {
        shell: Shell::Bash,
        program_name: "bad/name",
        script: b"complete -F _demo demo\n",
        path_override: Some(target),
    })
    .expect_err("invalid program name should fail");

    assert!(matches!(error, Error::InvalidProgramName { .. }));
    assert!(error.reason().is_some());
    assert!(error.next_step().is_some());
}

#[test]
fn install_returns_structured_failure_for_path_without_parent() {
    let error = install(InstallRequest {
        shell: Shell::Fish,
        program_name: "demo",
        script: b"complete -c demo\n",
        path_override: Some(PathBuf::from("/")),
    })
    .expect_err("path without parent should fail");

    match error {
        Error::Failure(report) => {
            assert_eq!(report.kind, FailureKind::InvalidTargetPath);
            assert_eq!(report.target_path.as_deref(), Some(Path::new("/")));
            assert_eq!(report.file_change, None);
        }
        other => panic!("unexpected error variant: {other}"),
    }
}

#[test]
fn default_install_path_rejects_unsupported_shell_via_public_api() {
    let error = default_install_path(Shell::Other("xonsh".to_owned()), "demo")
        .expect_err("unsupported shell should fail");

    assert!(matches!(
        error,
        Error::UnsupportedShell(Shell::Other(value)) if value == "xonsh"
    ));
}

#[cfg(feature = "clap")]
mod clap_tests {
    use clap::Parser;
    use shellcomp::{Error, Shell, render_clap_completion};

    #[derive(Parser)]
    struct Cli {
        #[arg(long)]
        verbose: bool,
    }

    #[test]
    fn render_clap_completion_is_available_from_public_api() {
        let script = render_clap_completion::<Cli>(Shell::Fish, "demo")
            .expect("fish completion should render");
        let rendered = String::from_utf8(script).expect("rendered script should be utf-8");

        assert!(rendered.contains("demo"));
    }

    #[test]
    fn render_clap_completion_rejects_other_shell_via_public_api() {
        let error = render_clap_completion::<Cli>(Shell::Other("xonsh".to_owned()), "demo")
            .expect_err("unsupported shell should fail");

        assert!(matches!(
            error,
            Error::UnsupportedShell(Shell::Other(value)) if value == "xonsh"
        ));
    }
}
