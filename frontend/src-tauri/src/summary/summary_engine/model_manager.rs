// Model manager for built-in AI models - handles downloads and lifecycle
// Follows the same pattern as whisper_engine/whisper_engine.rs for consistency

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tokio::time::timeout;

use super::models::{get_available_models, get_model_by_name, ModelDef};

#[cfg(test)]
#[path = "download_tests.rs"]
mod download_tests;

fn check_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() { Err(anyhow!("CANCELLED: 下载已取消")) } else { Ok(()) }
}

// Only verified, byte-identical mirrors may share a resumable partial file.
fn summary_sources(model: &ModelDef) -> (Vec<String>, Option<&'static str>) {
    let hash = match model.name.as_str() {
        "qwen3.5:4b" => Some("00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4"),
        "qwen3.5:2b" => Some("aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223"),
        _ => None,
    };
    let mut urls = vec![model.download_url.clone()];
    if hash.is_some() {
        urls.push(model.download_url.replace("https://huggingface.co/", "https://www.modelscope.cn/models/")
            .replace("/resolve/main/", "/resolve/master/"));
    }
    (urls, hash)
}

fn resume_total(range: &str, offset: u64) -> Result<u64> {
    let parsed = (|| {
        let (span, total) = range.strip_prefix("bytes ")?.split_once('/')?;
        let (start, end) = span.split_once('-')?;
        Some((start.parse::<u64>().ok()?, end.parse::<u64>().ok()?, total.parse::<u64>().ok()?))
    })();
    match parsed {
        Some((start, end, total)) if start == offset && end >= start && end.checked_add(1) == Some(total) => Ok(total),
        _ => Err(anyhow!("下载源返回了不匹配的断点范围，已拒绝追加文件。")),
    }
}

// ============================================================================
// Model Status Types
// ============================================================================

/// Detailed download progress info (MB-based with speed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    /// Bytes downloaded so far
    pub downloaded_bytes: u64,
    /// Total file size in bytes
    pub total_bytes: u64,
    /// Downloaded in MB (for display)
    pub downloaded_mb: f64,
    /// Total size in MB (for display)
    pub total_mb: f64,
    /// Download speed in MB/s
    pub speed_mbps: f64,
    /// Percentage complete (0-100)
    pub percent: u8,
}

impl DownloadProgress {
    pub fn new(downloaded: u64, total: u64, speed_mbps: f64) -> Self {
        let percent = if total > 0 {
            ((downloaded as f64 / total as f64) * 100.0) as u8
        } else {
            0
        };
        Self {
            downloaded_bytes: downloaded,
            total_bytes: total,
            downloaded_mb: downloaded as f64 / (1024.0 * 1024.0),
            total_mb: total as f64 / (1024.0 * 1024.0),
            speed_mbps,
            percent,
        }
    }
}

/// Model status in the system
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelStatus {
    /// Model is not yet downloaded
    NotDownloaded,

    /// Model is currently being downloaded (progress 0-100)
    Downloading { progress: u8 },

    /// Model is downloaded and ready to use
    Available,

    /// Model file is corrupted and needs redownload
    Corrupted { file_size: u64, expected_min_size: u64 },

    /// Error occurred with the model
    Error(String),
}

/// Model information for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model name (e.g., "gemma3:1b")
    pub name: String,

    /// Display name for UI
    pub display_name: String,

    /// Current status
    pub status: ModelStatus,

    /// File path (if available)
    pub path: PathBuf,

    /// Size in MB
    pub size_mb: u64,

    /// Context window size in tokens
    pub context_size: u32,

    /// Description
    pub description: String,

    /// GGUF filename on disk
    pub gguf_file: String,
}

// ============================================================================
// Model Manager
// ============================================================================

pub struct ModelManager {
    /// Directory where models are stored
    models_dir: PathBuf,

    /// Currently available models with their status
    available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,

    /// Active downloads (model names)
    active_downloads: Arc<RwLock<HashMap<String, CancellationToken>>>,
}

impl ModelManager {
    /// Create a new model manager with default models directory
    pub fn new() -> Result<Self> {
        Self::new_with_models_dir(None)
    }

    /// Create a new model manager with custom models directory
    pub fn new_with_models_dir(models_dir: Option<PathBuf>) -> Result<Self> {
        let models_dir = if let Some(dir) = models_dir {
            dir
        } else {
            // Fallback: Use current directory in development
            let current_dir = std::env::current_dir()
                .map_err(|e| anyhow!("Failed to get current directory: {}", e))?;

            if cfg!(debug_assertions) {
                // Development mode
                current_dir.join("models").join("summary")
            } else {
                // Production mode fallback (caller should provide path)
                log::warn!("ModelManager: No models directory provided, using fallback path");
                dirs::data_dir()
                    .or_else(|| dirs::home_dir())
                    .ok_or_else(|| anyhow!("Could not find system data directory"))?
                    .join("Meetily")
                    .join("models")
                    .join("summary")
            }
        };

        log::info!(
            "Built-in AI ModelManager using directory: {}",
            models_dir.display()
        );

        Ok(Self {
            models_dir,
            available_models: Arc::new(RwLock::new(HashMap::new())),
            active_downloads: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Initialize and scan for existing models
    pub async fn init(&self) -> Result<()> {
        // Create models directory if it doesn't exist
        if !self.models_dir.exists() {
            fs::create_dir_all(&self.models_dir).await?;
            log::info!("Created models directory: {}", self.models_dir.display());
        }

        // Scan for existing models
        self.scan_models().await?;

        Ok(())
    }

    /// Scan models directory and update status
    pub async fn scan_models(&self) -> Result<()> {
        let start = std::time::Instant::now();

        log::info!(
            "Starting model scan in directory: {}",
            self.models_dir.display()
        );

        let model_defs = get_available_models();
        let mut models_map = HashMap::new();

        for model_def in model_defs {
            let model_path = self.models_dir.join(&model_def.gguf_file);
            log::debug!(
                "Checking model '{}' at path: {}",
                model_def.name,
                model_path.display()
            );

            let is_actively_downloading = {
                let active = self.active_downloads.read().await;
                active.contains_key(&model_def.name)
            };

            // If actively downloading, preserve existing status from memory
            if is_actively_downloading {
                let existing_info = {
                    let models = self.available_models.read().await;
                    models.get(&model_def.name).cloned()
                };

                if let Some(info) = existing_info {
                    // Preserve existing status (should be Downloading)
                    models_map.insert(model_def.name.clone(), info);
                    log::debug!(
                        "Model '{}': Preserving Downloading status during scan",
                        model_def.name
                    );
                    continue;
                }
            }

            let status = if model_path.exists() {
                // Check if file size matches expected size (basic validation)
                match fs::metadata(&model_path).await {
                    Ok(metadata) => {
                        let file_size_mb = metadata.len() / (1024 * 1024);

                        // Allow 10% variance for file size check
                        let expected_min = (model_def.size_mb as f64 * 0.9) as u64;
                        let expected_max = (model_def.size_mb as f64 * 1.1) as u64;

                        log::info!(
                            "Model '{}': found {} MB (expected {}-{} MB)",
                            model_def.name,
                            file_size_mb,
                            expected_min,
                            expected_max
                        );

                        if file_size_mb >= expected_min && file_size_mb <= expected_max {
                            log::info!("Model '{}': AVAILABLE", model_def.name);
                            ModelStatus::Available
                        } else {
                            log::warn!(
                                "Model '{}': CORRUPTED (size mismatch: {} MB, expected {} MB)",
                                model_def.name,
                                file_size_mb,
                                model_def.size_mb
                            );
                            ModelStatus::Corrupted {
                                file_size: file_size_mb,
                                expected_min_size: expected_min,
                            }
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Model '{}': Failed to read metadata: {}",
                            model_def.name,
                            e
                        );
                        ModelStatus::Error(format!("Failed to read metadata: {}", e))
                    }
                }
            } else {
                log::debug!("Model '{}': NOT FOUND", model_def.name);
                ModelStatus::NotDownloaded
            };

            let model_info = ModelInfo {
                name: model_def.name.clone(),
                display_name: model_def.display_name.clone(),
                status,
                path: model_path,
                size_mb: model_def.size_mb,
                context_size: model_def.context_size,
                description: model_def.description.clone(),
                gguf_file: model_def.gguf_file.clone(),
            };

            models_map.insert(model_def.name.clone(), model_info);
        }

        let model_count = models_map.len();

        let mut models = self.available_models.write().await;
        *models = models_map;

        let elapsed = start.elapsed();
        log::info!(
            "Model scan complete: {} models checked in {:?}",
            model_count,
            elapsed
        );
        Ok(())
    }

    /// Get list of all models with their status
    pub async fn list_models(&self) -> Vec<ModelInfo> {
        self.available_models
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Get info for a specific model
    pub async fn get_model_info(&self, model_name: &str) -> Option<ModelInfo> {
        self.available_models
            .read()
            .await
            .get(model_name)
            .cloned()
    }

    /// Check if a model is ready to use
    /// If refresh=true, scans filesystem before checking (slower but accurate)
    pub async fn is_model_ready(&self, model_name: &str, refresh: bool) -> bool {
        if refresh {
            if let Err(e) = self.scan_models().await {
                log::error!("Failed to scan models: {}", e);
                return false;
            }
        }

        if let Some(info) = self.get_model_info(model_name).await {
            info.status == ModelStatus::Available
        } else {
            false
        }
    }

    /// Download a model with simple percentage callback (backward compatible)
    pub async fn download_model(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        // Wrap the simple callback to use detailed progress internally
        let detailed_callback: Option<Box<dyn Fn(DownloadProgress) + Send>> =
            progress_callback.map(|cb| {
                Box::new(move |p: DownloadProgress| cb(p.percent)) as Box<dyn Fn(DownloadProgress) + Send>
            });
        self.download_model_detailed(model_name, detailed_callback).await
    }

    /// Every exit path releases ownership, including connect, disk and validation errors.
    pub async fn download_model_detailed(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let model = get_model_by_name(model_name)
            .ok_or_else(|| anyhow!("Unknown model: {}", model_name))?;
        let (urls, checksum) = summary_sources(&model);
        self.download_from_sources(&model, &urls, checksum, progress_callback).await
    }

    async fn download_from_sources(
        &self,
        model: &ModelDef,
        urls: &[String],
        checksum: Option<&str>,
        mut callback: Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let cancel = CancellationToken::new();
        {
            let mut active = self.active_downloads.write().await;
            if active.contains_key(&model.name) {
                return Err(anyhow!("模型正在下载或正在取消，请稍候再试。"));
            }
            active.insert(model.name.clone(), cancel.clone());
        }
        self.set_download_status(&model.name, ModelStatus::Downloading { progress: 0 }).await;
        let result = self.download_inner(model, urls, checksum, &cancel, &mut callback).await;
        let status = match &result {
            Ok(()) => ModelStatus::Available,
            Err(_) if cancel.is_cancelled() => ModelStatus::NotDownloaded,
            Err(e) => ModelStatus::Error(e.to_string()),
        };
        // Publish terminal state before allowing a new owner to start.
        let mut active = self.active_downloads.write().await;
        self.set_download_status(&model.name, status).await;
        active.remove(&model.name);
        drop(active);
        if result.is_ok() {
            if let Some(cb) = callback {
                let size = fs::metadata(self.models_dir.join(&model.gguf_file)).await?.len();
                cb(DownloadProgress::new(size, size, 0.0));
            }
        }
        result
    }

    async fn set_download_status(&self, name: &str, status: ModelStatus) {
        if let Some(info) = self.available_models.write().await.get_mut(name) {
            info.status = status;
        }
    }

    async fn validate_download(
        &self, path: &PathBuf, model: &ModelDef, checksum: Option<&str>, cancel: &CancellationToken,
    ) -> Result<()> {
        check_cancelled(cancel)?;
        let size = fs::metadata(path).await?.len();
        let expected = model.size_mb * 1024 * 1024;
        if size < expected * 9 / 10 || size > expected * 11 / 10 {
            return Err(anyhow!("模型文件大小不正确：{} 字节", size));
        }
        self.validate_gguf_file(path).await?;
        if let Some(expected_hash) = checksum {
            let mut file = fs::File::open(path).await?;
            let mut buf = vec![0; 1024 * 1024];
            let mut digest = Sha256::new();
            loop {
                check_cancelled(cancel)?;
                let len = file.read(&mut buf).await?;
                if len == 0 { break; }
                digest.update(&buf[..len]);
            }
            if format!("{:x}", digest.finalize()) != expected_hash {
                return Err(anyhow!("模型 SHA-256 校验失败，文件不完整或与官方版本不一致。"));
            }
        }
        check_cancelled(cancel)
    }

    async fn download_inner(
        &self, model: &ModelDef, urls: &[String], checksum: Option<&str>,
        cancel: &CancellationToken, callback: &mut Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        fs::create_dir_all(&self.models_dir).await?;
        let final_path = self.models_dir.join(&model.gguf_file);
        if self.validate_download(&final_path, model, checksum, cancel).await.is_ok() {
            log::info!("Summary model {} already verified; skipping download", model.name);
            return Ok(());
        }
        check_cancelled(cancel)?;
        let part = final_path.with_extension("gguf.part");
        let client = Client::builder()
            .user_agent(concat!("Meetily/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        let mut errors = Vec::new();
        for (index, url) in urls.iter().enumerate() {
            check_cancelled(cancel)?;
            log::info!("Summary {} download source {}: {}", model.name, index + 1, url);
            // A complete .part may remain after cancellation during verification.
            if checksum.is_some() && self.validate_download(&part, model, checksum, cancel).await.is_ok() {
                fs::rename(&part, &final_path).await?;
                return Ok(());
            }
            let result = self.download_source(model, url, &part, &client, cancel, callback).await;
            check_cancelled(cancel)?;
            match result {
                Ok(()) => match self.validate_download(&part, model, checksum, cancel).await {
                    Ok(()) => {
                        check_cancelled(cancel)?;
                        fs::rename(&part, &final_path).await?;
                        return Ok(());
                    }
                    Err(e) => {
                        check_cancelled(cancel)?;
                        // Never resume a completed but mismatched file.
                        fs::remove_file(&part).await?;
                        errors.push(format!("下载源 {}：{}", index + 1, e));
                    }
                },
                Err(e) => errors.push(format!("下载源 {}：{}", index + 1, e)),
            }
        }
        Err(anyhow!("纪要模型下载失败，可重试或使用已下载模型。{}", errors.join("；")))
    }

    async fn download_source(
        &self, model: &ModelDef, url: &str, part: &PathBuf, client: &Client,
        cancel: &CancellationToken, callback: &mut Option<Box<dyn Fn(DownloadProgress) + Send>>,
    ) -> Result<()> {
        let offset = fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        let mut request = client.get(url).header("Accept-Encoding", "identity");
        if offset > 0 { request = request.header("Range", format!("bytes={}-", offset)); }
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(anyhow!("CANCELLED: 下载已取消")),
            result = timeout(Duration::from_secs(30), request.send()) => result
                .map_err(|_| anyhow!("连接下载源超时"))?
                .map_err(|e| anyhow!("无法连接下载源：{}", e))?,
        };
        let content_length = response.content_length();
        let partial_response = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let (total, resume) = match response.status() {
            reqwest::StatusCode::OK => (response.content_length().unwrap_or(model.size_mb * 1024 * 1024), false),
            reqwest::StatusCode::PARTIAL_CONTENT => {
                let range = response.headers().get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok()).unwrap_or("");
                (resume_total(range, offset)?, offset > 0)
            }
            reqwest::StatusCode::RANGE_NOT_SATISFIABLE => {
                if part.exists() { fs::remove_file(part).await?; }
                return Err(anyhow!("下载断点无效，已清除临时断点，请重试。"));
            }
            status => return Err(anyhow!("下载源返回 HTTP {}", status)),
        };
        check_cancelled(cancel)?;
        let file = OpenOptions::new().create(true).write(true).append(resume).truncate(!resume)
            .open(part).await?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        let mut downloaded = if resume { offset } else { 0 };
        let mut reported = downloaded;
        let mut report_at = std::time::Instant::now();
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let transfer: Result<()> = async {
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(anyhow!("CANCELLED: 下载已取消")),
                    next = timeout(Duration::from_secs(60), stream.next()) =>
                        next.map_err(|_| anyhow!("下载连续 60 秒未收到数据"))?,
                };
                let Some(chunk) = chunk else { break; };
                let chunk = chunk.map_err(|e| anyhow!("下载中断：{}", e))?;
                writer.write_all(&chunk).await?;
                downloaded += chunk.len() as u64;
                if downloaded > model.size_mb * 1024 * 1024 * 11 / 10 {
                    return Err(anyhow!("下载内容超过预期模型大小"));
                }
                if report_at.elapsed().as_millis() >= 500 {
                    let speed = (downloaded - reported) as f64 / 1048576.0 / report_at.elapsed().as_secs_f64();
                    let mut progress = DownloadProgress::new(downloaded, total, speed);
                    progress.percent = progress.percent.min(99);
                    self.set_download_status(&model.name, ModelStatus::Downloading { progress: progress.percent }).await;
                    if let Some(cb) = callback.as_mut() { cb(progress); }
                    reported = downloaded;
                    report_at = std::time::Instant::now();
                }
            }
            Ok(())
        }.await;
        // Preserve an exact prefix for a retry, including after cancellation or disconnect.
        let flushed = writer.flush().await;
        drop(writer);
        transfer?;
        flushed?;
        let start = if resume { offset } else { 0 };
        if content_length.is_some_and(|len| downloaded - start != len)
            || (partial_response && downloaded != total) {
            return Err(anyhow!("下载文件不完整，请重试以继续下载。"));
        }
        check_cancelled(cancel)
    }

    /// Validate that a file is a valid GGUF model
    async fn validate_gguf_file(&self, path: &PathBuf) -> Result<()> {
        let mut file = fs::File::open(path).await?;

        // Read first 4 bytes to check for GGUF magic number
        use tokio::io::AsyncReadExt;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic).await?;

        // GGUF magic number is "GGUF" (0x47475546)
        if &magic == b"GGUF" {
            Ok(())
        } else if &magic == b"ggjt" || &magic == b"ggla" || &magic == b"ggml" {
            // Older formats (GGML, GGJT)
            Ok(())
        } else {
            Err(anyhow!(
                "Invalid model file: magic number {:?} doesn't match GGUF/GGML",
                magic
            ))
        }
    }

    /// Cancel an ongoing download
    pub async fn cancel_download(&self, model_name: &str) -> Result<()> {
        log::info!("Cancelling download for model: {}", model_name);

        if let Some(cancel) = self.active_downloads.read().await.get(model_name) {
            cancel.cancel();
        }
        // Keep ownership and Downloading state until the task has closed its file.
        Ok(())
    }

    /// Delete a corrupted or available model file
    pub async fn delete_model(&self, model_name: &str) -> Result<()> {
        log::info!("Deleting model: {}", model_name);

        let model_def = get_model_by_name(model_name)
            .ok_or_else(|| anyhow!("Unknown model: {}", model_name))?;

        let active = self.active_downloads.read().await;
        if active.contains_key(model_name) {
            return Err(anyhow!("请先取消下载，等待停止后再删除模型。"));
        }
        let file_path = self.models_dir.join(&model_def.gguf_file);

        if file_path.exists() {
            fs::remove_file(&file_path).await?;
            log::info!("Deleted model file: {}", file_path.display());
        }

        // Update status
        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::NotDownloaded;
            }
        }

        Ok(())
    }

    /// Get models directory path
    pub fn get_models_directory(&self) -> PathBuf {
        self.models_dir.clone()
    }
}
