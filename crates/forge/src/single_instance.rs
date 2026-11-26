use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fd_lock::{RwLock, RwLockWriteGuard};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::util::{constant_time_eq, random_hex_token, sanitize_identifier};

/// Payload delivered via the `app:second-instance` push event when a duplicate instance launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondInstancePayload {
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct EndpointInfo {
    port: u16,
    token: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ForwardedLaunch {
    token: String,
    args: Vec<String>,
    cwd: PathBuf,
}

/// Outcome of acquiring a single-instance application lock (`[app] single_instance = true`).
pub enum AcquireOutcome {
    /// This process holds the exclusive lock and is running the loopback handoff listener.
    Primary(SingleInstanceGuard),
    /// Another instance already holds the lock; CLI arguments and `cwd` were forwarded to it.
    SecondaryForwarded,
}

/// RAII guard holding the OS advisory file lock and loopback handoff server for the primary instance.
pub struct SingleInstanceGuard {
    _lock_guard: RwLockWriteGuard<'static, File>,
    _boxed_lock: Box<RwLock<File>>,
    endpoint_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    wakeup_port: u16,
    listener_thread: Option<JoinHandle<()>>,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.wakeup_port));
        if let Some(handle) = self.listener_thread.take() {
            let _ = handle.join();
        }
        let _ = std::fs::remove_file(&self.endpoint_path);
    }
}

impl SingleInstanceGuard {
    pub(crate) fn default_lock_dir() -> PathBuf {
        #[cfg(any(feature = "webview", feature = "cef", feature = "plugin-fs"))]
        {
            dirs::runtime_dir()
                .or_else(dirs::cache_dir)
                .unwrap_or_else(std::env::temp_dir)
                .join("forge-locks")
        }
        #[cfg(not(any(feature = "webview", feature = "cef", feature = "plugin-fs")))]
        {
            std::env::temp_dir().join("forge-locks")
        }
    }

    /// Acquires an exclusive advisory lock inside `lock_dir` for `identifier`, or forwards `args`/`cwd`
    /// over loopback TCP to the existing primary instance.
    pub fn acquire<F>(
        lock_dir: &Path,
        identifier: &str,
        args: Vec<String>,
        cwd: PathBuf,
        on_second_instance: F,
    ) -> Result<AcquireOutcome>
    where
        F: Fn(SecondInstancePayload) + Send + Sync + 'static,
    {
        std::fs::create_dir_all(lock_dir).map_err(|e| {
            Error::Runtime(format!(
                "failed to create single-instance lock directory '{}': {e}",
                lock_dir.display()
            ))
        })?;

        let safe_id = sanitize_identifier(identifier);
        let lock_path = lock_dir.join(format!("{safe_id}.lock"));
        let endpoint_path = lock_dir.join(format!("{safe_id}.endpoint"));

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| {
                Error::Runtime(format!(
                    "failed to open single-instance lock '{}': {e}",
                    lock_path.display()
                ))
            })?;

        let mut boxed_lock = Box::new(RwLock::new(file));
        // SAFETY: `boxed_lock` is heap-allocated and stored alongside `_lock_guard` inside
        // `SingleInstanceGuard`, where `_lock_guard` is declared BEFORE `_boxed_lock` so Rust's
        // deterministic field drop order drops the guard before the `Box<RwLock<File>>`.
        let lock_static: &'static mut RwLock<File> =
            unsafe { &mut *(&mut *boxed_lock as *mut RwLock<File>) };

        match lock_static.try_write() {
            Ok(guard) => {
                let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| {
                    Error::Runtime(format!(
                        "failed to bind single-instance loopback listener: {e}"
                    ))
                })?;
                let port = listener
                    .local_addr()
                    .map_err(|e| Error::Runtime(format!("failed to read listener port: {e}")))?
                    .port();
                let token = random_hex_token::<16>()?;
                write_restricted_endpoint(
                    &endpoint_path,
                    &EndpointInfo {
                        port,
                        token: token.clone(),
                    },
                )?;

                let shutdown = Arc::new(AtomicBool::new(false));
                let shutdown_flag = Arc::clone(&shutdown);
                let callback = Arc::new(on_second_instance);

                let thread = std::thread::spawn(move || {
                    for stream_res in listener.incoming() {
                        if shutdown_flag.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(stream) = stream_res else {
                            continue;
                        };
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let mut buf = Vec::with_capacity(512);
                        if stream.take(65_536).read_to_end(&mut buf).is_ok()
                            && let Ok(msg) = serde_json::from_slice::<ForwardedLaunch>(&buf)
                            && constant_time_eq(msg.token.as_bytes(), token.as_bytes())
                        {
                            callback(SecondInstancePayload {
                                args: msg.args,
                                cwd: msg.cwd,
                            });
                        }
                    }
                });

                Ok(AcquireOutcome::Primary(SingleInstanceGuard {
                    _lock_guard: guard,
                    _boxed_lock: boxed_lock,
                    endpoint_path,
                    shutdown,
                    wakeup_port: port,
                    listener_thread: Some(thread),
                }))
            }
            Err(_) => {
                forward_to_primary(&endpoint_path, args, cwd)?;
                Ok(AcquireOutcome::SecondaryForwarded)
            }
        }
    }
}

fn forward_to_primary(endpoint_path: &Path, args: Vec<String>, cwd: PathBuf) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(800);
    let info: EndpointInfo = loop {
        if let Ok(raw) = std::fs::read_to_string(endpoint_path)
            && let Ok(parsed) = serde_json::from_str::<EndpointInfo>(&raw)
        {
            break parsed;
        }
        if Instant::now() >= deadline {
            return Err(Error::Runtime(format!(
                "another instance holds the lock, but endpoint '{}' was unreadable",
                endpoint_path.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], info.port)),
        Duration::from_secs(2),
    )
    .map_err(|e| Error::Runtime(format!("failed to connect to primary instance: {e}")))?;

    let payload = serde_json::to_vec(&ForwardedLaunch {
        token: info.token,
        args,
        cwd,
    })
    .map_err(|e| Error::Runtime(format!("failed to serialize single-instance payload: {e}")))?;

    stream
        .write_all(&payload)
        .map_err(|e| Error::Runtime(format!("failed to forward arguments to primary: {e}")))?;
    let _ = stream.flush();
    Ok(())
}

fn write_restricted_endpoint(path: &Path, info: &EndpointInfo) -> Result<()> {
    let json = serde_json::to_vec(info)
        .map_err(|e| Error::Runtime(format!("failed to serialize endpoint info: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| Error::Runtime(format!("failed to write endpoint file: {e}")))?;
        f.write_all(&json)
            .map_err(|e| Error::Runtime(format!("failed to write endpoint file: {e}")))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, json)
            .map_err(|e| Error::Runtime(format!("failed to write endpoint file: {e}")))?;
    }
    Ok(())
}
