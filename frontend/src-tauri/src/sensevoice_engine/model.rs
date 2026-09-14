//! Model catalogue and download for the SenseVoice-Small int8 model.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// One file that belongs to a model, with the URLs that may serve it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelFileSpec {
    pub file_name: &'static str,
    /// Mirror list, tried in order. The first entry is Hugging Face itself,
    /// the second is the community mirror that is reachable from mainland
    /// China without a proxy.
    pub urls: Vec<String>,
    /// Nominal size used for progress reporting before the server answers.
    pub expected_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SenseVoiceModelSpec {
    pub name: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub language: &'static str,
    pub files: Vec<ModelFileSpec>,
}

const HF_BASE: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main";
const MIRROR_BASE: &str = "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main";

pub const SENSEVOICE_MODELS: &[SenseVoiceModelSpec] = &[SenseVoiceModelSpec {
    name: "sensevoice-small-int8",
    display_name: "SenseVoice Small (int8, 中英粤日韩)",
    description: "本地中文优先转写模型，支持中英粤日韩",
    language: "zh",
    files: Vec::new(), // filled by `model_files()` so the Table can stay const
}];

/// Files for the SenseVoice-Small int8 model.
pub fn model_files() -> Vec<ModelFileSpec> {
    vec![
        ModelFileSpec {
            file_name: "model.int8.onnx",
            urls: vec![format!("{HF_BASE}/model.int8.onnx"), format!("{MIRROR_BASE}/model.int8.onnx")],
            expected_bytes: 239_233_841,
        },
        ModelFileSpec {
            file_name: "tokens.txt",
            urls: vec![format!("{HF_BASE}/tokens.txt"), format!("{MIRROR_BASE}/tokens.txt")],
            expected_bytes: 315_894,
        },
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
    Available,
    Missing,
    Partial,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub language: String,
    pub status: ModelStatus,
    pub size_bytes: u64,
    pub model_path: String,
}

pub fn model_directory(models_root: &Path, model_name: &str) -> PathBuf {
    models_root.join("sensevoice").join(model_name)
}

fn valid_file(dir: &Path, file: &ModelFileSpec) -> bool {
    std::fs::metadata(dir.join(file.file_name))
        .map(|meta| meta.is_file() && meta.len() == file.expected_bytes)
        .unwrap_or(false)
}

pub fn model_info(models_root: &Path, model_name: &str) -> Result<ModelInfo> {
    let spec = SENSEVOICE_MODELS
        .iter()
        .find(|spec| spec.name == model_name)
        .ok_or_else(|| anyhow!("Unknown SenseVoice model: {model_name}"))?;
    let dir = model_directory(models_root, model_name);
    let files = model_files();

    let present = files.iter().filter(|file| valid_file(&dir, file)).count();
    let status = if present == files.len() {
        ModelStatus::Available
    } else if !files.iter().any(|file| dir.join(file.file_name).exists() || dir.join(format!("{}.download", file.file_name)).exists()) {
        ModelStatus::Missing
    } else {
        ModelStatus::Partial
    };
    let size_bytes = files
        .iter()
        .map(|file| std::fs::metadata(dir.join(file.file_name)).map(|meta| meta.len()).unwrap_or(0))
        .sum();

    Ok(ModelInfo {
        name: spec.name.to_string(),
        display_name: spec.display_name.to_string(),
        description: spec.description.to_string(),
        language: spec.language.to_string(),
        status,
        size_bytes,
        model_path: dir.to_string_lossy().to_string(),
    })
}

pub fn discover_models(models_root: &Path) -> Result<Vec<ModelInfo>> {
    SENSEVOICE_MODELS
        .iter()
        .map(|spec| model_info(models_root, spec.name))
        .collect()
}

pub fn delete_model(models_root: &Path, model_name: &str) -> Result<()> {
    model_info(models_root, model_name)?; // Validate catalogue membership before deleting a path.
    let dir = model_directory(models_root, model_name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to delete {}", dir.display()))?;
    }
    Ok(())
}

/// Download every file of one model, emitting progress through `on_progress`.
///
/// Files are written to `<name>.download` and renamed on completion, so an
/// interrupted download can never be mistaken for a usable model.
pub async fn download_model<F>(models_root: &Path, model_name: &str, mut on_progress: F) -> Result<PathBuf>
where
    F: FnMut(u64, u64, &str),
{
    model_info(models_root, model_name)?; // Reject unknown model names before writing.
    let dir = model_directory(models_root, model_name);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("Failed to create {}", dir.display()))?;

    let files = model_files();
    let total_bytes: u64 = files.iter().map(|file| file.expected_bytes).sum();
    let mut done_bytes: u64 = 0;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60 * 60))
        .build()
        .context("Failed to build HTTP client")?;

    for file in &files {
        let final_path = dir.join(file.file_name);
        if valid_file(&dir, file) {
            let size = std::fs::metadata(&final_path).map(|meta| meta.len()).unwrap_or(0);
            done_bytes += size;
            on_progress(done_bytes, total_bytes, file.file_name);
            continue;
        }

        let temp_path = dir.join(format!("{}.download", file.file_name));
        let mut last_error: Option<String> = None;
        let mut downloaded_for_file = 0u64;

        for url in &file.urls {
            match download_one(&client, url, &temp_path, file.expected_bytes, |delta| {
                downloaded_for_file += delta;
                on_progress(done_bytes + downloaded_for_file, total_bytes, file.file_name);
            })
            .await
            {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(error) => {
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    downloaded_for_file = 0;
                    last_error = Some(format!("{url}: {error}"));
                }
            }
        }

        if let Some(error) = last_error {
            return Err(anyhow!(
                "Failed to download {} for model {model_name}: {error}",
                file.file_name
            ));
        }

        if final_path.is_file() { tokio::fs::remove_file(&final_path).await?; }
        tokio::fs::rename(&temp_path, &final_path)
            .await
            .with_context(|| format!("Failed to finalize {}", final_path.display()))?;
        done_bytes += downloaded_for_file;
    }

    Ok(dir)
}

async fn download_one<F>(client: &reqwest::Client, url: &str, target: &Path, expected_bytes: u64, mut on_delta: F) -> Result<()>
where
    F: FnMut(u64),
{
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("HTTP request failed for {url}"))?;
    if !response.status().is_success() {
        return Err(anyhow!("HTTP {} from {url}", response.status()));
    }

    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(target)
        .await
        .with_context(|| format!("Failed to create {}", target.display()))?;
    let mut bytes = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("Stream error while reading {url}"))?;
        file.write_all(&chunk)
            .await
            .with_context(|| format!("Write error for {}", target.display()))?;
        bytes += chunk.len() as u64;
        if bytes > expected_bytes { return Err(anyhow!("Unexpected file size")); }
        on_delta(chunk.len() as u64);
    }
    if bytes != expected_bytes { return Err(anyhow!("Incomplete model file: {bytes}/{expected_bytes}")); }
    file.flush().await?;
    file.sync_all().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_partial_and_zero_length_files_are_not_available() {
        let temp = tempfile::tempdir().unwrap();
        let name = SENSEVOICE_MODELS[0].name;
        assert_eq!(model_info(temp.path(), name).unwrap().status, ModelStatus::Missing);
        let dir = model_directory(temp.path(), name);
        std::fs::create_dir_all(&dir).unwrap();
        for file in model_files() { std::fs::File::create(dir.join(file.file_name)).unwrap(); }
        assert_eq!(model_info(temp.path(), name).unwrap().status, ModelStatus::Partial);
        for file in model_files() { std::fs::File::create(dir.join(file.file_name)).unwrap().set_len(file.expected_bytes).unwrap(); }
        assert_eq!(model_info(temp.path(), name).unwrap().status, ModelStatus::Available);
        // Size checks cover incomplete transfer, not same-length corruption or engine loading.
        assert!(delete_model(temp.path(), "../outside").is_err());
    }
    #[tokio::test]
    async fn rejects_truncated_http_success() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = server.accept().await.unwrap();
            let mut request = [0u8; 1024]; stream.read(&mut request).await.unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad").await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let result = download_one(&reqwest::Client::new(), &format!("http://{address}"), &temp.path().join("model.download"), 20, |_| {}).await;
        assert!(result.is_err()); task.await.unwrap();
    }
}
