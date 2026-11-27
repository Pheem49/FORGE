use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::bundle::staged::StagedApp;
use crate::bundle::target::BundleTarget;
use crate::dev::{CommandSpec, ProcessRunner};
use crate::{CliError, Result, RuntimeChoice};

pub struct NsisTarget;

impl BundleTarget for NsisTarget {
    fn id(&self) -> &'static str {
        "nsis"
    }

    fn supported_on_host(&self) -> bool {
        cfg!(target_os = "windows")
    }

    fn required_tool(&self) -> Option<&'static str> {
        Some("makensis")
    }

    fn package(
        &self,
        app: &StagedApp,
        runner: &dyn ProcessRunner,
        out_dir: &Path,
    ) -> Result<PathBuf> {
        let out_installer = out_dir.join(format!(
            "{}-{}-{}-setup.exe",
            app.manifest.app.name,
            app.manifest.app.version,
            std::env::consts::ARCH
        ));
        let nsi_path = app
            .staging_dir
            .join(format!("{}-installer.nsi", app.manifest.app.name));

        let bin_filename = app
            .binary_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&app.bin_name);
        let uninst_key = format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
            app.manifest.app.identifier
        );

        let webview2_check = if app.runtime == RuntimeChoice::Webview {
            r#"
  ; Check for Microsoft Edge WebView2 Evergreen Runtime
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" 0 +3
  ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  StrCmp $0 "" 0 +2
  MessageBox MB_ICONINFORMATION|MB_OK "Microsoft Edge WebView2 Runtime was not detected. Visit https://go.microsoft.com/fwlink/p/?LinkId=2124703 if the application does not start."
"#
        } else {
            ""
        };

        let safe_title = nsis_escape(&app.manifest.window.title);
        let safe_publisher = nsis_escape(app.publisher());
        let safe_version = nsis_escape(&app.manifest.app.version);

        let mut scheme_install = String::new();
        let mut scheme_uninstall = String::new();
        for scheme in &app.manifest.app.schemes {
            scheme_install.push_str(&format!(
                "  WriteRegStr HKCU \"Software\\Classes\\{scheme}\" \"\" \"URL:{safe_title} Protocol\"\n  WriteRegStr HKCU \"Software\\Classes\\{scheme}\" \"URL Protocol\" \"\"\n  WriteRegStr HKCU \"Software\\Classes\\{scheme}\\DefaultIcon\" \"\" '\"$INSTDIR\\icon.ico\",0'\n  WriteRegStr HKCU \"Software\\Classes\\{scheme}\\shell\\open\\command\" \"\" '\"$INSTDIR\\{bin_filename}\" \"%1\"'\n"
            ));
            scheme_uninstall.push_str(&format!(
                "  DeleteRegKey HKCU \"Software\\Classes\\{scheme}\"\n"
            ));
        }

        let script = format!(
            r#"!include "MUI2.nsh"
Unicode true
Name "{title}"
OutFile "{out_file}"
InstallDir "$LOCALAPPDATA\{app_name}"
RequestExecutionLevel user
!define MUI_ICON "{ico_path}"
!define MUI_UNICON "{ico_path}"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
{webview2_check}
  SetOutPath "$INSTDIR"
  File /r "{app_dir}\*.*"
  File "{ico_path}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\{title}"
  CreateShortcut "$SMPROGRAMS\{title}\{title}.lnk" "$INSTDIR\{bin_filename}" "" "$INSTDIR\icon.ico"
  WriteRegStr HKCU "{uninst_key}" "DisplayName" "{title}"
  WriteRegStr HKCU "{uninst_key}" "DisplayVersion" "{version}"
  WriteRegStr HKCU "{uninst_key}" "Publisher" "{publisher}"
  WriteRegStr HKCU "{uninst_key}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
{scheme_install}SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\{title}\{title}.lnk"
  RMDir "$SMPROGRAMS\{title}"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "{uninst_key}"
{scheme_uninstall}SectionEnd
"#,
            title = safe_title,
            out_file = out_installer.display(),
            app_name = app.manifest.app.name,
            ico_path = app.icons.ico_path.display(),
            webview2_check = webview2_check,
            app_dir = app.app_dir.display(),
            bin_filename = bin_filename,
            uninst_key = uninst_key,
            version = safe_version,
            publisher = safe_publisher,
            scheme_install = scheme_install,
            scheme_uninstall = scheme_uninstall,
        );

        fs::write(&nsi_path, script).map_err(|e| CliError::Io {
            path: nsi_path.clone(),
            message: format!("failed to write NSIS script: {e}"),
        })?;

        let spec = CommandSpec {
            program: "makensis".to_string(),
            args: vec![nsi_path.to_string_lossy().into_owned()],
            current_dir: app.project_root.clone(),
            env: BTreeMap::new(),
        };
        let code = runner.run(&spec)?;
        if code != 0 {
            return Err(CliError::ProcessFailed {
                command: "makensis".to_string(),
                code,
            });
        }

        Ok(out_installer)
    }
}

fn nsis_escape(raw: &str) -> String {
    raw.replace('$', "$$")
        .replace('"', "$\\\"")
        .replace(['\r', '\n'], " ")
}
