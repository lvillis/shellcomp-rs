use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use shellcomp::{
    ActivationMode, ActivationPolicy, Availability, Error, FailureKind, FileChange, InstallRequest,
    LegacyManagedBlock, MigrateManagedBlocksRequest, Operation, OperationEventPhase, Shell,
    UninstallRequest, default_install_path, detect_activation_at_path, install,
    install_with_policy, migrate_managed_blocks, uninstall, uninstall_with_policy,
    with_operation_events,
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

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock should not be poisoned")
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

    let report = error
        .as_failure()
        .expect("path without parent should fail structurally");
    assert_eq!(report.kind, FailureKind::InvalidTargetPath);
    assert_eq!(report.target_path.as_deref(), Some(Path::new("/")));
    assert_eq!(report.file_change, None);
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

#[test]
fn default_install_path_rejects_invalid_default_env_values() {
    let _guard = env_lock();
    let temp_root = temp_dir("default-path-invalid");
    let old_home = std::env::var_os("HOME");
    let old_xdg_data = std::env::var_os("XDG_DATA_HOME");

    let home = temp_root.join("home");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_DATA_HOME", "relative-cache");
    }

    let error = default_install_path(Shell::Bash, "demo")
        .expect_err("relative XDG_DATA_HOME should be rejected");
    assert_eq!(error.error_code(), FailureKind::InvalidTargetPath.code());

    unsafe {
        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match old_xdg_data {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }
}

#[test]
fn operation_events_capture_install_lifecycle() {
    let temp_root = temp_dir("install-events");
    let target = temp_root.join("demo.bash");
    let script = b"complete -F _demo demo\n";

    let events = Arc::new(Mutex::new(Vec::new()));
    let report = with_operation_events(
        Some({
            let events = Arc::clone(&events);
            move |event: &shellcomp::OperationEvent| {
                events.lock().unwrap().push(event.clone());
            }
        }),
        || {
            install(InstallRequest {
                shell: Shell::Bash,
                program_name: "demo",
                script,
                path_override: Some(target.clone()),
            })
            .expect("install should succeed")
        },
    );

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, OperationEventPhase::Started);
    assert_eq!(events[1].phase, OperationEventPhase::Succeeded);
    assert_eq!(events[0].trace_id, events[1].trace_id);
    assert_eq!(events[0].operation, Operation::Install);
    assert_eq!(events[0].program_name, "demo");
    assert_eq!(events[1].target_path, Some(target.clone()));
    assert_eq!(report.target_path, target);
}

#[test]
fn operation_events_capture_failure_metadata() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let error = with_operation_events(
        Some({
            let events = Arc::clone(&events);
            move |event: &shellcomp::OperationEvent| {
                events.lock().unwrap().push(event.clone());
            }
        }),
        || {
            uninstall(UninstallRequest {
                shell: Shell::Bash,
                program_name: "demo",
                path_override: Some(PathBuf::from("relative-path")),
            })
        },
    )
    .expect_err("with_operation_events should preserve failure");

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].phase, OperationEventPhase::Started);
    assert_eq!(events[1].phase, OperationEventPhase::Failed);
    assert_eq!(events[1].operation, Operation::Uninstall);
    assert_eq!(
        events[1].error_code,
        Some(FailureKind::InvalidTargetPath.code())
    );
    assert!(!events[1].retryable);
    let report = error.as_failure().expect("failure expected");
    assert_eq!(report.kind, FailureKind::InvalidTargetPath);
    assert_eq!(events[1].target_path, Some(PathBuf::from("relative-path")));
}

#[test]
fn install_and_uninstall_with_policy_work_for_custom_fish_paths() {
    let temp_root = temp_dir("policy-roundtrip");
    let target = temp_root.join("completions").join("demo.fish");

    let install_report = install_with_policy(
        InstallRequest {
            shell: Shell::Fish,
            program_name: "demo",
            script: b"complete -c demo -f\n",
            path_override: Some(target.clone()),
        },
        ActivationPolicy::Manual,
    )
    .expect("install_with_policy should succeed");

    assert_eq!(install_report.file_change, FileChange::Created);
    assert_eq!(install_report.activation.mode, ActivationMode::Manual);

    let uninstall_report = uninstall_with_policy(
        UninstallRequest {
            shell: Shell::Fish,
            program_name: "demo",
            path_override: Some(target.clone()),
        },
        ActivationPolicy::Manual,
    )
    .expect("uninstall_with_policy should succeed");

    assert_eq!(uninstall_report.file_change, FileChange::Removed);
    assert_eq!(uninstall_report.cleanup.mode, ActivationMode::Manual);
    assert!(!target.exists());
}

#[test]
fn detect_activation_at_path_reports_status_for_custom_fish_path() {
    let temp_root = temp_dir("detect-at-path");
    let target = temp_root.join("completions").join("demo.fish");
    std::fs::create_dir_all(target.parent().expect("target should have a parent"))
        .expect("target dir should be creatable");
    std::fs::write(&target, "complete -c demo -f\n").expect("target file should be writable");

    let report = detect_activation_at_path(Shell::Fish, "demo", &target)
        .expect("detect_activation_at_path should succeed");

    assert_eq!(report.mode, ActivationMode::Manual);
    assert_eq!(report.availability, Availability::Unknown);
}

#[test]
fn powershell_default_install_uses_managed_profile_via_public_api() {
    let _guard = env_lock();
    let temp_root = temp_dir("powershell-managed");
    let home = temp_root.join("home");
    std::fs::create_dir_all(&home).expect("home should be creatable");
    let old_home = std::env::var_os("HOME");
    let old_xdg_config = std::env::var_os("XDG_CONFIG_HOME");
    let old_xdg_data = std::env::var_os("XDG_DATA_HOME");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
    }

    let report = install(InstallRequest {
        shell: Shell::Powershell,
        program_name: "demo",
        script: b"# powershell completion\n",
        path_override: None,
    })
    .expect("powershell install should succeed");

    unsafe {
        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match old_xdg_config {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match old_xdg_data {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    assert_eq!(report.activation.mode, ActivationMode::ManagedRcBlock);
    assert_eq!(
        report.activation.availability,
        Availability::AvailableAfterNewShell
    );
}

#[test]
fn migrate_managed_blocks_rewrites_legacy_markers_via_public_api() {
    let _guard = env_lock();
    let temp_root = temp_dir("migrate-public");
    let home = temp_root.join("home");
    let bashrc = home.join(".bashrc");
    std::fs::create_dir_all(&home).expect("home should be creatable");
    std::fs::write(
        &bashrc,
        "# >>> legacy demo >>>\n. '/tmp/demo'\n# <<< legacy demo <<<\n",
    )
    .expect("bashrc should be writable");
    let old_home = std::env::var_os("HOME");
    let old_xdg_data = std::env::var_os("XDG_DATA_HOME");
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::remove_var("XDG_DATA_HOME");
    }

    let report = migrate_managed_blocks(MigrateManagedBlocksRequest {
        shell: Shell::Bash,
        program_name: "demo",
        path_override: None,
        legacy_blocks: vec![LegacyManagedBlock {
            start_marker: "# >>> legacy demo >>>".to_owned(),
            end_marker: "# <<< legacy demo <<<".to_owned(),
        }],
    })
    .expect("migration should succeed");

    unsafe {
        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match old_xdg_data {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }

    assert_eq!(report.legacy_change, FileChange::Removed);
    let rendered = std::fs::read_to_string(bashrc).expect("bashrc should remain readable");
    assert!(rendered.contains("shellcomp bash demo"));
    assert!(!rendered.contains("legacy demo"));
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
    fn render_clap_completion_accepts_reexported_clap_complete_shell() {
        let script = render_clap_completion::<Cli>(shellcomp::clap_complete::Shell::Fish, "demo")
            .expect("fish completion should render");
        let rendered = String::from_utf8(script).expect("rendered script should be utf-8");

        assert!(rendered.contains("demo"));
    }

    #[test]
    fn shell_converts_from_reexported_clap_complete_shell() {
        let shell: Shell = shellcomp::clap_complete::Shell::Zsh.into();
        assert_eq!(shell, Shell::Zsh);
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
