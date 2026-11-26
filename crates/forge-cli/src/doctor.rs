use std::path::PathBuf;

use forge::manifest::Manifest;

use crate::RuntimeChoice;
use crate::cef_provisioner::CachedCefProvisioner;
use crate::dev::ProcessRunner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    OptionalMissing,
    RequiredMissing,
}

#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
    pub required_failed: usize,
}

impl DoctorReport {
    pub fn is_healthy(&self) -> bool {
        self.required_failed == 0
    }

    pub fn render(&self) -> String {
        let mut lines = Vec::with_capacity(self.checks.len() + 4);
        lines.push("FORGE Environment Doctor".to_string());
        lines.push("========================".to_string());
        for c in &self.checks {
            let symbol = match c.status {
                CheckStatus::Ok => "✔",
                CheckStatus::OptionalMissing => "!",
                CheckStatus::RequiredMissing => "✖",
            };
            lines.push(format!("  {symbol} {:<30} {}", c.name, c.detail));
            if let Some(hint) = &c.hint {
                lines.push(format!("      hint: {hint}"));
            }
        }
        if self.required_failed == 0 {
            lines.push("\nResult: all required checks passed.".to_string());
        } else {
            lines.push(format!(
                "\nResult: {} required check(s) failed.",
                self.required_failed
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    pub manifest_path: Option<PathBuf>,
    pub runtime_filter: Option<RuntimeChoice>,
    pub cef_cache_dir_override: Option<PathBuf>,
}

/// Diagnoses host Rust, WebView, Chromium CEF, frontend Node.js, and OS bundler prerequisites
/// exclusively through [`ProcessRunner`].
pub fn run_doctor<R: ProcessRunner>(opts: &DoctorOptions, runner: &R) -> DoctorReport {
    let manifest_candidate = opts
        .manifest_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("forge.toml"));
    let loaded_manifest = if manifest_candidate.exists() {
        Manifest::load(&manifest_candidate).ok()
    } else {
        None
    };

    let effective_runtime = opts.runtime_filter.or_else(|| {
        loaded_manifest
            .as_ref()
            .map(|m| RuntimeChoice::from(m.runtime.default))
    });
    let webview_required = effective_runtime != Some(RuntimeChoice::Cef);
    let cef_required = effective_runtime == Some(RuntimeChoice::Cef);

    let needs_node = loaded_manifest.as_ref().is_some_and(|m| {
        m.dev
            .before_dev_command
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || m.dev
                .before_build_command
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    });

    let mut checks = Vec::new();

    // 1. Core Rust Toolchain
    push_tool_check(
        &mut checks,
        runner,
        "rustc",
        &["--version"],
        true,
        "Rust compiler",
        "Install via https://rustup.rs",
    );
    push_tool_check(
        &mut checks,
        runner,
        "cargo",
        &["--version"],
        true,
        "Cargo package manager",
        "Install via https://rustup.rs",
    );

    // 2. Host OS WebView & Desktop UI Libraries
    if cfg!(target_os = "linux") {
        let has_webkit = runner.probe_tool("pkg-config", &["--exists", "webkit2gtk-4.1"]);
        checks.push(make_check(
            "webkit2gtk-4.1 (Linux WebView)",
            has_webkit,
            webview_required,
            "pkg-config webkit2gtk-4.1",
            "sudo apt install libwebkit2gtk-4.1-dev",
        ));

        let has_gtk = runner.probe_tool("pkg-config", &["--exists", "gtk+-3.0"]);
        checks.push(make_check(
            "gtk+-3.0 (Linux windowing)",
            has_gtk,
            webview_required,
            "pkg-config gtk+-3.0",
            "sudo apt install libgtk-3-dev",
        ));

        let has_tray = runner.probe_tool("pkg-config", &["--exists", "ayatana-appindicator3-0.1"])
            || runner.probe_tool("pkg-config", &["--exists", "appindicator3-0.1"]);
        checks.push(make_check(
            "appindicator3 (desktop-ui)",
            has_tray,
            false,
            "system tray library",
            "sudo apt install libayatana-appindicator3-dev",
        ));
    }

    // 3. Chromium CEF Toolchain & Local Binary Cache
    for (tool, label, hint) in [
        ("cmake", "CMake (CEF build)", "sudo apt install cmake"),
        ("ninja", "Ninja (CEF build)", "sudo apt install ninja-build"),
        ("clang", "Clang (CEF build)", "sudo apt install clang"),
    ] {
        push_tool_check(
            &mut checks,
            runner,
            tool,
            &["--version"],
            cef_required,
            label,
            hint,
        );
    }

    let cef_cache_root = opts
        .cef_cache_dir_override
        .clone()
        .unwrap_or_else(|| CachedCefProvisioner::default().cache_root());
    let cef_cached = cef_cache_root.is_dir()
        && std::fs::read_dir(&cef_cache_root)
            .ok()
            .is_some_and(|mut it| it.next().is_some());
    checks.push(DoctorCheck {
        name: "CEF SDK cache".to_string(),
        status: if cef_cached {
            CheckStatus::Ok
        } else {
            CheckStatus::OptionalMissing
        },
        detail: if cef_cached {
            format!("cached at {}", cef_cache_root.display())
        } else {
            "not downloaded yet (auto-provisioned on first `--runtime cef` run)".to_string()
        },
        hint: None,
    });

    // 4. Frontend Node.js Toolchain
    push_tool_check(
        &mut checks,
        runner,
        "node",
        &["--version"],
        needs_node,
        "Node.js runtime",
        "Install Node.js 20+ from https://nodejs.org",
    );
    push_tool_check(
        &mut checks,
        runner,
        "npm",
        &["--version"],
        needs_node,
        "npm package manager",
        "Install Node.js 20+ from https://nodejs.org",
    );

    // 5. OS Packaging Tools
    if cfg!(target_os = "linux") {
        push_tool_check(
            &mut checks,
            runner,
            "dpkg-deb",
            &["--version"],
            false,
            "Debian .deb packager",
            "sudo apt install dpkg",
        );
        push_tool_check(
            &mut checks,
            runner,
            "patchelf",
            &["--version"],
            false,
            "ELF rpath patcher (CEF Linux)",
            "sudo apt install patchelf",
        );
        push_tool_check(
            &mut checks,
            runner,
            "appimagetool",
            &["--version"],
            false,
            "Linux AppImage packager",
            "Download appimagetool-x86_64.AppImage onto PATH",
        );
    } else if cfg!(target_os = "windows") {
        push_tool_check(
            &mut checks,
            runner,
            "makensis",
            &["-VERSION"],
            false,
            "NSIS Windows installer compiler",
            "winget install NSIS.NSIS",
        );
    } else if cfg!(target_os = "macos") {
        push_tool_check(
            &mut checks,
            runner,
            "hdiutil",
            &["help"],
            false,
            "macOS .dmg disk image builder",
            "Included with macOS Command Line Tools",
        );
    }

    let required_failed = checks
        .iter()
        .filter(|c| c.status == CheckStatus::RequiredMissing)
        .count();

    DoctorReport {
        checks,
        required_failed,
    }
}

fn push_tool_check<R: ProcessRunner>(
    checks: &mut Vec<DoctorCheck>,
    runner: &R,
    program: &str,
    args: &[&str],
    required: bool,
    label: &str,
    install_hint: &str,
) {
    let found = runner.probe_tool(program, args);
    checks.push(make_check(label, found, required, program, install_hint));
}

fn make_check(
    label: &str,
    found: bool,
    required: bool,
    detail_name: &str,
    install_hint: &str,
) -> DoctorCheck {
    if found {
        DoctorCheck {
            name: label.to_string(),
            status: CheckStatus::Ok,
            detail: format!("found ({detail_name})"),
            hint: None,
        }
    } else if required {
        DoctorCheck {
            name: label.to_string(),
            status: CheckStatus::RequiredMissing,
            detail: format!("MISSING ({detail_name} is required)"),
            hint: Some(install_hint.to_string()),
        }
    } else {
        DoctorCheck {
            name: label.to_string(),
            status: CheckStatus::OptionalMissing,
            detail: format!("not installed ({detail_name}, optional)"),
            hint: Some(install_hint.to_string()),
        }
    }
}
