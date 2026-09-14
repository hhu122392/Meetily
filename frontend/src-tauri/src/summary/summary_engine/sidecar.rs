// Sidecar process lifecycle management for llama-helper
// Handles spawning, health checking, keep-alive, and graceful shutdown

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::summary::measurement;
use anyhow::{anyhow, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock};

#[cfg(not(windows))]
use std::process::Stdio;
#[cfg(not(windows))]
use tokio::process::{Child, ChildStdin, ChildStdout};

#[cfg(windows)]
use crate::moss_helper::windows_job::{self, JobControl};

use super::models;

#[cfg(windows)]
type SidecarChild = Arc<JobControl>;
#[cfg(not(windows))]
type SidecarChild = Child;

#[cfg(windows)]
type SidecarStdin = tokio::fs::File;
#[cfg(not(windows))]
type SidecarStdin = ChildStdin;

#[cfg(windows)]
type SidecarStdout = tokio::fs::File;
#[cfg(not(windows))]
type SidecarStdout = ChildStdout;

// ============================================================================
// Sidecar State Management
// ============================================================================

/// Sidecar process manager with keep-alive and health monitoring
pub struct SidecarManager {
    /// Child process handle
    child_process: Arc<Mutex<Option<SidecarChild>>>,

    /// Stdin writer for sending requests
    stdin_writer: Arc<Mutex<Option<SidecarStdin>>>,

    /// Stdout reader for receiving responses
    stdout_reader: Arc<Mutex<Option<BufReader<SidecarStdout>>>>,

    /// Serializes shutdown so every caller observes the same idempotent close.
    shutdown_lock: Arc<Mutex<()>>,

    /// Serializes complete generation lifecycles. Each request now owns one
    /// helper from startup through verified shutdown, so a second summary cannot
    /// enqueue work on a helper the first summary is about to close.
    generation_lock: Arc<Mutex<()>>,

    /// Last activity timestamp
    last_activity: Arc<RwLock<Instant>>,

    /// Health status
    is_healthy: Arc<AtomicBool>,

    /// Shutdown flag
    should_shutdown: Arc<AtomicBool>,

    /// Active request count (for graceful shutdown)
    active_request_count: Arc<AtomicUsize>,

    /// Serializes a complete stdin request/stdout response exchange.
    ///
    /// Locking stdin and stdout independently is not enough: a health-check ping
    /// can otherwise be written between a generation request and its response,
    /// then consume the generation response (or hand `pong` to the generator).
    request_io_lock: Arc<Mutex<()>>,

    /// Identifies the currently running process lifecycle. Background tasks from
    /// an older model process must not become active again after `spawn` clears
    /// the shared shutdown flag for the replacement process.
    lifecycle_generation: Arc<AtomicUsize>,

    /// Path to llama-helper binary
    helper_binary_path: PathBuf,

    /// Production uses no arguments. Tests inject a protocol-compatible helper
    /// through an exact executable path and explicit argument list.
    helper_arguments: Vec<OsString>,

    /// Current model path (if loaded)
    current_model_path: Arc<RwLock<Option<PathBuf>>>,

    /// D-12 generation that owns the current helper lifecycle.
    measurement_generation_id: Arc<RwLock<Option<String>>>,

    /// Idle timeout in seconds (configurable via env var)
    idle_timeout_secs: u64,

    /// Background health/idle workers share state but must never run Drop cleanup.
    cleanup_on_drop: bool,
}

/// RAII guard for tracking active requests
/// Decrements the active request count when dropped
struct RequestGuard {
    counter: Arc<AtomicUsize>,
}

impl RequestGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SidecarManager {
    /// Create a new sidecar manager
    pub fn new(_summary_models_dir: PathBuf) -> Result<Self> {
        let helper_binary_path = Self::resolve_helper_binary()?;

        // Get idle timeout from env var or use default
        let idle_timeout_secs = std::env::var("LLAMA_IDLE_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(models::DEFAULT_IDLE_TIMEOUT_SECS);

        log::info!(
            "SidecarManager initialized with idle timeout: {}s",
            idle_timeout_secs
        );
        log::info!("Helper binary path: {}", helper_binary_path.display());

        Ok(Self {
            child_process: Arc::new(Mutex::new(None)),
            stdin_writer: Arc::new(Mutex::new(None)),
            stdout_reader: Arc::new(Mutex::new(None)),
            shutdown_lock: Arc::new(Mutex::new(())),
            generation_lock: Arc::new(Mutex::new(())),
            last_activity: Arc::new(RwLock::new(Instant::now())),
            is_healthy: Arc::new(AtomicBool::new(false)),
            should_shutdown: Arc::new(AtomicBool::new(false)),
            active_request_count: Arc::new(AtomicUsize::new(0)),
            request_io_lock: Arc::new(Mutex::new(())),
            lifecycle_generation: Arc::new(AtomicUsize::new(0)),
            helper_binary_path,
            helper_arguments: Vec::new(),
            current_model_path: Arc::new(RwLock::new(None)),
            measurement_generation_id: Arc::new(RwLock::new(None)),
            idle_timeout_secs,
            cleanup_on_drop: true,
        })
    }

    #[cfg(test)]
    fn new_for_test(
        helper_binary_path: PathBuf,
        helper_arguments: Vec<OsString>,
        idle_timeout_secs: u64,
    ) -> Self {
        Self {
            child_process: Arc::new(Mutex::new(None)),
            stdin_writer: Arc::new(Mutex::new(None)),
            stdout_reader: Arc::new(Mutex::new(None)),
            shutdown_lock: Arc::new(Mutex::new(())),
            generation_lock: Arc::new(Mutex::new(())),
            last_activity: Arc::new(RwLock::new(Instant::now())),
            is_healthy: Arc::new(AtomicBool::new(false)),
            should_shutdown: Arc::new(AtomicBool::new(false)),
            active_request_count: Arc::new(AtomicUsize::new(0)),
            request_io_lock: Arc::new(Mutex::new(())),
            lifecycle_generation: Arc::new(AtomicUsize::new(0)),
            helper_binary_path,
            helper_arguments,
            current_model_path: Arc::new(RwLock::new(None)),
            measurement_generation_id: Arc::new(RwLock::new(None)),
            idle_timeout_secs,
            cleanup_on_drop: true,
        }
    }

    /// Resolve the path to llama-helper binary
    fn resolve_helper_binary() -> Result<PathBuf> {
        // 1. Check environment variable (dev mode or manual override)
        if let Ok(env_path) = std::env::var("MEETILY_LLAMA_HELPER") {
            if !env_path.is_empty() {
                let path = PathBuf::from(env_path);
                if path.exists() {
                    log::info!(
                        "Using llama-helper from MEETILY_LLAMA_HELPER: {}",
                        path.display()
                    );
                    return Ok(path);
                }
            }
        }

        // In production, Tauri bundles the binary with target triple suffix
        // 2. Check relative to current executable (most reliable for AppImage/bundled apps)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                log::info!(
                    "Searching for llama-helper relative to executable: {}",
                    exe_dir.display()
                );

                // Get the target triple (same logic as before)
                let target_triple = std::env::var("TARGET").unwrap_or_else(|_| {
                    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
                    {
                        "x86_64-unknown-linux-gnu".to_string()
                    }
                    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
                    {
                        "aarch64-unknown-linux-gnu".to_string()
                    }
                    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
                    {
                        "x86_64-apple-darwin".to_string()
                    }
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    {
                        "aarch64-apple-darwin".to_string()
                    }
                    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
                    {
                        "x86_64-pc-windows-msvc".to_string()
                    }
                    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
                    {
                        "aarch64-pc-windows-msvc".to_string()
                    }
                    #[cfg(not(any(
                        all(
                            target_os = "linux",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "macos",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        ),
                        all(
                            target_os = "windows",
                            any(target_arch = "x86_64", target_arch = "aarch64")
                        )
                    )))]
                    {
                        "unknown".to_string()
                    }
                });

                let binary_name = if cfg!(windows) {
                    format!("llama-helper-{}.exe", target_triple)
                } else {
                    format!("llama-helper-{}", target_triple)
                };

                // Try exact match in exe dir
                let bundled = exe_dir.join(&binary_name);
                if bundled.exists() {
                    log::info!(
                        "Found exact match next to executable: {}",
                        bundled.display()
                    );
                    return Ok(bundled);
                }

                // Fuzzy match in exe dir
                log::info!("Attempting fuzzy match in exe dir: {}", exe_dir.display());
                if let Ok(entries) = std::fs::read_dir(exe_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("llama-helper") && !name.ends_with(".d") {
                                log::info!(
                                    "Found fuzzy match next to executable: {}",
                                    path.display()
                                );
                                return Ok(path);
                            }
                        }
                    }
                }
            }
        }

        // 3. Check bundled resources (RESOURCE_DIR) - Fallback
        if let Ok(resource_dir) = std::env::var("RESOURCE_DIR") {
            log::info!(
                "Searching for llama-helper in RESOURCE_DIR: {}",
                resource_dir
            );
            let resource_path = PathBuf::from(&resource_dir);
            // Get the target triple again (or we could have shared it, but code duplication is safer for this tool usage)
            let target_triple = std::env::var("TARGET").unwrap_or_else(|_| {
                #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
                {
                    "x86_64-unknown-linux-gnu".to_string()
                }
                // ... (abbreviated for brevity in thought, but must be full in tool)
                #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
                {
                    "aarch64-unknown-linux-gnu".to_string()
                }
                #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
                {
                    "x86_64-apple-darwin".to_string()
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                {
                    "aarch64-apple-darwin".to_string()
                }
                #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
                {
                    "x86_64-pc-windows-msvc".to_string()
                }
                #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
                {
                    "aarch64-pc-windows-msvc".to_string()
                }
                #[cfg(not(any(
                    all(
                        target_os = "linux",
                        any(target_arch = "x86_64", target_arch = "aarch64")
                    ),
                    all(
                        target_os = "macos",
                        any(target_arch = "x86_64", target_arch = "aarch64")
                    ),
                    all(
                        target_os = "windows",
                        any(target_arch = "x86_64", target_arch = "aarch64")
                    )
                )))]
                {
                    "unknown".to_string()
                }
            });

            let binary_name = if cfg!(windows) {
                format!("llama-helper-{}.exe", target_triple)
            } else {
                format!("llama-helper-{}", target_triple)
            };

            let bundled = resource_path.join(&binary_name);
            if bundled.exists() {
                log::info!("Found exact match in RESOURCE_DIR: {}", bundled.display());
                return Ok(bundled);
            }

            // Fuzzy match in RESOURCE_DIR
            if let Ok(entries) = std::fs::read_dir(&resource_path) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with("llama-helper") && !name.ends_with(".d") {
                            log::info!("Found fuzzy match in RESOURCE_DIR: {}", path.display());
                            return Ok(path);
                        }
                    }
                }
            }
        } else {
            log::warn!("RESOURCE_DIR environment variable not set");
        }

        // 3. Fallback for dev: try relative paths from workspace (no target triple in dev builds)
        if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
            let project_root = PathBuf::from(&manifest_dir)
                .parent()
                .and_then(|p| p.parent())
                .ok_or_else(|| anyhow!("Failed to determine project root"))?
                .to_path_buf();

            let candidates = vec![
                project_root.join("target/release/llama-helper"),
                project_root.join("target/debug/llama-helper"),
                project_root.join("target/release/llama-helper.exe"),
                project_root.join("target/debug/llama-helper.exe"),
            ];

            for candidate in candidates {
                if candidate.exists() {
                    log::info!("Using dev llama-helper: {}", candidate.display());
                    return Ok(candidate);
                }
            }
        }

        Err(anyhow!(
            "llama-helper binary not found. Build with 'cd llama-helper && cargo build --release' or set MEETILY_LLAMA_HELPER env var."
        ))
    }

    /// Ensure sidecar is running, spawn if needed
    pub async fn ensure_running(&self, model_path: PathBuf) -> Result<()> {
        // Check if already running with correct model
        {
            let current_model = self.current_model_path.read().await;
            if current_model.as_ref() == Some(&model_path) && self.is_healthy() {
                log::debug!("Sidecar already running with correct model");
                self.update_activity().await;
                return Ok(());
            }
        }

        // Need to spawn or restart
        self.spawn(model_path).await
    }

    pub(super) async fn lock_generation(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.generation_lock.clone().lock_owned().await
    }

    /// Spawn the sidecar process
    async fn spawn(&self, model_path: PathBuf) -> Result<()> {
        // Shutdown existing process if running
        self.shutdown().await?;

        let measurement_generation_id = measurement::current_generation_id();
        {
            let mut owner = self.measurement_generation_id.write().await;
            *owner = measurement_generation_id.clone();
        }
        if let Some(generation_id) = &measurement_generation_id {
            measurement::record_stage_start_for(generation_id, "load_model");
        }

        log::info!("Spawning llama-helper sidecar");
        log::info!("Model path: {}", model_path.display());

        #[cfg(windows)]
        let (child, stdin, stdout) = {
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            let spawned = windows_job::spawn_suspended_assigned_with_creation_flags(
                &self.helper_binary_path,
                &self.helper_arguments,
                BELOW_NORMAL_PRIORITY_CLASS,
            )
            .with_context(|| {
                format!(
                    "Failed to spawn llama-helper at {:?}",
                    self.helper_binary_path
                )
            })?;

            // The common Windows launcher intentionally pipes stderr so only the
            // three explicit child handles are inherited. Drain it continuously;
            // otherwise a noisy helper could fill the pipe and never exit.
            let job_process_ids = spawned
                .control
                .process_ids()
                .unwrap_or_else(|_| vec![spawned.control.process_id()]);
            if let Some(generation_id) = &measurement_generation_id {
                measurement::record_lifecycle_event_for(
                    generation_id,
                    "helper_process_start",
                    &job_process_ids,
                );
            }
            Self::start_stderr_drain(tokio::fs::File::from_std(spawned.stderr));
            (
                spawned.control,
                tokio::fs::File::from_std(spawned.stdin),
                tokio::fs::File::from_std(spawned.stdout),
            )
        };

        #[cfg(not(windows))]
        let (child, stdin, stdout) = {
            #[cfg(unix)]
            let mut command = tokio::process::Command::new("nice");

            #[cfg(not(unix))]
            let mut command = tokio::process::Command::new(&self.helper_binary_path);

            #[cfg(unix)]
            command.arg("-n").arg("10").arg(&self.helper_binary_path);

            command
                .args(&self.helper_arguments)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .env("LLAMA_IDLE_TIMEOUT", self.idle_timeout_secs.to_string())
                .kill_on_drop(true);

            let mut child = command.spawn().with_context(|| {
                format!(
                    "Failed to spawn llama-helper at {:?}",
                    self.helper_binary_path
                )
            })?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("Failed to get stdin"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("Failed to get stdout"))?;
            if let Some(generation_id) = &measurement_generation_id {
                let pids = child.id().into_iter().collect::<Vec<_>>();
                measurement::record_lifecycle_event_for(
                    generation_id,
                    "helper_process_start",
                    &pids,
                );
            }
            (child, stdin, stdout)
        };

        // Store handles
        {
            let mut child_lock = self.child_process.lock().await;
            *child_lock = Some(child);
        }

        {
            let mut stdin_lock = self.stdin_writer.lock().await;
            *stdin_lock = Some(stdin);
        }

        {
            let mut stdout_lock = self.stdout_reader.lock().await;
            *stdout_lock = Some(BufReader::new(stdout));
        }

        // Update state
        {
            let mut current_model = self.current_model_path.write().await;
            *current_model = Some(model_path);
        }

        self.is_healthy.store(true, Ordering::SeqCst);
        self.should_shutdown.store(false, Ordering::SeqCst);
        self.update_activity().await;

        let lifecycle_generation = self.lifecycle_generation.load(Ordering::SeqCst);

        log::info!("Sidecar spawned successfully");

        // Start background tasks
        self.start_health_check_loop(lifecycle_generation);
        self.start_idle_check_loop(lifecycle_generation);

        Ok(())
    }

    #[cfg(windows)]
    fn start_stderr_drain(stderr: tokio::fs::File) {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        #[cfg(test)]
                        eprintln!("LLAMA_HELPER_STDERR {line}");
                        #[cfg(not(test))]
                        log::debug!("llama-helper stderr: {}", line);
                    }
                    Ok(None) => break,
                    Err(error) => {
                        log::debug!("llama-helper stderr closed with error: {}", error);
                        break;
                    }
                }
            }
        });
    }

    /// Send a request to the sidecar and wait for response
    pub async fn send_request(&self, request_json: String, timeout: Duration) -> Result<String> {
        // Track active request
        let _guard = RequestGuard::new(self.active_request_count.clone());

        // Keep the complete request/response exchange atomic relative to pings.
        let _io_guard = self.request_io_lock.lock().await;

        // Write request to stdin
        {
            let mut stdin_lock = self.stdin_writer.lock().await;
            let stdin = stdin_lock
                .as_mut()
                .ok_or_else(|| anyhow!("Sidecar not running"))?;

            stdin
                .write_all(request_json.as_bytes())
                .await
                .context("Failed to write request to stdin")?;
            stdin
                .write_all(b"\n")
                .await
                .context("Failed to write newline")?;
            stdin.flush().await.context("Failed to flush stdin")?;
        }

        // Read response from stdout with timeout
        match tokio::time::timeout(timeout, self.read_response()).await {
            Ok(Ok(response)) => {
                self.update_activity().await;
                Ok(response)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Timeout reached - shutdown sidecar to stop generation
                log::error!("Request timeout after {:?}, shutting down sidecar", timeout);
                if let Err(shutdown_err) = self.shutdown().await {
                    log::error!("Failed to shutdown sidecar after timeout: {}", shutdown_err);
                }
                Err(anyhow!("Request timed out after {:?}", timeout))
            }
        }
    }

    /// Read a single line response from stdout
    async fn read_response(&self) -> Result<String> {
        let mut stdout_lock = self.stdout_reader.lock().await;
        let reader = stdout_lock
            .as_mut()
            .ok_or_else(|| anyhow!("Sidecar not running"))?;

        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .context("Failed to read response from stdout")?;

            if line.is_empty() {
                return Err(anyhow!("Sidecar closed stdout (process may have crashed)"));
            }

            let trimmed = line.trim();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if value.get("type").and_then(serde_json::Value::as_str) == Some("event") {
                    if value.get("event").and_then(serde_json::Value::as_str)
                        == Some("model_loaded")
                    {
                        if let Some(generation_id) =
                            self.measurement_generation_id.read().await.clone()
                        {
                            measurement::record_lifecycle_event_for(
                                &generation_id,
                                "model_loaded",
                                &[],
                            );
                            measurement::record_stage_end_for(&generation_id, "load_model");
                        }
                    }
                    continue;
                }
            }
            return Ok(trimmed.to_string());
        }
    }

    /// Send ping to keep sidecar alive
    async fn send_ping(&self) -> Result<()> {
        // A ping must never interleave with a generation request. Without this
        // guard, either reader can consume the other request's response.
        let _io_guard = self.request_io_lock.lock().await;
        let request = serde_json::json!({"type": "ping"}).to_string();
        let timeout = Duration::from_secs(5);

        // Note: We don't use send_request here to avoid incrementing active_request_count
        // for internal health checks, as that would prevent graceful shutdown

        // Write request
        {
            let mut stdin_lock = self.stdin_writer.lock().await;
            if let Some(stdin) = stdin_lock.as_mut() {
                stdin.write_all(request.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await?;
            } else {
                return Err(anyhow!("Sidecar not running"));
            }
        }

        // Read response
        let response = tokio::time::timeout(timeout, self.read_response()).await??;

        let resp: serde_json::Value = serde_json::from_str(&response)?;
        if resp.get("type").and_then(|t| t.as_str()) == Some("pong") {
            Ok(())
        } else {
            Err(anyhow!("Unexpected ping response: {}", response))
        }
    }

    /// Gracefully shutdown the sidecar
    /// Uses the same bounded, idempotent close as every other exit path.
    pub async fn shutdown_gracefully(&self) -> Result<()> {
        self.shutdown().await
    }

    /// Close the sidecar once. Concurrent and repeated calls are safe.
    ///
    /// The fixed sequence is: request shutdown, wait up to three seconds,
    /// terminate the whole Windows Job if anything remains, wait again, then
    /// confirm the process count is zero.
    pub async fn shutdown(&self) -> Result<()> {
        let _shutdown_guard = self.shutdown_lock.lock().await;
        let measurement_generation_id = self.measurement_generation_id.read().await.clone();
        let has_process = self.child_process.lock().await.is_some();
        if has_process {
            if let Some(generation_id) = &measurement_generation_id {
                measurement::record_lifecycle_event_for(generation_id, "cleanup_entry", &[]);
            }
        }

        // Set shutdown flag
        self.should_shutdown.store(true, Ordering::SeqCst);
        // Permanently invalidate health/idle tasks belonging to this process.
        // `spawn` may clear the shutdown flag for a replacement process before
        // an old interval wakes up, so the flag alone is not a sufficient guard.
        self.lifecycle_generation.fetch_add(1, Ordering::SeqCst);

        // Send shutdown command. A broken stdin is not a reason to skip process
        // cleanup; it commonly means the helper has already failed.
        if self.is_healthy() {
            if let Some(generation_id) = &measurement_generation_id {
                measurement::record_lifecycle_event_for(
                    generation_id,
                    "graceful_shutdown_request",
                    &[],
                );
            }
            let request = serde_json::json!({"type": "shutdown"}).to_string();
            let send_result = async {
                let mut stdin_lock = self.stdin_writer.lock().await;
                if let Some(stdin) = stdin_lock.as_mut() {
                    stdin.write_all(request.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = send_result {
                log::debug!("Could not send llama-helper shutdown request: {}", error);
            }
        }

        let process = {
            let mut child_lock = self.child_process.lock().await;
            child_lock.take()
        };

        #[cfg(windows)]
        let close_result = if let Some(control) = process {
            let close_generation_id = measurement_generation_id.clone();
            match tokio::task::spawn_blocking(move || {
                Self::close_windows_process(control, close_generation_id)
            })
            .await
            {
                Ok(result) => result,
                Err(error) => Err(anyhow!("llama-helper shutdown worker failed: {error}")),
            }
        } else {
            Ok(())
        };

        #[cfg(not(windows))]
        let close_result = Self::close_non_windows_process(process).await;

        // Clear handles
        {
            let mut stdin_lock = self.stdin_writer.lock().await;
            *stdin_lock = None;
        }

        {
            let mut stdout_lock = self.stdout_reader.lock().await;
            *stdout_lock = None;
        }

        {
            let mut current_model = self.current_model_path.write().await;
            *current_model = None;
        }

        self.is_healthy.store(false, Ordering::SeqCst);

        if let Some(generation_id) = &measurement_generation_id {
            measurement::record_stage_end_for(generation_id, "load_model");
        }
        {
            let mut owner = self.measurement_generation_id.write().await;
            *owner = None;
        }

        close_result?;
        log::info!("Sidecar shutdown complete with zero managed processes");
        Ok(())
    }

    #[cfg(windows)]
    fn close_windows_process(
        control: Arc<JobControl>,
        measurement_generation_id: Option<String>,
    ) -> Result<()> {
        let graceful_exit = match control.wait(Duration::from_secs(3)) {
            Ok(value) => value,
            Err(error) => {
                log::warn!("Could not wait for llama-helper shutdown: {}", error);
                false
            }
        };
        let remaining = match control.active_processes() {
            Ok(value) => Some(value),
            Err(error) => {
                log::warn!("Could not read llama-helper Job process count: {}", error);
                None
            }
        };

        if !graceful_exit || remaining != Some(0) {
            log::warn!(
                "llama-helper did not fully exit in three seconds (root_exited={}, remaining={:?}); terminating Job",
                graceful_exit,
                remaining
            );
            control
                .terminate()
                .context("terminating llama-helper Job")?;
            if !control
                .wait(Duration::from_secs(3))
                .context("waiting again after terminating llama-helper Job")?
            {
                return Err(anyhow!(
                    "llama-helper root process remained after Job termination"
                ));
            }
        }

        control
            .confirm_zero(Duration::from_secs(3))
            .context("confirming llama-helper Job process count is zero")?;
        if let Some(generation_id) = &measurement_generation_id {
            measurement::record_lifecycle_event_for(generation_id, "job_confirm_zero", &[]);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    async fn close_non_windows_process(process: Option<SidecarChild>) -> Result<()> {
        let Some(mut child) = process else {
            return Ok(());
        };

        match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
            Ok(Ok(status)) => {
                log::info!("Sidecar exited with status: {}", status);
                Ok(())
            }
            Ok(Err(error)) => Err(error).context("waiting for llama-helper shutdown"),
            Err(_) => {
                log::warn!("llama-helper did not exit in three seconds; killing it");
                child.kill().await.context("killing llama-helper")?;
                tokio::time::timeout(Duration::from_secs(3), child.wait())
                    .await
                    .context("waiting again after killing llama-helper")??;
                Ok(())
            }
        }
    }

    /// Check if sidecar is healthy
    pub fn is_healthy(&self) -> bool {
        self.is_healthy.load(Ordering::SeqCst)
    }

    /// Update last activity timestamp
    async fn update_activity(&self) {
        let mut last_activity = self.last_activity.write().await;
        *last_activity = Instant::now();
    }

    /// Get seconds since last activity
    async fn seconds_since_activity(&self) -> u64 {
        let last_activity = self.last_activity.read().await;
        last_activity.elapsed().as_secs()
    }

    /// Start health check loop (runs in background)
    fn start_health_check_loop(&self, expected_generation: usize) {
        let manager = Self {
            child_process: self.child_process.clone(),
            stdin_writer: self.stdin_writer.clone(),
            stdout_reader: self.stdout_reader.clone(),
            shutdown_lock: self.shutdown_lock.clone(),
            generation_lock: self.generation_lock.clone(),
            last_activity: self.last_activity.clone(),
            is_healthy: self.is_healthy.clone(),
            should_shutdown: self.should_shutdown.clone(),
            active_request_count: self.active_request_count.clone(),
            request_io_lock: self.request_io_lock.clone(),
            lifecycle_generation: self.lifecycle_generation.clone(),
            helper_binary_path: self.helper_binary_path.clone(),
            helper_arguments: self.helper_arguments.clone(),
            current_model_path: self.current_model_path.clone(),
            measurement_generation_id: self.measurement_generation_id.clone(),
            idle_timeout_secs: self.idle_timeout_secs,
            cleanup_on_drop: false,
        };

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Tokio intervals tick immediately once. Delay the first health ping
            // so it cannot race the generation request that follows `spawn`.
            interval.tick().await;

            loop {
                interval.tick().await;

                if manager.should_shutdown.load(Ordering::SeqCst)
                    || manager.lifecycle_generation.load(Ordering::SeqCst) != expected_generation
                {
                    log::debug!("Health check loop: lifecycle ended, exiting");
                    break;
                }

                if !manager.is_healthy() {
                    log::debug!("Health check loop: sidecar unhealthy, skipping ping");
                    continue;
                }

                // Don't ping if we are busy with a request
                if manager.active_request_count.load(Ordering::SeqCst) > 0 {
                    continue;
                }

                log::debug!("Health check: sending ping");
                if let Err(e) = manager.send_ping().await {
                    log::warn!("Health check failed: {}", e);
                    manager.is_healthy.store(false, Ordering::SeqCst);
                }
            }

            log::debug!("Health check loop exited");
        });
    }

    /// Start idle check loop (runs in background)
    fn start_idle_check_loop(&self, expected_generation: usize) {
        let manager = Self {
            child_process: self.child_process.clone(),
            stdin_writer: self.stdin_writer.clone(),
            stdout_reader: self.stdout_reader.clone(),
            shutdown_lock: self.shutdown_lock.clone(),
            generation_lock: self.generation_lock.clone(),
            last_activity: self.last_activity.clone(),
            is_healthy: self.is_healthy.clone(),
            should_shutdown: self.should_shutdown.clone(),
            active_request_count: self.active_request_count.clone(),
            request_io_lock: self.request_io_lock.clone(),
            lifecycle_generation: self.lifecycle_generation.clone(),
            helper_binary_path: self.helper_binary_path.clone(),
            helper_arguments: self.helper_arguments.clone(),
            current_model_path: self.current_model_path.clone(),
            measurement_generation_id: self.measurement_generation_id.clone(),
            idle_timeout_secs: self.idle_timeout_secs,
            cleanup_on_drop: false,
        };

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Match the configured interval instead of running the idle check
            // immediately at process startup.
            interval.tick().await;

            loop {
                interval.tick().await;

                if manager.should_shutdown.load(Ordering::SeqCst)
                    || manager.lifecycle_generation.load(Ordering::SeqCst) != expected_generation
                {
                    log::debug!("Idle check loop: lifecycle ended, exiting");
                    break;
                }

                // Don't shutdown if we are busy
                if manager.active_request_count.load(Ordering::SeqCst) > 0 {
                    // Update activity to prevent timeout immediately after request finishes
                    manager.update_activity().await;
                    continue;
                }

                let idle_secs = manager.seconds_since_activity().await;
                log::debug!("Idle check: {}s since last activity", idle_secs);

                if idle_secs > manager.idle_timeout_secs {
                    log::info!(
                        "Sidecar idle for {}s (timeout: {}s), shutting down",
                        idle_secs,
                        manager.idle_timeout_secs
                    );

                    if let Err(e) = manager.shutdown().await {
                        log::error!("Failed to shutdown idle sidecar: {}", e);
                    }

                    break;
                }
            }

            log::debug!("Idle check loop exited");
        });
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }

        // Emergency fallback only. Normal paths call shutdown() and verify zero.
        self.should_shutdown.store(true, Ordering::SeqCst);
        self.lifecycle_generation.fetch_add(1, Ordering::SeqCst);

        if let Ok(mut process) = self.child_process.try_lock() {
            if let Some(process) = process.take() {
                #[cfg(windows)]
                {
                    let _ = process.terminate();
                }
                #[cfg(not(windows))]
                {
                    let mut process = process;
                    let _ = process.start_kill();
                }
            }
        }

        log::debug!("SidecarManager dropped after emergency cleanup request");
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::summary::summary_engine::client::{parse_generation_response, request_with_cleanup};
    use std::process::{Command, Stdio};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    struct Fixture {
        manager: SidecarManager,
        pids: Vec<u32>,
    }

    struct ObservedProcess(HANDLE);

    impl ObservedProcess {
        fn open(process_id: u32) -> Self {
            const SYNCHRONIZE: u32 = 0x0010_0000;
            // SAFETY: this is a read/wait handle for an exact PID emitted by the fixture.
            let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, process_id) };
            assert!(
                !handle.is_null(),
                "fixture process {process_id} was not running"
            );
            Self(handle)
        }

        fn wait_for_exit(&self, timeout_ms: u32) -> bool {
            // SAFETY: the observed process handle remains owned by this value.
            unsafe { WaitForSingleObject(self.0, timeout_ms) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for ObservedProcess {
        fn drop(&mut self) {
            // SAFETY: this wrapper uniquely owns the observation handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn cmd_path() -> PathBuf {
        PathBuf::from(std::env::var_os("WINDIR").expect("WINDIR"))
            .join("System32")
            .join("cmd.exe")
    }

    fn fixture_script_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("scripts")
            .join("qa")
            .join("fixtures")
            .join("llama-helper-lifecycle-fixture.cmd")
    }

    fn helper_arguments(script: &std::path::Path, mode: &str) -> Vec<OsString> {
        vec![
            OsString::from("/d"),
            OsString::from("/q"),
            OsString::from("/v:off"),
            OsString::from("/c"),
            script.as_os_str().to_os_string(),
            OsString::from(mode),
        ]
    }

    async fn wait_for_job_pids(control: &JobControl, minimum: usize) -> Vec<u32> {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let values = control.process_ids().expect("query fixture Job PIDs");
            if values.len() >= minimum {
                return values;
            }
            assert!(
                Instant::now() < deadline,
                "fixture Job did not reach {minimum} processes"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn wait_for_pids(pid_file: &std::path::Path, minimum: usize) -> Vec<u32> {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Ok(contents) = std::fs::read_to_string(pid_file) {
                let values: Vec<u32> = contents
                    .lines()
                    .filter_map(|line| line.trim().parse::<u32>().ok())
                    .collect();
                if values.len() >= minimum {
                    return values;
                }
            }
            assert!(
                Instant::now() < deadline,
                "fixture did not publish {minimum} process IDs"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn start_fixture(mode: &str, minimum_pids: usize) -> Fixture {
        let script = fixture_script_path();
        let manager =
            SidecarManager::new_for_test(cmd_path(), helper_arguments(&script, mode), 300);
        manager
            .ensure_running(PathBuf::from("fixture-model.gguf"))
            .await
            .expect("start fixture helper");
        let control = job_control(&manager).await;
        let pids = wait_for_job_pids(&control, minimum_pids).await;
        eprintln!("LLAMA_LIFECYCLE_START mode={mode} pids={pids:?}");
        Fixture { manager, pids }
    }

    async fn job_control(manager: &SidecarManager) -> Arc<JobControl> {
        manager
            .child_process
            .lock()
            .await
            .as_ref()
            .expect("running Job")
            .clone()
    }

    fn generate_request() -> String {
        serde_json::json!({"type": "generate", "prompt": "fixture"}).to_string()
    }

    #[tokio::test]
    async fn normal_shutdown_waits_until_process_is_gone() {
        let fixture = start_fixture("delayed-shutdown", 1).await;
        let control = job_control(&fixture.manager).await;
        let started = Instant::now();
        fixture.manager.shutdown().await.expect("normal shutdown");
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert_eq!(control.active_processes().unwrap(), 0);
        eprintln!(
            "LLAMA_LIFECYCLE_DONE case=normal elapsed_ms={} active=0",
            started.elapsed().as_millis()
        );
    }

    #[tokio::test]
    async fn shutdown_timeout_kills_job_tree_and_waits_again() {
        let fixture = start_fixture("ignore-shutdown", 2).await;
        let pids = fixture.pids.clone();
        let observed: Vec<ObservedProcess> =
            pids.iter().copied().map(ObservedProcess::open).collect();
        let control = job_control(&fixture.manager).await;
        assert!(control.active_processes().unwrap() >= 2);
        let started = Instant::now();
        fixture
            .manager
            .shutdown()
            .await
            .expect("forced Job shutdown");
        assert!(started.elapsed() >= Duration::from_millis(2_800));
        assert_eq!(control.active_processes().unwrap(), 0);
        assert!(observed.iter().all(|process| process.wait_for_exit(1_000)));
        eprintln!(
            "LLAMA_LIFECYCLE_DONE case=timeout pids={pids:?} elapsed_ms={} active=0",
            started.elapsed().as_millis()
        );
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let fixture = start_fixture("success", 1).await;
        let control = job_control(&fixture.manager).await;
        fixture.manager.shutdown().await.expect("first shutdown");
        fixture.manager.shutdown().await.expect("second shutdown");
        assert_eq!(control.active_processes().unwrap(), 0);
        eprintln!("LLAMA_LIFECYCLE_DONE case=idempotent active=0");
    }

    #[tokio::test]
    async fn generation_success_releases_sidecar() {
        let fixture = start_fixture("success", 1).await;
        let control = job_control(&fixture.manager).await;
        let response = request_with_cleanup(
            &fixture.manager,
            generate_request(),
            Duration::from_secs(10),
            None,
        )
        .await
        .expect("successful request");
        assert_eq!(
            parse_generation_response(&response).unwrap(),
            "generated text"
        );
        assert!(!fixture.manager.is_healthy());
        assert_eq!(control.active_processes().unwrap(), 0);
        eprintln!("LLAMA_LIFECYCLE_DONE case=generation-success active=0");
    }

    #[tokio::test]
    async fn generation_failure_releases_sidecar() {
        let fixture = start_fixture("failure", 1).await;
        let control = job_control(&fixture.manager).await;
        let response = request_with_cleanup(
            &fixture.manager,
            generate_request(),
            Duration::from_secs(10),
            None,
        )
        .await
        .expect("protocol response");
        let error = parse_generation_response(&response).unwrap_err();
        assert!(error.to_string().contains("fixture model failure"));
        assert!(!fixture.manager.is_healthy());
        assert_eq!(control.active_processes().unwrap(), 0);
        eprintln!("LLAMA_LIFECYCLE_DONE case=generation-failure active=0");
    }

    #[tokio::test]
    #[ignore = "launched only by parent_force_exit_releases_helper_and_grandchild"]
    async fn force_exit_parent_fixture() {
        let pid_file =
            PathBuf::from(std::env::var_os("MEETILY_FORCE_EXIT_PID_FILE").expect("pid file"));
        let script = fixture_script_path();
        let manager =
            SidecarManager::new_for_test(cmd_path(), helper_arguments(&script, "parent-exit"), 300);
        manager
            .ensure_running(PathBuf::from("fixture-model.gguf"))
            .await
            .expect("start force-exit fixture");
        let control = job_control(&manager).await;
        let pids = wait_for_job_pids(&control, 2).await;
        std::fs::write(
            &pid_file,
            pids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("publish force-exit fixture PIDs");
        eprintln!("LLAMA_FORCE_EXIT_PARENT_READY pids={pids:?}");
        std::future::pending::<()>().await;
    }

    #[tokio::test]
    async fn parent_force_exit_releases_helper_and_grandchild() {
        let directory = tempfile::tempdir().expect("temporary fixture directory");
        let pid_file = directory.path().join("force-exit-process-ids.txt");

        let mut parent = Command::new(std::env::current_exe().expect("test executable"))
            .arg("force_exit_parent_fixture")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("MEETILY_FORCE_EXIT_PID_FILE", &pid_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn fixture parent");

        let pids = wait_for_pids(&pid_file, 2).await;
        let observed: Vec<ObservedProcess> =
            pids.iter().copied().map(ObservedProcess::open).collect();
        let parent_id = parent.id();
        parent.kill().expect("force terminate fixture parent");
        parent.wait().expect("reap fixture parent");
        assert!(observed.iter().all(|process| process.wait_for_exit(5_000)));
        eprintln!(
            "LLAMA_LIFECYCLE_DONE case=parent-force-exit parent_pid={parent_id} child_pids={pids:?} active=0"
        );
    }
}

#[cfg(all(test, not(windows)))]
mod non_windows_tests {
    use super::*;

    #[tokio::test]
    async fn non_windows_shutdown_without_a_process_is_idempotent() {
        let manager = SidecarManager::new_for_test(PathBuf::from("unused"), Vec::new(), 300);
        manager.shutdown().await.unwrap();
        manager.shutdown().await.unwrap();
    }
}
