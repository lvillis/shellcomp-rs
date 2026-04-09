pub(crate) mod detect;
pub(crate) mod install;
pub(crate) mod migrate;
pub(crate) mod uninstall;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::error::{Error, Result};
use crate::infra::{env::Environment, paths};
use crate::model::{
    ActivationMode, ActivationPolicy, ActivationReport, Availability, CleanupReport, FailureKind,
    FailureReport, FileChange, Operation, OperationEvent, OperationEventPhase, Shell,
};

type OperationEventHook = Arc<dyn Fn(&OperationEvent) + Send + Sync>;
type TargetPathLocks = HashMap<PathBuf, Arc<Mutex<()>>>;

thread_local! {
    static OPERATION_TRACE_ID: Cell<u64> = const { Cell::new(0) };
    static OPERATION_EVENT_HOOK: RefCell<Option<OperationEventHook>> = const { RefCell::new(None) };
}

static OPERATION_LOCKS: OnceLock<Mutex<TargetPathLocks>> = OnceLock::new();
static NEXT_OPERATION_TRACE_ID: AtomicU64 = AtomicU64::new(1);

fn allocate_trace_id() -> u64 {
    NEXT_OPERATION_TRACE_ID.fetch_add(1, Relaxed)
}

struct TraceScope {
    previous: u64,
}

impl Drop for TraceScope {
    fn drop(&mut self) {
        OPERATION_TRACE_ID.with(|slot| slot.set(self.previous));
    }
}

fn with_trace_scope<R>(f: impl FnOnce(u64) -> R) -> R {
    let (trace_id, previous) = OPERATION_TRACE_ID.with(|slot| {
        let previous = slot.get();
        let trace_id = if previous == 0 {
            allocate_trace_id()
        } else {
            previous
        };
        slot.set(trace_id);
        (trace_id, previous)
    });

    let _scope = TraceScope { previous };
    f(trace_id)
}

pub(crate) fn with_operation_trace<R>(f: impl FnOnce(u64) -> R) -> R {
    with_trace_scope(f)
}

pub(crate) fn with_operation_event_hook<R>(
    hook: Option<OperationEventHook>,
    f: impl FnOnce() -> R,
) -> R {
    struct OperationEventHookScope {
        previous: Option<OperationEventHook>,
    }

    impl Drop for OperationEventHookScope {
        fn drop(&mut self) {
            OPERATION_EVENT_HOOK.with(|slot| {
                *slot.borrow_mut() = self.previous.take();
            });
        }
    }

    let previous = OPERATION_EVENT_HOOK.with(|slot| {
        let mut slot = slot.borrow_mut();
        std::mem::replace(&mut *slot, hook)
    });
    let _scope = OperationEventHookScope { previous };
    f()
}

fn operation_event_scope() -> &'static Mutex<TargetPathLocks> {
    OPERATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_for_target_path(target_path: &Path) -> Arc<Mutex<()>> {
    let mut locks = operation_event_scope()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let key = target_path.to_path_buf();
    match locks.entry(key) {
        Entry::Occupied(entry) => entry.get().clone(),
        Entry::Vacant(entry) => {
            let mutex = Arc::new(Mutex::new(()));
            entry.insert(mutex.clone());
            mutex
        }
    }
}

pub(crate) fn with_operation_lock<R>(target_path: &Path, f: impl FnOnce() -> R) -> R {
    let lock = lock_for_target_path(target_path);
    let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
    f()
}

pub(crate) fn with_operation_observation<T>(
    operation: Operation,
    shell: &Shell,
    program_name: &str,
    planned_target_path: Option<&Path>,
    run: impl FnOnce() -> Result<T>,
    result_target_path: impl Fn(&T) -> Option<PathBuf>,
) -> Result<T> {
    let started_at = Instant::now();
    publish_operation_event(OperationEvent {
        operation,
        phase: OperationEventPhase::Started,
        shell: shell.clone(),
        program_name: program_name.to_owned(),
        trace_id: active_trace_id(),
        target_path: planned_target_path.map(Path::to_path_buf),
        error_code: None,
        retryable: false,
        duration_ms: None,
    });

    match run() {
        Ok(result) => {
            publish_operation_event(OperationEvent {
                operation,
                phase: OperationEventPhase::Succeeded,
                shell: shell.clone(),
                program_name: program_name.to_owned(),
                trace_id: active_trace_id(),
                target_path: result_target_path(&result),
                error_code: None,
                retryable: false,
                duration_ms: Some(started_at.elapsed().as_millis()),
            });
            Ok(result)
        }
        Err(error) => {
            let (error_code, retryable) = operation_error_event_info(&error);
            publish_operation_event(OperationEvent {
                operation,
                phase: OperationEventPhase::Failed,
                shell: shell.clone(),
                program_name: program_name.to_owned(),
                trace_id: active_trace_id(),
                target_path: error
                    .location()
                    .map(Path::to_path_buf)
                    .or_else(|| planned_target_path.map(Path::to_path_buf)),
                error_code,
                retryable,
                duration_ms: Some(started_at.elapsed().as_millis()),
            });
            Err(error)
        }
    }
}

fn operation_error_event_info(error: &Error) -> (Option<&'static str>, bool) {
    if let Error::Failure(report) = error {
        (Some(report.error_code()), report.is_retryable())
    } else {
        (Some(error.error_code()), error.is_retryable())
    }
}

fn publish_operation_event(event: OperationEvent) {
    OPERATION_EVENT_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow().as_ref() {
            hook(&event);
        }
    });
}

pub(crate) fn resolve_default_target_path(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
) -> Result<PathBuf> {
    let target_path = paths::default_install_path(env, shell, program_name)?;
    validate_target_path(&target_path)?;
    Ok(target_path)
}

pub(crate) fn default_target_path_if_valid(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
) -> Option<PathBuf> {
    resolve_default_target_path(env, shell, program_name).ok()
}

pub(crate) fn default_target_path_matches(
    env: &Environment,
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> bool {
    default_target_path_if_valid(env, shell, program_name)
        .is_some_and(|default_path| default_path == target_path)
}

fn active_trace_id() -> u64 {
    OPERATION_TRACE_ID.with(|slot| {
        let trace_id = slot.get();
        if trace_id == 0 {
            let fallback = allocate_trace_id();
            slot.set(fallback);
            fallback
        } else {
            trace_id
        }
    })
}

pub(crate) fn validate_target_path(path: &Path) -> Result<()> {
    if path.is_relative() {
        return Err(Error::InvalidTargetPath {
            path: path.to_path_buf(),
            reason: "target path must be absolute",
        });
    }

    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(Error::InvalidTargetPath {
            path: path.to_path_buf(),
            reason: "target path must be normalized",
        });
    }

    for candidate in path_ancestor_sequence(path) {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::InvalidTargetPath {
                    path: path.to_path_buf(),
                    reason: "target path must not be a symbolic link",
                });
            }
            Ok(metadata) if !metadata.is_dir() && candidate == path => {}
            Ok(metadata) if !metadata.is_dir() => {
                return Err(Error::InvalidTargetPath {
                    path: path.to_path_buf(),
                    reason: "target path parent is not a directory",
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotADirectory => {
                // `NotADirectory` can occur when an ancestor segment is not a real directory.
                return Err(Error::InvalidTargetPath {
                    path: path.to_path_buf(),
                    reason: "target path parent is not a directory",
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Error::io("inspect path", candidate, error));
            }
        }
    }

    Ok(())
}

fn path_ancestor_sequence(path: &Path) -> Vec<PathBuf> {
    let mut parts = Vec::new();
    let mut current = Some(path);

    while let Some(entry) = current {
        parts.push(entry.to_path_buf());
        current = entry.parent();
    }

    parts
}

pub(crate) fn manual_activation_report(
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
    custom_path: bool,
    activation_policy: ActivationPolicy,
) -> Result<ActivationReport> {
    let target = target_path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: target_path.to_path_buf(),
    })?;

    let next_step = match shell {
        Shell::Bash => format!(
            "Source `{target}` from your shell startup file, or move it into the standard bash-completion directory."
        ),
        Shell::Zsh => {
            let expected = format!("_{program_name}");
            if target_path.file_name().and_then(|value| value.to_str()) == Some(expected.as_str()) {
                format!(
                    "Ensure `{target}` is in a directory listed in `fpath`, then run `compinit -i`."
                )
            } else {
                format!(
                    "Rename the file to `{expected}` or load it manually, then ensure its directory is in `fpath` and run `compinit -i`."
                )
            }
        }
        Shell::Fish => {
            format!("Place `{target}` under Fish's completions directory or source it manually.")
        }
        Shell::Powershell => format!(
            "Add `. {}` to `$PROFILE.CurrentUserAllHosts` or another PowerShell profile.",
            powershell_quote(target)
        ),
        Shell::Elvish => format!(
            "Add `eval (slurp < {})` to your Elvish rc.elv.",
            elvish_quote(target)
        ),
        Shell::Other(_) => format!("Activate `{target}` manually for `{shell}`."),
    };

    let reason = if custom_path && matches!(activation_policy, ActivationPolicy::Manual) {
        "A custom install path was provided, so shellcomp skipped automatic activation wiring."
            .to_owned()
    } else {
        match activation_policy {
            ActivationPolicy::AutoManaged => {
                "The shell does not support safe managed activation for this installation target, so manual activation is required."
                    .to_owned()
            }
            ActivationPolicy::Manual => {
            "Automatic activation wiring was skipped because the activation policy is manual."
                .to_owned()
            }
        }
    };

    Ok(ActivationReport {
        mode: ActivationMode::Manual,
        availability: Availability::ManualActionRequired,
        location: Some(target_path.to_path_buf()),
        reason: Some(reason),
        next_step: Some(next_step),
    })
}

pub(crate) fn missing_completion_next_step(
    shell: &Shell,
    program_name: &str,
    target_path: &Path,
) -> Result<String> {
    let target = target_path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: target_path.to_path_buf(),
    })?;

    Ok(match shell {
        Shell::Bash => format!(
            "Run your CLI's completion install command, or install the script at `{target}` and source it from your shell startup file."
        ),
        Shell::Zsh => {
            let expected = format!("_{program_name}");
            if target_path.file_name().and_then(|value| value.to_str()) == Some(expected.as_str()) {
                format!(
                    "Run your CLI's completion install command, or place `{target}` in a directory listed in `fpath` and run `compinit -i`."
                )
            } else {
                format!(
                    "Run your CLI's completion install command, or place the file at `{target}`, rename it to `{expected}`, and ensure its directory is in `fpath` before running `compinit -i`."
                )
            }
        }
        Shell::Fish => format!(
            "Run your CLI's completion install command, or place the completion file at `{target}` manually."
        ),
        Shell::Powershell => format!(
            "Run your CLI's completion install command, or place the script at `{target}` and add `. {}` to a PowerShell profile.",
            powershell_quote(target)
        ),
        Shell::Elvish => format!(
            "Run your CLI's completion install command, or place the script at `{target}` and add `eval (slurp < {})` to rc.elv.",
            elvish_quote(target)
        ),
        Shell::Other(_) => format!(
            "Run your CLI's completion install command, or install `{target}` manually for `{shell}`."
        ),
    })
}

pub(crate) fn push_unique(paths: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub(crate) fn zsh_target_is_autoloadable(program_name: &str, target_path: &Path) -> bool {
    let expected = format!("_{program_name}");
    target_path.file_name().and_then(|value| value.to_str()) == Some(expected.as_str())
}

pub(crate) fn home_env_hint(env: &Environment, shell: &Shell) -> &'static str {
    if matches!(shell, Shell::Powershell) && env.is_windows_platform() {
        "HOME or USERPROFILE"
    } else {
        "HOME"
    }
}

fn powershell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

fn elvish_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

pub(crate) struct FailureContext<'a> {
    pub(crate) operation: Operation,
    pub(crate) shell: &'a Shell,
    pub(crate) target_path: Option<&'a Path>,
    pub(crate) affected_locations: Vec<PathBuf>,
    pub(crate) kind: FailureKind,
}

#[derive(Default)]
pub(crate) struct FailureStatus {
    pub(crate) file_change: Option<FileChange>,
    pub(crate) activation: Option<ActivationReport>,
    pub(crate) cleanup: Option<CleanupReport>,
}

pub(crate) fn failure(
    context: FailureContext<'_>,
    reason: impl Into<String>,
    next_step: Option<String>,
) -> Error {
    failure_with_status(context, FailureStatus::default(), reason, next_step)
}

pub(crate) fn failure_with_status(
    context: FailureContext<'_>,
    status: FailureStatus,
    reason: impl Into<String>,
    next_step: Option<String>,
) -> Error {
    Error::failure(FailureReport {
        operation: context.operation,
        shell: context.shell.clone(),
        target_path: context.target_path.map(Path::to_path_buf),
        affected_locations: context.affected_locations,
        kind: context.kind,
        file_change: status.file_change,
        activation: status.activation,
        cleanup: status.cleanup,
        reason: reason.into(),
        next_step,
        trace_id: active_trace_id(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::tests::temp_dir;
    use crate::{Error, InstallRequest, Shell};

    use super::{install, validate_target_path, with_operation_event_hook, with_operation_lock};

    #[test]
    fn operation_lock_serializes_threads_for_same_target_path() {
        let target = temp_dir("operation-lock").join("tool");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (attempt_tx, attempt_rx) = mpsc::channel::<PathBuf>();
        let (go_tx, go_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let critical = Arc::new(AtomicBool::new(false));

        let handle = {
            let target = target.clone();
            let entered_tx = entered_tx.clone();
            let critical = Arc::clone(&critical);
            let ready_rx = ready_rx;
            let go_rx = go_rx;
            let attempt_tx = attempt_tx;
            thread::spawn(move || {
                ready_rx.recv().expect("thread should receive start signal");
                go_rx.recv().expect("thread should wait for go signal");
                attempt_tx
                    .send(target.clone())
                    .expect("thread should report lock attempt");
                let entered = Instant::now();
                with_operation_lock(&target, || {
                    critical.store(true, Ordering::SeqCst);
                    entered_tx
                        .send(entered)
                        .expect("entered timestamp should be sent");
                    thread::sleep(Duration::from_millis(20));
                    critical.store(false, Ordering::SeqCst);
                });
            })
        };

        with_operation_lock(&target, || {
            ready_tx
                .send(())
                .expect("thread should be signaled while lock is held");
            go_tx
                .send(())
                .expect("thread should be released after main lock is acquired");
            let thread_target = attempt_rx
                .recv()
                .expect("thread should attempt lock while main holds lock");
            assert_eq!(
                thread_target, target,
                "thread should attempt lock for same target"
            );
            assert!(
                !critical.load(Ordering::SeqCst),
                "thread should not execute critical section while main lock is held",
            );
            thread::sleep(Duration::from_millis(120));
        });

        let thread_entered = entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("thread should acquire lock after main");
        let _thread_entered = thread_entered;

        handle.join().expect("thread should finish");
    }

    #[test]
    fn operation_events_capture_duration_for_success() {
        let home = temp_dir("operation-observation-success").join("home");
        let env = crate::infra::env::Environment::test()
            .with_var("HOME", &home)
            .with_var("XDG_DATA_HOME", home.join("data"))
            .without_real_path_lookups();

        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&events);

        with_operation_event_hook(
            Some(Arc::new(move |event| {
                let mut events = observed.lock().expect("event buffer should be usable");
                events.push(event.clone());
            })),
            || {
                let report = install::execute(
                    &env,
                    InstallRequest {
                        shell: Shell::Bash,
                        program_name: "tool",
                        script: b"complete -F _tool tool\n",
                        path_override: None,
                    },
                )
                .expect("install should succeed");

                assert_eq!(report.shell, Shell::Bash);
            },
        );

        let events = events.lock().expect("events should be readable");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, crate::model::OperationEventPhase::Started);
        assert_eq!(events[0].duration_ms, None);
        assert_eq!(
            events[1].phase,
            crate::model::OperationEventPhase::Succeeded
        );
        assert!(events[1].duration_ms.is_some());
        assert_eq!(events[0].trace_id, events[1].trace_id);
    }

    #[test]
    fn operation_events_capture_error_code_for_failures() {
        let env = crate::infra::env::Environment::test()
            .without_var("HOME")
            .without_var("XDG_DATA_HOME")
            .without_real_path_lookups();

        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_hook = Arc::clone(&observed);

        let result = {
            with_operation_event_hook(
                Some(Arc::new(move |event| {
                    let mut events = observed_hook.lock().expect("event buffer should be usable");
                    events.push(event.clone());
                })),
                || {
                    install::execute(
                        &env,
                        InstallRequest {
                            shell: Shell::Bash,
                            program_name: "tool",
                            script: b"complete -F _tool tool\n",
                            path_override: None,
                        },
                    )
                },
            )
        };

        let error = result.expect_err("install should fail when HOME is unavailable");
        let report = error.into_failure().expect("failure should be structured");
        assert_eq!(report.kind, crate::FailureKind::MissingHome);

        let events = observed.lock().expect("events should be readable");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].phase, crate::model::OperationEventPhase::Started);
        assert_eq!(events[1].phase, crate::model::OperationEventPhase::Failed);
        assert_eq!(events[1].error_code, Some("shellcomp.missing_home"));
        assert_eq!(events[1].retryable, false);
        assert!(events[1].duration_ms.is_some());
        assert_eq!(events[0].trace_id, events[1].trace_id);
    }

    #[test]
    fn validate_target_path_rejects_parent_file_path() {
        let temp = temp_dir("validate-target-parent-file");
        let home = temp.join("home");
        std::fs::write(&home, "block").expect(".home target file should be created");
        let error = validate_target_path(&home.join("tool")).expect_err("validation should fail");
        assert!(
            matches!(error, Error::InvalidTargetPath { reason, .. } if reason == "target path parent is not a directory"),
            "unexpected validation error: {error:?}"
        );
    }

    #[test]
    fn validate_target_path_accepts_existing_target_file() {
        let temp = temp_dir("validate-target-existing-file");
        let target = temp.join("tool.bash");
        std::fs::write(&target, "complete -F _tool tool\n")
            .expect(".bashrc target file should be created");
        validate_target_path(&target).expect("existing file target path should pass validation");
    }
}
