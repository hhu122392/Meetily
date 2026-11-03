use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use sha2::{Digest, Sha256};

async fn server(status: &str, body: Vec<u8>, extra: &str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    let header = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 8192];
        let len = socket.read(&mut request).await.unwrap();
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(&body).await;
        String::from_utf8_lossy(&request[..len]).to_string()
    });
    (url, task)
}

fn fixture() -> (ModelDef, Vec<u8>, String) {
    let mut model = get_model_by_name("qwen3.5:4b").unwrap();
    model.size_mb = 1;
    let mut data = vec![42; 1024 * 1024];
    data[..4].copy_from_slice(b"GGUF");
    let hash = format!("{:x}", Sha256::digest(&data));
    (model, data, hash)
}

#[tokio::test]
async fn connect_failure_releases_owner_and_retry_can_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ModelManager::new_with_models_dir(Some(dir.path().into())).unwrap();
    manager.init().await.unwrap();
    let (model, data, hash) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let refused = format!("http://{}/model", listener.local_addr().unwrap());
    drop(listener);
    assert!(manager.download_from_sources(&model, &[refused], Some(&hash), None).await.is_err());
    assert!(manager.active_downloads.read().await.is_empty());
    assert!(matches!(manager.get_model_info(&model.name).await.unwrap().status, ModelStatus::Error(_)));
    let (url, task) = server("200 OK", data.clone(), "").await;
    manager.download_from_sources(&model, &[url], Some(&hash), None).await.unwrap();
    task.await.unwrap();
    assert_eq!(tokio::fs::read(dir.path().join(&model.gguf_file)).await.unwrap(), data);
}

#[tokio::test]
async fn bad_hash_falls_back_without_replacing_existing_model() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ModelManager::new_with_models_dir(Some(dir.path().into())).unwrap();
    manager.init().await.unwrap();
    let (model, data, hash) = fixture();
    let final_path = dir.path().join(&model.gguf_file);
    tokio::fs::write(&final_path, b"old model").await.unwrap();
    let mut wrong = data.clone(); wrong[9] = 0;
    let (bad, first) = server("200 OK", wrong.clone(), "").await;
    assert!(manager.download_from_sources(&model, &[bad], Some(&hash), None).await.is_err());
    first.await.unwrap();
    assert_eq!(tokio::fs::read(&final_path).await.unwrap(), b"old model");
    assert!(!final_path.with_extension("gguf.part").exists());
    let (bad, first) = server("200 OK", wrong, "").await;
    let (good, second) = server("200 OK", data.clone(), "").await;
    manager.download_from_sources(&model, &[bad, good], Some(&hash), None).await.unwrap();
    first.await.unwrap(); second.await.unwrap();
    assert_eq!(tokio::fs::read(&final_path).await.unwrap(), data);
}

#[tokio::test]
async fn resumes_partial_file_and_checks_response_range() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ModelManager::new_with_models_dir(Some(dir.path().into())).unwrap();
    manager.init().await.unwrap();
    let (model, data, hash) = fixture();
    let part = dir.path().join(&model.gguf_file).with_extension("gguf.part");
    tokio::fs::write(&part, &data[..100]).await.unwrap();
    let (url, task) = server("206 Partial Content", data[100..].to_vec(), "Content-Range: bytes 100-1048575/1048576\r\n").await;
    manager.download_from_sources(&model, &[url], Some(&hash), None).await.unwrap();
    assert!(task.await.unwrap().to_lowercase().contains("range: bytes=100-"));
    assert!(!part.exists());
    assert!(resume_total("bytes 0-1048575/1048576", 100).is_err());
    assert!(resume_total("bytes 100-199/*", 100).is_err());
    assert!(resume_total("bytes 100-199/1048576", 100).is_err());
}

#[tokio::test]
async fn cancellation_interrupts_waiting_headers_and_releases_owner() {
    let dir = tempfile::tempdir().unwrap();
    let manager = std::sync::Arc::new(ModelManager::new_with_models_dir(Some(dir.path().into())).unwrap());
    manager.init().await.unwrap();
    let (model, _, hash) = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/model", listener.local_addr().unwrap());
    let worker = manager.clone();
    let task = tokio::spawn(async move { worker.download_from_sources(&model, &[url], Some(&hash), None).await });
    let (_socket, _) = listener.accept().await.unwrap();
    manager.cancel_download("qwen3.5:4b").await.unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), task).await.unwrap().unwrap().unwrap_err();
    assert!(error.to_string().starts_with("CANCELLED:"));
    assert!(manager.active_downloads.read().await.is_empty());
}

#[tokio::test]
async fn a_large_partial_without_checksum_is_not_published_as_complete() {
    let dir = tempfile::tempdir().unwrap();
    let manager = ModelManager::new_with_models_dir(Some(dir.path().into())).unwrap();
    manager.init().await.unwrap();
    let (model, data, _) = fixture();
    let part = dir.path().join(&model.gguf_file).with_extension("gguf.part");
    tokio::fs::write(&part, &data[..1000000]).await.unwrap();
    let (url, task) = server("200 OK", data.clone(), "").await;
    manager.download_from_sources(&model, &[url], None, None).await.unwrap();
    task.await.unwrap();
    assert_eq!(tokio::fs::read(dir.path().join(&model.gguf_file)).await.unwrap(), data);
}

#[tokio::test]
async fn directory_failure_does_not_poison_retry() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-directory");
    tokio::fs::write(&blocked, b"occupied").await.unwrap();
    let manager = ModelManager::new_with_models_dir(Some(blocked)).unwrap();
    let (model, _, hash) = fixture();
    for _ in 0..2 {
        let error = manager.download_from_sources(&model, &[], Some(&hash), None).await.unwrap_err();
        assert!(!error.to_string().contains("正在下载"));
        assert!(manager.active_downloads.read().await.is_empty());
    }
}

#[tokio::test]
#[ignore = "需要设置 MEETILY_SUMMARY_TEST_DIR；验证已有模型或执行完整联网下载"]
async fn real_summary_model_download() {
    let dir = std::env::var("MEETILY_SUMMARY_TEST_DIR").expect("需要独立测试目录");
    let manager = ModelManager::new_with_models_dir(Some(dir.into())).unwrap();
    manager.init().await.unwrap();
    manager.download_model_detailed("qwen3.5:4b", None).await.unwrap();
    assert_eq!(manager.get_model_info("qwen3.5:4b").await.unwrap().status, ModelStatus::Available);
}
