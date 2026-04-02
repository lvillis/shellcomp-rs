use std::env;

use clap::Parser;
use shellcomp::{InstallRequest, Shell, install, render_clap_completion};

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    verbose: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let script = render_clap_completion::<Cli>(Shell::Bash, "example-cli")?;
    let demo_path = env::temp_dir().join(format!("example-cli-{}.bash", std::process::id()));
    let report = install(InstallRequest {
        shell: Shell::Bash,
        program_name: "example-cli",
        script: &script,
        path_override: Some(demo_path.clone()),
    })?;

    println!("Rendered completion from clap and installed it to a temporary path.");
    println!("Path: {}", demo_path.display());
    println!("{report:#?}");
    Ok(())
}
