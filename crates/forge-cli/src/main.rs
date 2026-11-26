use std::process::ExitCode;

use clap::Parser;
use forge_cli::build::{BuildOptions, run_build};
use forge_cli::cef_provisioner::CachedCefProvisioner;
use forge_cli::dev::{DevOptions, OsProcessRunner, run_dev};
use forge_cli::new::{NewOptions, create_project};
use forge_cli::permissions::{PermissionsInitOptions, run_permissions_init};
use forge_cli::{Cli, Commands, PermissionsAction, TemplateChoice};

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
            if template == TemplateChoice::React {
                println!("  npm install");
            }
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
        Commands::Build {
            manifest_path,
            runtime,
            format,
            out_dir,
        } => {
            let provisioner = CachedCefProvisioner::default();
            let runner = OsProcessRunner;
            run_build(
                &BuildOptions {
                    manifest_path,
                    runtime_override: runtime,
                    format,
                    out_dir,
                },
                &provisioner,
                &runner,
            )
            .map(|artifacts| {
                println!("Built standalone bundle: {}", artifacts.app_dir.display());
                println!("  Executable: {}", artifacts.binary_path.display());
                if let Some(tar) = artifacts.tarball_path {
                    println!("  Tarball:    {}", tar.display());
                }
                if let Some(deb) = artifacts.deb_path {
                    println!("  Debian pkg: {}", deb.display());
                }
            })
        }
        Commands::Permissions { action } => match action {
            PermissionsAction::Init {
                manifest_path,
                runtime,
            } => {
                let provisioner = CachedCefProvisioner::default();
                let runner = OsProcessRunner;
                run_permissions_init(
                    &PermissionsInitOptions {
                        manifest_path,
                        runtime_override: runtime,
                    },
                    &provisioner,
                    &runner,
                )
                .map(|allowed| {
                    println!(
                        "Updated [permissions] in forge.toml ({} allowed entries):",
                        allowed.len()
                    );
                    for entry in allowed {
                        println!("  - {entry}");
                    }
                })
            }
        },
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
