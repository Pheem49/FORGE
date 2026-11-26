use std::process::ExitCode;

use clap::Parser;
use forge_cli::bindings::{BindingsOptions, run_bindings};
use forge_cli::build::{BuildOptions, run_build};
use forge_cli::cef_provisioner::CachedCefProvisioner;
use forge_cli::dev::{DevOptions, OsProcessRunner, run_dev};
use forge_cli::doctor::{DoctorOptions, run_doctor};
use forge_cli::new::{NewOptions, create_project};
use forge_cli::permissions::{PermissionsInitOptions, run_permissions_init};
use forge_cli::{Cli, Commands, PermissionsAction};

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
            if template.uses_npm() {
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
                for (label, maybe_path) in [
                    ("Tarball:   ", &artifacts.tarball_path),
                    ("Debian pkg:", &artifacts.deb_path),
                    ("AppImage:  ", &artifacts.appimage_path),
                    ("NSIS setup:", &artifacts.nsis_path),
                    ("Zip:       ", &artifacts.zip_path),
                    ("macOS .app:", &artifacts.macos_app_path),
                    ("macOS .dmg:", &artifacts.dmg_path),
                    ("Checksums: ", &artifacts.checksums_path),
                ] {
                    if let Some(p) = maybe_path {
                        println!("  {label} {}", p.display());
                    }
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
        Commands::Bindings {
            manifest_path,
            out_path,
            runtime,
        } => {
            let provisioner = CachedCefProvisioner::default();
            let runner = OsProcessRunner;
            run_bindings(
                &BindingsOptions {
                    manifest_path,
                    runtime_override: runtime,
                    out_path,
                },
                &provisioner,
                &runner,
            )
            .map(|written| {
                println!("Generated TypeScript bindings at {}", written.display());
            })
        }
        Commands::Doctor {
            manifest_path,
            runtime,
        } => {
            let runner = OsProcessRunner;
            let report = run_doctor(
                &DoctorOptions {
                    manifest_path,
                    runtime_filter: runtime,
                    cef_cache_dir_override: None,
                },
                &runner,
            );
            println!("{}", report.render());
            if report.is_healthy() {
                Ok(())
            } else {
                return ExitCode::FAILURE;
            }
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
