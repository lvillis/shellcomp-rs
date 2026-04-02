# shellcomp

`shellcomp` is a deployment layer for shell completions in Rust CLI projects.

It does not generate completion scripts. It installs, wires, detects, and removes them in a way
that is predictable, idempotent, and structured for callers.

## What It Handles

- Default install paths for Bash, Zsh, Fish, PowerShell, and Elvish
- `write-if-changed` completion file writes
- Managed `~/.bashrc` fallback wiring when Bash has no system loader
- Managed `~/.zshrc` wiring for `fpath` and `compinit`
- Native Fish completion directory installs
- Structured manual activation guidance for PowerShell and Elvish
- Symmetric uninstall cleanup
- Structured reports that callers can render however they want

## Supported Shells

- Bash
- Zsh
- Fish
- PowerShell
- Elvish

`Shell::Other(_)` remains the explicit unsupported-shell escape hatch.

## Add The Dependency

Until the crate is published, depend on a local checkout or a git revision:

```toml
[dependencies]
shellcomp = { path = "../shellcomp-rs" }
```

Once published, replace the path dependency with a version:

```toml
[dependencies]
shellcomp = "0.1.0"
```

Enable `clap` integration if you want the library to render completions from `clap::CommandFactory`
directly:

```toml
[dependencies]
shellcomp = { version = "0.1.0", features = ["clap"] }
clap = { version = "4.6.0", features = ["derive"] }
```

## Install A Prebuilt Completion Script

```rust
use shellcomp::{InstallRequest, Shell, install};

fn install_bash_completion(script: &[u8]) -> shellcomp::Result<()> {
    let report = install(InstallRequest {
        shell: Shell::Bash,
        program_name: "my-cli",
        script,
        path_override: None,
    })?;

    println!("{report:#?}");
    Ok(())
}
```

## Integrate With clap

```rust
use clap::Parser;
use shellcomp::{InstallRequest, Shell, install, render_clap_completion};

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    verbose: bool,
}

fn install_zsh_completion() -> Result<(), Box<dyn std::error::Error>> {
    let script = render_clap_completion::<Cli>(Shell::Zsh, "my-cli")?;
    let report = install(InstallRequest {
        shell: Shell::Zsh,
        program_name: "my-cli",
        script: &script,
        path_override: None,
    })?;

    println!("{report:#?}");
    Ok(())
}
```

The examples above install into managed shell locations. Use `path_override` during local testing if
you do not want to touch your real shell profile yet.

## Uninstall

```rust
use shellcomp::{Shell, UninstallRequest, uninstall};

fn remove_fish_completion() -> shellcomp::Result<()> {
    let report = uninstall(UninstallRequest {
        shell: Shell::Fish,
        program_name: "my-cli",
        path_override: None,
    })?;

    println!("{report:#?}");
    Ok(())
}
```

## Custom Paths

When `path_override` is set, `install` keeps the legacy behavior for non-default custom paths and
reports `ActivationMode::Manual`. If the override is exactly the shell's managed default path, the
default activation semantics still apply. If you want a custom path plus managed Bash/Zsh
activation, use `install_with_policy(..., ActivationPolicy::AutoManaged)`.

```rust
use std::path::PathBuf;

use shellcomp::{InstallRequest, Shell, install};

fn install_to_custom_path(script: &[u8]) -> shellcomp::Result<()> {
    let report = install(InstallRequest {
        shell: Shell::Bash,
        program_name: "my-cli",
        script,
        path_override: Some(PathBuf::from("/tmp/my-cli.bash")),
    })?;

    assert_eq!(report.activation.mode, shellcomp::ActivationMode::Manual);
    Ok(())
}
```

## Examples

- `cargo run --example install_prebuilt`
- `cargo run --example roundtrip_custom_path`
- `cargo run --example inspect_managed_paths`
- `cargo run --example clap_integration --features clap`
