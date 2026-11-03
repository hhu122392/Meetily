//! Process boundary for native Whisper inference. No database, capture, or Lark access.
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command, sync::Mutex};

const WORKER_ARG: &str = "--meetily-whisper-worker";
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
static INFERENCE_SLOT: Mutex<()> = Mutex::const_new(());

#[cfg(target_os = "windows")]
struct WorkerJob(isize);

#[cfg(target_os = "windows")]
impl WorkerJob {
    fn attach(child: &tokio::process::Child) -> Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(handle as isize);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let process = child
                .raw_handle()
                .ok_or_else(|| anyhow!("转写进程已经退出"))?;
            if AssignProcessToJobObject(handle, process as _) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(job)
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for WorkerJob {
    fn drop(&mut self) {
        // Windows also closes this non-inheritable handle if the host itself crashes.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Request {
    models_dir: PathBuf,
    model: String,
    audio: Vec<f32>,
    language: Option<String>,
    with_confidence: bool,
}

#[derive(Serialize, Deserialize)]
struct Response {
    result: Option<(String, f32, bool)>,
    error: Option<String>,
}

/// Called before Tauri initialization, including the single-instance plugin.
pub fn run_if_requested() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(WORKER_ARG)) {
        return false;
    }
    #[cfg(target_os = "windows")]
    unsafe {
        // Report native exceptions as process exit codes; never block on a crash dialog.
        #[link(name = "kernel32")]
        extern "system" {
            fn SetErrorMode(mode: u32) -> u32;
        }
        SetErrorMode(0x0001 | 0x0002);
    }
    let result = run_request();
    let response = match result {
        Ok(result) => Response {
            result: Some(result),
            error: None,
        },
        Err(error) => Response {
            result: None,
            error: Some(format!("{error:#}")),
        },
    };
    if let Ok(bytes) = serde_json::to_vec(&response) {
        let _ = std::io::stdout().lock().write_all(&bytes);
    }
    true
}

fn run_request() -> Result<(String, f32, bool)> {
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    if input.len() > MAX_REQUEST_BYTES {
        return Err(anyhow!("转写音频片段过大，请拆分后重试"));
    }
    let request: Request = serde_json::from_slice(&input).context("无效的转写任务")?;
    if request.audio.is_empty() || request.audio.iter().any(|v| !v.is_finite()) {
        return Err(anyhow!("转写音频为空或包含无效采样"));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let engine = super::WhisperEngine::new_native_worker(request.models_dir)?;
        engine.discover_models().await?;
        engine.load_model(&request.model).await?;
        if request.with_confidence {
            engine
                .transcribe_audio_with_confidence(request.audio, request.language)
                .await
        } else {
            let partial = request.audio.len() < 15 * 16000;
            let text = engine
                .transcribe_audio(request.audio, request.language)
                .await?;
            Ok((text, 0.0, partial))
        }
    })
}

pub(super) async fn transcribe(
    models_dir: PathBuf,
    model: String,
    audio: Vec<f32>,
    language: Option<String>,
    with_confidence: bool,
) -> Result<(String, f32, bool)> {
    let _slot = INFERENCE_SLOT.lock().await;
    let request = serde_json::to_vec(&Request {
        models_dir,
        model,
        audio,
        language,
        with_confidence,
    })?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err(anyhow!("转写音频片段过大，请拆分后重试"));
    }
    #[allow(unused_mut)]
    let mut executable = std::env::current_exe()?;
    #[cfg(test)]
    if let Some(path) = std::env::var_os("TEST_WHISPER_WORKER_EXE") {
        executable = path.into();
    }
    let mut command = Command::new(executable);
    command.arg(WORKER_ARG);
    exchange(command, request, INFERENCE_TIMEOUT).await
}

async fn exchange(
    mut command: Command,
    request: Vec<u8>,
    timeout: Duration,
) -> Result<(String, f32, bool)> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = command.spawn().context("无法启动独立转写进程")?;
    #[cfg(target_os = "windows")]
    let _job = match WorkerJob::attach(&child) {
        Ok(job) => Some(job),
        Err(_) if child.try_wait()?.is_some() => None,
        Err(error) => return Err(error.context("无法设置转写进程退出清理")),
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("无法连接转写进程"))?;
    let output = tokio::time::timeout(timeout, async move {
        // Join the writer and reader to drain native stderr even during input upload.
        let writer = async move {
            stdin.write_all(&request).await?;
            stdin.shutdown().await
        };
        let (written, output) = tokio::join!(writer, child.wait_with_output());
        let output = output?;
        if !output.status.success() {
            return Err(anyhow!(
                "转写引擎异常退出（{}）；原音频已保留，录音不会因此停止。请重试或更换模型。",
                output.status
            ));
        }
        written.context("向转写进程传输音频失败")?;
        Ok(output)
    })
    .await
    .map_err(|_| {
        anyhow!("转写引擎处理超时，已结束该进程；原音频已保留，可重试或更换较小模型。")
    })??;
    let response: Response =
        serde_json::from_slice(&output.stdout).context("转写进程返回了无效结果，原音频已保留")?;
    response.result.ok_or_else(|| {
        anyhow!(
            "转写未完成：{}",
            response.error.unwrap_or_else(|| "未知错误".into())
        )
    })
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn child_failure_is_an_error_not_a_host_crash() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "exit -1073740940"]); // STATUS_HEAP_CORRUPTION
        let error = exchange(command, Vec::new(), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("异常退出"), "{error}");
        // The runtime remains usable after the native worker failed.
        assert_eq!(tokio::spawn(async { 42 }).await.unwrap(), 42);
    }

    #[tokio::test]
    async fn child_timeout_returns_without_hanging_the_host() {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
        let start = std::time::Instant::now();
        let error = exchange(command, Vec::new(), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("超时"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn invalid_worker_output_is_not_accepted_as_a_transcript() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "echo invalid-json"]);
        let error = exchange(command, Vec::new(), Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("无效结果"), "{error}");
    }

    #[tokio::test]
    async fn closing_worker_job_terminates_inference_process() {
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
            .creation_flags(0x08000000)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let job = WorkerJob::attach(&child).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        drop(job);
        // Closing a Windows job may terminate members with exit code zero.
        // Completion within 3s (rather than the 30s sleep) proves cleanup.
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
    }
}
