use std::process::ExitCode;

use clap::Parser;
use forge_cli::cef_provisioner::CachedCefProvisioner;
use forge_cli::dev::{DevOptions, OsProcessRunner, run_dev};
use forge_cli::new::{NewOptions, create_project};
use forge_cli::{Cli, Commands};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let outcome = match cli.command {
        Commands::New {
            path,
            template,
            runtime,
            forge_path,
        } => create_project(&NewOptions {
            target_dir: path,
            template,
            runtime,
            forge_path,
        })
        .map(|created| {
            println!("Created FORGE project at {}", created.display());
            println!("Next steps:");
            println!("  cd {}", created.display());
            println!("  forge dev");
        }),
        Commands::Dev {
            manifest_path,
            runtime,
            release,
        } => {
            let provisioner = CachedCefProvisioner::default();
            let runner = OsProcessRunner;
            run_dev(
                &DevOptions {
                    manifest_path,
                    runtime_override: runtime,
                    release,
                },
                &provisioner,
                &runner,
            )
        }
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
