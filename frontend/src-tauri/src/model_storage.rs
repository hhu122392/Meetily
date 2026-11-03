use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Emitter, Manager};

const CONFIG: &str = "model-storage.json";
pub(crate) static MODEL_FILES: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

pub(crate) async fn use_model_files() -> Result<tokio::sync::RwLockReadGuard<'static, ()>, String> {
    MODEL_FILES
        .try_read()
        .map_err(|_| "正在迁移模型，请稍后重试。".into())
}

#[derive(Clone, Serialize, Deserialize)]
struct CopiedFile {
    source: PathBuf,
    destination: PathBuf,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
struct Configuration {
    directory: PathBuf,
    #[serde(default)]
    cleanup: Vec<CopiedFile>,
}

fn read_config(root: &Path) -> Result<Option<Configuration>, String> {
    let path = root.join(CONFIG);
    if !path.exists() {
        return Ok(None);
    }
    let config: Configuration = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "模型目录配置损坏，请恢复配置后重启；未切换到默认目录。")?;
    if !config.directory.is_absolute() {
        return Err("模型目录必须是绝对路径。".into());
    }
    Ok(Some(config))
}

pub fn configured_directory(root: &Path) -> Option<PathBuf> {
    // Startup validates the configuration before any engine is initialized.
    read_config(root).ok().flatten().map(|c| c.directory)
}

pub fn models_directory(root: &Path) -> PathBuf {
    configured_directory(root).unwrap_or_else(|| root.join("models"))
}

fn persist(root: &Path, config: &Configuration) -> Result<(), String> {
    let mut temp = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    temp.write_all(&serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(root.join(CONFIG)).map_err(|e| e.to_string())?;
    Ok(())
}

fn hash(path: &Path) -> Result<String, String> {
    let mut input = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 1024 * 128];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

fn collect(source: &Path, destination: &Path, files: &mut Vec<CopiedFile>) -> Result<(), String> {
    let meta = fs::symlink_metadata(source).map_err(|e| e.to_string())?;
    if is_link(&meta) {
        return Err(format!("模型子目录包含链接，未迁移：{}", source.display()));
    }
    if meta.is_dir() {
        for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            collect(&entry.path(), &destination.join(entry.file_name()), files)?;
        }
    } else if meta.is_file() {
        files.push(CopiedFile {
            source: source.into(),
            destination: destination.into(),
            sha256: String::new(),
        });
    } else {
        return Err("模型目录含不支持的文件类型。".into());
    }
    Ok(())
}

fn migration_plan(source: &Path, summary: &Path, target: &Path) -> Result<Vec<CopiedFile>, String> {
    if !target.is_absolute() || target.parent().is_none() {
        return Err("请选择有效的模型文件夹。".into());
    }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(target).map_err(|e| e.to_string())?;
    if fs::read_dir(&target)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("请选择空文件夹，避免覆盖已有模型。".into());
    }
    let mut files = Vec::new();
    for (root, is_summary) in [(source, false), (summary, true)] {
        if !root.exists() {
            continue;
        }
        // Explicit model roots may be junctions from an earlier manual migration.
        let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
        if root.starts_with(&target) || target.starts_with(&root) {
            return Err("新模型目录不能与原目录相同，或互为父子目录。".into());
        }
        if is_summary {
            collect(&root, &target.join("summary"), &mut files)?;
        } else {
            for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_name() == "summary" {
                    continue;
                }
                collect(&entry.path(), &target.join(entry.file_name()), &mut files)?;
            }
        }
    }
    Ok(files)
}

fn copy_and_commit(
    root: &Path,
    source: &Path,
    summary: &Path,
    target: &Path,
    mut progress: impl FnMut(usize, usize),
) -> Result<usize, String> {
    let mut files = migration_plan(source, summary, target)?;
    let total = files.len();
    for (i, file) in files.iter_mut().enumerate() {
        fs::create_dir_all(file.destination.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut input = fs::File::open(&file.source).map_err(|e| e.to_string())?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file.destination)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        file.sha256 = hash(&file.source)?;
        if file.sha256 != hash(&file.destination)? {
            return Err("模型复制校验失败，仍使用原目录。".into());
        }
        progress(i + 1, total);
    }
    fs::create_dir_all(target.join("summary")).map_err(|e| e.to_string())?;
    // One atomic configuration commit switches both engines. Originals are only
    // removed on next startup, after old processes and mmap handles have exited.
    persist(
        root,
        &Configuration {
            directory: fs::canonicalize(target).map_err(|e| e.to_string())?,
            cleanup: files,
        },
    )?;
    Ok(total)
}

fn remove_verified_copy(file: &CopiedFile, directory: &Path) -> Result<(), String> {
    if !file.source.exists() {
        return Ok(());
    }
    let source = fs::canonicalize(&file.source).map_err(|e| e.to_string())?;
    let destination = fs::canonicalize(&file.destination).map_err(|e| e.to_string())?;
    let directory = fs::canonicalize(directory).map_err(|e| e.to_string())?;
    if source != file.source
        || destination != file.destination
        || source.starts_with(&directory)
        || !destination.starts_with(&directory)
        || is_link(&fs::symlink_metadata(&file.source).map_err(|e| e.to_string())?)
    {
        return Err("模型清理路径已变化，已保留原文件。".into());
    }
    if hash(&source)? != file.sha256 || hash(&destination)? != file.sha256 {
        return Err("模型清理校验不一致，已保留原文件。".into());
    }
    fs::remove_file(&source).map_err(|e| e.to_string())
}

pub fn initialize(root: &Path) -> Result<(), String> {
    if let Some(mut config) = read_config(root)? {
        if !config.directory.is_dir() {
            return Err("模型目录不可用，请连接对应磁盘后重启。".into());
        }
        if !config.cleanup.is_empty() {
            let pending = std::mem::take(&mut config.cleanup);
            for file in pending {
                if let Err(error) = remove_verified_copy(&file, &config.directory) {
                    log::warn!(
                        "Model source retained: {}: {}",
                        file.source.display(),
                        error
                    );
                    config.cleanup.push(file);
                }
            }
            persist(root, &config)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_model_storage(app: AppHandle) -> Result<serde_json::Value, String> {
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let config = read_config(&root)?;
    // Use the shared recording root, not a company subdirectory: models contain
    // no meeting data and are reusable across companies on the same computer.
    let registry: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("companies.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let recordings = match registry.get("recording_root").and_then(|v| v.as_str()) {
        Some(p) => PathBuf::from(p),
        None => {
            crate::audio::recording_preferences::load_recording_preferences(&app)
                .await
                .map_err(|e| e.to_string())?
                .save_folder
        }
    };
    Ok(
        serde_json::json!({"directory":models_directory(&root),"summary_directory":crate::summary::summary_engine::models::get_models_directory(&root),"alongside_recordings":recordings.join("models"),"pending_cleanup":config.map(|c|c.cleanup.len()).unwrap_or(0)}),
    )
}

#[tauri::command]
pub async fn select_model_storage_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择空的模型保存文件夹")
            .blocking_pick_folder()
            .map(|p| p.to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn migrate_model_storage(
    app: AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    directory: String,
) -> Result<(), String> {
    let storage = crate::audio::storage_migration::STORAGE_LOCK
        .try_write()
        .map_err(|_| "正在处理会议文件，请完成后再迁移模型。")?;
    let models = MODEL_FILES
        .try_write()
        .map_err(|_| "正在下载、删除或使用模型，请完成后再迁移。")?;
    let batch = crate::audio::common::BATCH_INFERENCE_LOCK
        .try_lock()
        .map_err(|_| "正在转写，请完成或暂停后再迁移。")?;
    let delivery = crate::lark::app::DELIVERY_LOCK
        .try_lock()
        .map_err(|_| "正在发送纪要，请等待完成。")?;
    let webhook = crate::lark::SEND_LOCK
        .try_lock()
        .map_err(|_| "正在发送纪要，请等待完成。")?;
    crate::company::ensure_idle(state.db_manager.pool()).await?;
    let root = app.path().app_data_dir().map_err(|e| e.to_string())?;
    if read_config(&root)?.is_some_and(|c| !c.cleanup.is_empty()) {
        return Err("上次迁移的原文件尚未清理，请重启后重试。".into());
    }
    let source = models_directory(&root);
    let summary = crate::summary::summary_engine::models::get_models_directory(&root);
    let target = PathBuf::from(directory);
    let progress_app = app.clone();
    tokio::task::spawn_blocking(move || {
        copy_and_commit(&root, &source, &summary, &target, |done, total| {
            let _ = progress_app.emit(
                "model-storage-progress",
                serde_json::json!({"done":done,"total":total}),
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        format!("迁移未完成，原模型仍保留；目标目录可能有部分副本，请另选空目录重试。{e}")
    })?;
    // Keep gates closed until restart so no work can use a mixture of old/new paths.
    tauri::async_runtime::spawn(async move {
        let _guards = (storage, models, batch, delivery, webhook);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        app.restart();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_storage_migration_switches_all_engines_and_cleans_only_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("配置");
        let old = app.join("models");
        let summary = temp.path().join("独立纪要模型");
        let target = temp.path().join("录音/models");
        fs::create_dir_all(old.join("parakeet/v3")).unwrap();
        fs::create_dir_all(&summary).unwrap();
        fs::write(old.join("ggml-small.bin"), [0, 1, 2, 255]).unwrap();
        fs::write(old.join("parakeet/v3/model.onnx"), b"onnx").unwrap();
        fs::write(summary.join("qwen.gguf"), b"gguf").unwrap();
        let mut progress = Vec::new();
        assert_eq!(
            copy_and_commit(&app, &old, &summary, &target, |n, total| progress
                .push((n, total)))
            .unwrap(),
            3
        );
        assert_eq!(progress.last(), Some(&(3, 3)));
        assert!(old.join("ggml-small.bin").exists());
        assert_eq!(models_directory(&app), fs::canonicalize(&target).unwrap());
        assert_eq!(
            crate::summary::summary_engine::models::get_models_directory(&app),
            fs::canonicalize(&target).unwrap().join("summary")
        );
        initialize(&app).unwrap();
        assert!(!old.join("ggml-small.bin").exists());
        assert!(!summary.join("qwen.gguf").exists());
        assert_eq!(
            fs::read(target.join("ggml-small.bin")).unwrap(),
            [0, 1, 2, 255]
        );
        assert_eq!(
            fs::read(target.join("parakeet/v3/model.onnx")).unwrap(),
            b"onnx"
        );
        assert!(read_config(&app).unwrap().unwrap().cleanup.is_empty());
        initialize(&app).unwrap();
    }

    #[test]
    fn model_storage_keeps_original_when_destination_or_source_changes() {
        for changed_source in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let app = temp.path();
            let old = app.join("models");
            let target = app.join("target");
            fs::create_dir_all(&old).unwrap();
            fs::write(old.join("model.bin"), b"original").unwrap();
            copy_and_commit(app, &old, &old.join("summary"), &target, |_, _| {}).unwrap();
            fs::write(
                if changed_source {
                    old.join("model.bin")
                } else {
                    target.join("model.bin")
                },
                b"changed",
            )
            .unwrap();
            initialize(app).unwrap();
            assert!(old.join("model.bin").exists());
            assert_eq!(read_config(app).unwrap().unwrap().cleanup.len(), 1);
        }
    }

    #[test]
    fn model_storage_rejects_nested_relative_and_nonempty_destinations() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("models");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("model.bin"), b"keep").unwrap();
        for target in [
            old.clone(),
            old.join("nested"),
            temp.path().into(),
            PathBuf::from("relative/models"),
        ] {
            assert!(migration_plan(&old, &old.join("summary"), &target).is_err());
        }
        assert_eq!(fs::read(old.join("model.bin")).unwrap(), b"keep");
        assert!(!temp.path().join(CONFIG).exists());
    }

    #[test]
    fn model_storage_failed_config_commit_never_removes_source() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("models");
        let target = temp.path().join("target");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("model.bin"), b"keep").unwrap();
        assert!(copy_and_commit(
            &temp.path().join("missing-config-root"),
            &old,
            &old.join("summary"),
            &target,
            |_, _| {}
        )
        .is_err());
        assert_eq!(fs::read(old.join("model.bin")).unwrap(), b"keep");
        assert!(target.join("model.bin").exists());
    }

    #[test]
    fn model_storage_empty_migration_and_missing_drive_fail_safely() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        copy_and_commit(
            temp.path(),
            &temp.path().join("missing"),
            &temp.path().join("missing-summary"),
            &target,
            |_, _| {},
        )
        .unwrap();
        initialize(temp.path()).unwrap();
        let config = Configuration {
            directory: temp.path().join("offline-drive"),
            cleanup: Vec::new(),
        };
        persist(temp.path(), &config).unwrap();
        assert!(initialize(temp.path()).is_err());
        assert_eq!(models_directory(temp.path()), config.directory);
    }

    #[test]
    fn model_storage_invalid_configuration_is_not_silently_replaced() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(CONFIG), "broken").unwrap();
        assert!(initialize(temp.path()).is_err());
        assert_eq!(
            fs::read_to_string(temp.path().join(CONFIG)).unwrap(),
            "broken"
        );
    }

    #[tokio::test]
    async fn model_storage_write_gate_excludes_downloads_and_releases_on_failure() {
        let guard = MODEL_FILES.write().await;
        assert!(use_model_files().await.is_err());
        drop(guard);
        let reader = use_model_files().await.unwrap();
        assert!(MODEL_FILES.try_write().is_err());
        drop(reader);
        assert!(MODEL_FILES.try_write().is_ok());
    }
}
