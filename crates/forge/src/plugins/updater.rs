use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, VerifyingKey};
use http::Uri;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::AppConfig;
use crate::error::{ForgeError, codes};
use crate::manifest::{
    UpdatePlatformEntry, UpdateReleaseManifest, canonical_update_signing_payload,
    current_updater_platform_key,
};
use crate::runtime::WindowHandle;
use crate::sandbox::Sandbox;
use crate::util::{constant_time_eq, sanitize_identifier};
use crate::{App, Plugin, State};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024; // 1 MiB
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB
const INITIAL_BUFFER_CAP: u64 = 1024 * 1024; // Never preallocate > 1 MiB from untrusted Content-Length

/// Metadata returned by `updater.check` when a newer signed release is available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableUpdate {
    pub current_version: String,
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub platform: String,
    pub url: String,
    pub sha256: String,
}

/// Outcome returned by `updater.download_and_install` once Ed25519 & SHA-256 verification succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApplyOutcome {
    /// Binary or `.app` bundle was replaced on disk; restart the app to run the new version.
    InstalledNeedsRestart { version: String },
    /// Platform installer was launched in the background (e.g., Windows NSIS `/S`).
    InstallerLaunched { version: String, path: PathBuf },
    /// Verified artifact was staged into the sandboxed updates directory.
    Staged { version: String, path: PathBuf },
}

/// Seam for fetching remote `latest.json` manifests and update binaries.
pub trait UpdateTransport: Send + Sync + 'static {
    fn fetch_manifest(&self, url: &str) -> Result<UpdateReleaseManifest, ForgeError>;
    fn download_artifact(
        &self,
        url: &str,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ForgeError>;
}

/// Seam for applying or launching a cryptographically verified update package.
pub trait UpdateApplier: Send + Sync + 'static {
    fn apply(&self, staged_artifact: &Path, version: &str) -> Result<ApplyOutcome, ForgeError>;
}

/// Default HTTPS `ureq` transport adapter with HTTPS-only enforcement and bounded reads.
pub struct UreqUpdateTransport {
    agent: ureq::Agent,
}

impl Default for UreqUpdateTransport {
    fn default() -> Self {
        let cfg = ureq::Agent::config_builder()
            .https_only(!cfg!(debug_assertions))
            .max_redirects(5)
            .timeout_global(Some(std::time::Duration::from_secs(60)))
            .build();
        Self { agent: cfg.into() }
    }
}

impl UpdateTransport for UreqUpdateTransport {
    fn fetch_manifest(&self, url: &str) -> Result<UpdateReleaseManifest, ForgeError> {
        let res = self.agent.get(url).call().map_err(|e| {
            ForgeError::internal(
                "UPDATER_NETWORK_ERROR",
                format!("failed to fetch '{url}': {e}"),
            )
        })?;
        let mut limited = res.into_body().into_reader().take(MAX_MANIFEST_BYTES + 1);
        let mut bytes = Vec::with_capacity(4096);
        limited.read_to_end(&mut bytes).map_err(|e| {
            ForgeError::internal(
                "UPDATER_NETWORK_ERROR",
                format!("failed to read '{url}': {e}"),
            )
        })?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ForgeError::bad_request(
                "UPDATER_MANIFEST_TOO_LARGE",
                "remote latest.json exceeds 1 MiB limit",
            ));
        }
        serde_json::from_slice::<UpdateReleaseManifest>(&bytes).map_err(|e| {
            ForgeError::new(
                "UPDATER_INVALID_MANIFEST",
                format!("invalid update manifest JSON from '{url}': {e}"),
            )
        })
    }

    fn download_artifact(
        &self,
        url: &str,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ForgeError> {
        let res = self.agent.get(url).call().map_err(|e| {
            ForgeError::internal(
                "UPDATER_NETWORK_ERROR",
                format!("failed to download update artifact '{url}': {e}"),
            )
        })?;
        let total = res
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(t) = total
            && t > MAX_ARTIFACT_BYTES
        {
            return Err(ForgeError::bad_request(
                "UPDATER_ARTIFACT_TOO_LARGE",
                format!("update artifact exceeds {MAX_ARTIFACT_BYTES} byte limit"),
            ));
        }

        let init_cap = total.unwrap_or(65_536).min(INITIAL_BUFFER_CAP) as usize;
        let mut reader = res.into_body().into_reader();
        let mut out = Vec::with_capacity(init_cap);
        let mut chunk = [0u8; 32 * 1024];
        loop {
            let n = reader.read(&mut chunk).map_err(|e| {
                ForgeError::internal(
                    "UPDATER_NETWORK_ERROR",
                    format!("stream interrupted while downloading '{url}': {e}"),
                )
            })?;
            if n == 0 {
                break;
            }
            if (out.len() + n) as u64 > MAX_ARTIFACT_BYTES {
                return Err(ForgeError::bad_request(
                    "UPDATER_ARTIFACT_TOO_LARGE",
                    format!("update artifact exceeds {MAX_ARTIFACT_BYTES} byte limit"),
                ));
            }
            out.extend_from_slice(&chunk[..n]);
            on_progress(out.len() as u64, total);
        }
        Ok(out)
    }
}

/// Default OS update applier supporting atomic Linux AppImage replacement, silent Windows NSIS launch,
/// macOS `.app.tar.gz` bundle extraction, and safe staging for portable builds.
#[derive(Default)]
pub struct OsUpdateApplier;

impl UpdateApplier for OsUpdateApplier {
    fn apply(&self, staged_artifact: &Path, version: &str) -> Result<ApplyOutcome, ForgeError> {
        #[cfg(target_os = "linux")]
        if let Ok(appimage_env) = std::env::var("APPIMAGE")
            && !appimage_env.trim().is_empty()
            && staged_artifact
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("AppImage"))
        {
            let dest = PathBuf::from(appimage_env);
            let tmp_swap = dest.with_extension("AppImage.forge-new");
            fs::copy(staged_artifact, &tmp_swap).map_err(ForgeError::from)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&tmp_swap, fs::Permissions::from_mode(0o755));
            }
            fs::rename(&tmp_swap, &dest).map_err(ForgeError::from)?;
            return Ok(ApplyOutcome::InstalledNeedsRestart {
                version: version.to_string(),
            });
        }

        #[cfg(target_os = "windows")]
        if staged_artifact
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            std::process::Command::new(staged_artifact)
                .arg("/S")
                .spawn()
                .map_err(|e| {
                    ForgeError::internal(
                        "UPDATER_LAUNCH_FAILED",
                        format!("failed to launch NSIS installer: {e}"),
                    )
                })?;
            return Ok(ApplyOutcome::InstallerLaunched {
                version: version.to_string(),
                path: staged_artifact.to_path_buf(),
            });
        }

        #[cfg(target_os = "macos")]
        if staged_artifact
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".app.tar.gz"))
            && let Ok(exe) = std::env::current_exe()
            && let Some(app_bundle) = find_enclosing_macos_app(&exe)
            && let Some(parent_dir) = app_bundle.parent()
        {
            let file = fs::File::open(staged_artifact).map_err(ForgeError::from)?;
            let gz = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(gz);
            archive.unpack(parent_dir).map_err(ForgeError::from)?;
            return Ok(ApplyOutcome::InstalledNeedsRestart {
                version: version.to_string(),
            });
        }

        Ok(ApplyOutcome::Staged {
            version: version.to_string(),
            path: staged_artifact.to_path_buf(),
        })
    }
}

#[cfg(target_os = "macos")]
fn find_enclosing_macos_app(exe: &Path) -> Option<PathBuf> {
    let mut cursor = Some(exe);
    while let Some(p) = cursor {
        if p.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(p.to_path_buf());
        }
        cursor = p.parent();
    }
    None
}

/// In-memory transport adapter for deterministic, zero-network `App::headless()` tests.
#[derive(Clone, Default)]
pub struct MemoryUpdateTransport {
    manifests: Arc<Mutex<HashMap<String, UpdateReleaseManifest>>>,
    artifacts: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemoryUpdateTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_manifest(&self, url: impl Into<String>, manifest: UpdateReleaseManifest) {
        if let Ok(mut map) = self.manifests.lock() {
            map.insert(url.into(), manifest);
        }
    }

    pub fn insert_artifact(&self, url: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        if let Ok(mut map) = self.artifacts.lock() {
            map.insert(url.into(), bytes.into());
        }
    }
}

impl UpdateTransport for MemoryUpdateTransport {
    fn fetch_manifest(&self, url: &str) -> Result<UpdateReleaseManifest, ForgeError> {
        self.manifests
            .lock()
            .ok()
            .and_then(|m| m.get(url).cloned())
            .ok_or_else(|| {
                ForgeError::not_found(
                    "UPDATER_NOT_FOUND",
                    format!("mock manifest '{url}' missing"),
                )
            })
    }

    fn download_artifact(
        &self,
        url: &str,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, ForgeError> {
        let bytes = self
            .artifacts
            .lock()
            .ok()
            .and_then(|m| m.get(url).cloned())
            .ok_or_else(|| {
                ForgeError::not_found(
                    "UPDATER_NOT_FOUND",
                    format!("mock artifact '{url}' missing"),
                )
            })?;
        let total = bytes.len() as u64;
        on_progress(total, Some(total));
        Ok(bytes)
    }
}

#[derive(Clone)]
struct UpdaterConfigState {
    transport: Arc<dyn UpdateTransport>,
    applier: Arc<dyn UpdateApplier>,
    staging_dir_override: Option<PathBuf>,
    platform_override: Option<String>,
    allow_loopback_http: bool,
}

/// Ed25519-verified static release updater plugin (`updater.check`, `updater.download_and_install`).
pub struct UpdaterPlugin {
    inner: UpdaterConfigState,
}

impl Default for UpdaterPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdaterPlugin {
    pub fn new() -> Self {
        Self {
            inner: UpdaterConfigState {
                transport: Arc::new(UreqUpdateTransport::default()),
                applier: Arc::new(OsUpdateApplier),
                staging_dir_override: None,
                platform_override: None,
                allow_loopback_http: cfg!(debug_assertions),
            },
        }
    }

    pub fn with_transport(mut self, transport: impl UpdateTransport) -> Self {
        self.inner.transport = Arc::new(transport);
        self
    }

    pub fn with_applier(mut self, applier: impl UpdateApplier) -> Self {
        self.inner.applier = Arc::new(applier);
        self
    }

    pub fn with_staging_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.inner.staging_dir_override = Some(dir.into());
        self
    }

    pub fn with_platform_key(mut self, platform: impl Into<String>) -> Self {
        self.inner.platform_override = Some(platform.into());
        self
    }

    pub fn allow_loopback_http(mut self, allow: bool) -> Self {
        self.inner.allow_loopback_http = allow;
        self
    }
}

impl Plugin for UpdaterPlugin {
    fn apply(self, app: App) -> App {
        app.state(self.inner)
            .command("updater.check", updater_check)
            .command("updater.download_and_install", updater_download_and_install)
    }
}

async fn updater_check(
    cfg: AppConfig,
    state: State<UpdaterConfigState>,
) -> Result<Option<AvailableUpdate>, ForgeError> {
    let resolved = tokio::task::spawn_blocking(move || resolve_available_update(&cfg, &state))
        .await
        .map_err(|e| ForgeError::internal("UPDATER_TASK_FAILED", e.to_string()))??;
    Ok(resolved.map(|(avail, _, _)| avail))
}

async fn updater_download_and_install(
    cfg: AppConfig,
    state: State<UpdaterConfigState>,
    win: WindowHandle,
) -> Result<ApplyOutcome, ForgeError> {
    tokio::task::spawn_blocking(move || {
        let Some((avail, entry, verifying_key)) = resolve_available_update(&cfg, &state)? else {
            return Err(ForgeError::bad_request(
                "UPDATER_UP_TO_DATE",
                "application is already on the latest version",
            ));
        };

        validate_update_url(&entry.url, state.allow_loopback_http)?;

        let win_for_progress = win.clone();
        let mut emit_progress = |downloaded: u64, total: Option<u64>| {
            let _ = win_for_progress.emit(
                "updater:progress",
                json!({
                    "downloaded_bytes": downloaded,
                    "total_bytes": total,
                }),
            );
        };

        let bytes = state
            .transport
            .download_artifact(&entry.url, &mut emit_progress)?;

        verify_artifact_integrity(
            &verifying_key,
            &avail.version,
            &avail.platform,
            &entry,
            &bytes,
        )?;

        let stage_root = match &state.staging_dir_override {
            Some(dir) => dir.clone(),
            None => {
                let base = dirs::data_local_dir()
                    .or_else(dirs::cache_dir)
                    .ok_or_else(|| {
                        ForgeError::internal(
                            "UPDATER_NO_DATA_DIR",
                            "unable to resolve private user data directory for update staging",
                        )
                    })?;
                base.join(sanitize_identifier(&cfg.app.identifier))
                    .join("updates")
            }
        };

        // Enforce canonical Sandbox containment (`AGENTS.md` §4.3) before writing staged update files
        let sandbox = Sandbox::new(&stage_root)?;
        let file_name = infer_safe_filename(&entry.url, &cfg.app.name, &avail.version);
        let staged_path = sandbox.prepare_write_path(&file_name)?;
        let tmp_name = format!("{file_name}.part");
        let tmp_path = sandbox.prepare_write_path(&tmp_name)?;

        fs::write(&tmp_path, &bytes).map_err(ForgeError::from)?;
        #[cfg(unix)]
        if staged_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("AppImage"))
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755));
        }
        fs::rename(&tmp_path, &staged_path).map_err(ForgeError::from)?;

        state.applier.apply(&staged_path, &avail.version)
    })
    .await
    .map_err(|e| ForgeError::internal("UPDATER_TASK_FAILED", e.to_string()))?
}

fn resolve_available_update(
    cfg: &AppConfig,
    state: &UpdaterConfigState,
) -> Result<Option<(AvailableUpdate, UpdatePlatformEntry, VerifyingKey)>, ForgeError> {
    if cfg.updater.endpoints.is_empty() {
        return Err(ForgeError::bad_request(
            "UPDATER_UNCONFIGURED",
            "no [updater].endpoints configured in forge.toml",
        ));
    }
    let raw_pubkey = cfg
        .updater
        .pubkey
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            ForgeError::bad_request(
                "UPDATER_UNCONFIGURED",
                "missing [updater].pubkey in forge.toml (generate one with `forge signer keygen`)",
            )
        })?;
    let verifying_key = decode_verifying_key(raw_pubkey)?;

    let current_ver = Version::parse(cfg.app.version.trim()).map_err(|e| {
        ForgeError::bad_request(
            "UPDATER_INVALID_VERSION",
            format!("invalid current [app].version '{}': {e}", cfg.app.version),
        )
    })?;

    let platform_key = state
        .platform_override
        .clone()
        .unwrap_or_else(current_updater_platform_key);

    let mut last_err = None;
    for endpoint in &cfg.updater.endpoints {
        validate_update_url(endpoint, state.allow_loopback_http)?;
        match state.transport.fetch_manifest(endpoint) {
            Ok(manifest) => {
                let remote_ver = Version::parse(manifest.version.trim()).map_err(|e| {
                    ForgeError::new(
                        "UPDATER_INVALID_MANIFEST",
                        format!("invalid remote version '{}': {e}", manifest.version),
                    )
                })?;
                if remote_ver <= current_ver {
                    return Ok(None);
                }
                let Some(entry) = manifest.platforms.get(&platform_key).cloned() else {
                    return Ok(None);
                };
                validate_update_url(&entry.url, state.allow_loopback_http)?;

                verify_manifest_signature(
                    &verifying_key,
                    &manifest.version,
                    &platform_key,
                    &entry,
                )?;

                return Ok(Some((
                    AvailableUpdate {
                        current_version: cfg.app.version.clone(),
                        version: manifest.version,
                        notes: manifest.notes,
                        pub_date: manifest.pub_date,
                        platform: platform_key,
                        url: entry.url.clone(),
                        sha256: entry.sha256.clone(),
                    },
                    entry,
                    verifying_key,
                )));
            }
            Err(err) => {
                last_err = Some(err);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        ForgeError::internal("UPDATER_NETWORK_ERROR", "all update endpoints failed")
    }))
}

fn validate_update_url(url: &str, allow_loopback_http: bool) -> Result<(), ForgeError> {
    let trimmed = url.trim();
    let parsed: Uri = trimmed.parse().map_err(|e| {
        ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            format!("invalid update URL '{trimmed}': {e}"),
        )
    })?;
    let Some(host) = parsed.host() else {
        return Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            format!("update URL '{trimmed}' is missing a host"),
        ));
    };
    match parsed.scheme_str() {
        Some("https") if !host.is_empty() => Ok(()),
        Some("http")
            if allow_loopback_http
                && (host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1") =>
        {
            Ok(())
        }
        _ => Err(ForgeError::forbidden(
            codes::FORBIDDEN,
            format!("update URL '{trimmed}' must use HTTPS"),
        )),
    }
}

fn decode_verifying_key(raw_b64: &str) -> Result<VerifyingKey, ForgeError> {
    let bytes = BASE64.decode(raw_b64.trim()).map_err(|e| {
        ForgeError::bad_request(
            "UPDATER_INVALID_PUBKEY",
            format!("invalid base64 [updater].pubkey: {e}"),
        )
    })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| {
        ForgeError::bad_request(
            "UPDATER_INVALID_PUBKEY",
            "[updater].pubkey must decode to exactly 32 Ed25519 bytes",
        )
    })?;
    VerifyingKey::from_bytes(&arr).map_err(|e| {
        ForgeError::bad_request(
            "UPDATER_INVALID_PUBKEY",
            format!("invalid Ed25519 public key: {e}"),
        )
    })
}

fn verify_manifest_signature(
    verifying_key: &VerifyingKey,
    version: &str,
    platform: &str,
    entry: &UpdatePlatformEntry,
) -> Result<(), ForgeError> {
    let sig_bytes = BASE64.decode(entry.signature.trim()).map_err(|e| {
        ForgeError::forbidden(
            "UPDATER_BAD_SIGNATURE",
            format!("malformed base64 signature in update manifest: {e}"),
        )
    })?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|e| {
        ForgeError::forbidden(
            "UPDATER_BAD_SIGNATURE",
            format!("invalid 64-byte Ed25519 signature length: {e}"),
        )
    })?;
    let msg = canonical_update_signing_payload(version, platform, &entry.url, &entry.sha256);
    verifying_key.verify_strict(&msg, &sig).map_err(|_| {
        ForgeError::forbidden(
            "UPDATER_BAD_SIGNATURE",
            "Ed25519 signature verification failed for update manifest entry",
        )
    })
}

fn verify_artifact_integrity(
    verifying_key: &VerifyingKey,
    version: &str,
    platform: &str,
    entry: &UpdatePlatformEntry,
    bytes: &[u8],
) -> Result<(), ForgeError> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut computed_hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut computed_hex, "{b:02x}");
    }

    let expected_hex = entry.sha256.trim().to_ascii_lowercase();
    if !constant_time_eq(computed_hex.as_bytes(), expected_hex.as_bytes()) {
        return Err(ForgeError::forbidden(
            "UPDATER_HASH_MISMATCH",
            format!(
                "downloaded update SHA-256 mismatch (expected {expected_hex}, got {computed_hex})"
            ),
        ));
    }

    verify_manifest_signature(verifying_key, version, platform, entry)
}

fn infer_safe_filename(url: &str, app_name: &str, version: &str) -> String {
    let tail = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let cleaned: String = tail
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        format!("{}-{version}.bin", sanitize_identifier(app_name))
    } else {
        trimmed.to_string()
    }
}
