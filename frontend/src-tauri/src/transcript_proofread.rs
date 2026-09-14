//! 「AI 校对」：用已配置的摘要模型（本地 Qwen / 任意 OpenAI 兼容服务）逐段找出
//! **同音字、专有名词/术语写错、明显错别字**，只产出候选，不自动改。
//! 用户在前端勾选后由 `api_apply_transcript_proofread_edits` 写回（写回前自动备份）。
//!
//! 设计约束（写进提示词，也在解析时校验）：
//! 1. 只输出 JSON；2. `original` 必须逐字出现在对应片段里；3. 不改标点、不改写句子、不动时间戳；
//! 4. 不确定就不提；5. 校验不过的候选直接丢弃。
//!
//! v2（2026-09-12）改了四件事，都是为了"能发现明显错误、且不改错"：
//! - 提示词改成**逐段必答**（`{"segments":[{segment,verdict,edits}]}`），并要求先判"读起来通不通"再判错别字；
//! - 明确禁止同义词替换（实测 4B 会把 `影片` 改成 `视频`）；
//! - 漏答的片段会被**单独重问一次**，仍漏答就报 warning，不再静默当成"没问题"；
//! - 模型把片段编号写错时，按"这段原文到底在哪个片段里"**自动纠正归属**（实测发生过）。
//! - 每次校对的模型原始输出会写进会议文件夹的 `proofread-diagnostics-*.jsonl`，
//!   出问题不用再靠复现实验断案。

use anyhow::{anyhow, Context, Result};
use crate::transcript_file_store;
use crate::database::repositories::setting::SettingsRepository;
use crate::meeting_context::{load_recognition_context, RecognitionContext, RecognitionContextSelection};
use crate::state::AppState;
use crate::summary::llm_client::{generate_summary, LLMProvider};
use crate::transcript_revision::{
    load_current_segments, write_rows_backup, PROOFREAD_BACKUP_FILE_PREFIX,
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::transcript_text_edit::{apply_text_edits, TextEdit};
pub use crate::proofread_protocol::ProofreadCandidate;
use crate::proofread_protocol::{ProofreadRow, ParsedBatch, PROMPT_VERSION, SYSTEM_PROMPT, build_batch_user_prompt, build_retry_user_prompt, parse_candidates, build_review_batches};
use tauri_plugin_store::StoreExt;
use tauri::{AppHandle, Manager, Runtime, State};

/// 「文字纠错」用哪个模型跑，跟摘要模型分开存。
///
/// 背景（2026-09-12 实测）：校对复用的是"当前配置的摘要模型"。用户为了省钱把摘要切成本地
/// Qwen3.5-4B 之后，校对质量跟着掉（漏 `年→你`、误改 `影片→视频`）。摘要用本地、校对用 API
/// 是合理的组合，所以给校对一个独立、可回退的偏好。
const PROOFREAD_PREFERENCES_STORE: &str = "proofread_preferences.json";
const PROOFREAD_TARGET_KEY: &str = "target";

/// 「文字纠错」用哪个模型跑。
///
/// 不限于"自定义 OpenAI"：任何一个已配置好的 API 供应商（OpenAI / Claude / Groq /
/// OpenRouter / Ollama / 自定义 OpenAI 兼容服务）都可以选来跑校对。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProofreadTarget {
    /// 跟随「设置 → 摘要」里的模型（默认，保持老行为）
    Summary,
    /// 指定某个供应商 + 模型
    Provider { provider: String, model: String },
}

impl Default for ProofreadTarget {
    fn default() -> Self {
        Self::Summary
    }
}

/// 前端"校对用模型"下拉里的一项
#[derive(Debug, Clone, Serialize)]
pub struct ProofreadModelOption {
    pub target: ProofreadTarget,
    /// 拆开给前端拼文案用（后端不返回中文句子，避免绕过 i18n）
    pub provider: String,
    pub model: String,
    /// 本地内置模型（能力有限，前端要给提示）
    pub is_local: bool,
}

async fn load_proofread_target<R: Runtime>(app: &AppHandle<R>) -> ProofreadTarget {
    let Ok(store) = app.store(PROOFREAD_PREFERENCES_STORE) else {
        return ProofreadTarget::default();
    };
    match store.get(PROOFREAD_TARGET_KEY) {
        Some(value) => {
            serde_json::from_value::<ProofreadTarget>(value.clone()).unwrap_or_default()
        }
        None => ProofreadTarget::default(),
    }
}

async fn save_proofread_target<R: Runtime>(
    app: &AppHandle<R>,
    target: &ProofreadTarget,
) -> Result<(), String> {
    let store = app
        .store(PROOFREAD_PREFERENCES_STORE)
        .map_err(|error| format!("打开偏好存储失败：{error}"))?;
    store.set(
        PROOFREAD_TARGET_KEY,
        serde_json::to_value(target).map_err(|error| error.to_string())?,
    );
    store
        .save()
        .map_err(|error| format!("保存偏好失败：{error}"))
}

#[tauri::command]
pub async fn api_get_proofread_target<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ProofreadTarget, String> {
    Ok(load_proofread_target(&app).await)
}

#[tauri::command]
pub async fn api_set_proofread_target<R: Runtime>(
    app: AppHandle<R>,
    target: ProofreadTarget,
) -> Result<(), String> {
    save_proofread_target(&app, &target).await
}

/// 这台机器上"能拿来跑校对"的模型列表：摘要模型 + 每个已配置好的 API 供应商。
#[tauri::command]
pub async fn api_list_proofread_models<R: Runtime>(
    app: AppHandle<R>,
    known_models: Option<HashMap<String, String>>,
    app_state: State<'_, AppState>,
) -> Result<Vec<ProofreadModelOption>, String> {
    let pool = app_state.db_manager.pool();
    let setting = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|error| error.to_string())?;
    let mut options = Vec::new();

    let summary_provider = setting
        .as_ref()
        .map(|setting| setting.provider.clone())
        .unwrap_or_default();
    let summary_model = setting
        .as_ref()
        .map(|setting| setting.model.clone())
        .unwrap_or_default();
    if !summary_model.is_empty() {
        options.push(ProofreadModelOption {
            target: ProofreadTarget::Summary,
            provider: summary_provider.clone(),
            model: summary_model.clone(),
            is_local: summary_provider == "builtin-ai",
        });
    }

    let custom = SettingsRepository::get_custom_openai_config(pool)
        .await
        .ok()
        .flatten();
    if let Some(config) = custom {
        if !config.endpoint.trim().is_empty() && !config.model.trim().is_empty() {
            options.push(ProofreadModelOption {
                target: ProofreadTarget::Provider {
                    provider: "custom-openai".to_string(),
                    model: config.model.clone(),
                },
                provider: "custom-openai".to_string(),
                model: config.model,
                is_local: false,
            });
        }
    }

    // Preserve saved and previously selected models even when the summary provider changes.
    let saved = load_proofread_target(&app).await;
    let mut known = known_models.unwrap_or_default();
    if !summary_model.is_empty() {
        known.insert(summary_provider.clone(), summary_model);
    }
    let configured: Vec<_> = ["openai", "claude", "groq", "openrouter", "ollama"]
        .into_iter().collect();
    let catalogs = futures_util::future::join_all(configured.into_iter().map(|provider| async move {
        let key = SettingsRepository::get_api_key(pool, provider).await.ok().flatten();
        if provider != "ollama" && !key.as_ref().is_some_and(|key| !key.trim().is_empty()) {
            return (provider, Vec::new());
        }
        let fetch = async {
            match provider {
                "openai" => crate::openai::openai::get_openai_models(key).await.map(|rows| rows.into_iter().map(|row| row.id).collect()),
                "claude" => crate::anthropic::anthropic::get_anthropic_models(key).await.map(|rows| rows.into_iter().map(|row| row.id).collect()),
                "groq" => crate::groq::groq::get_groq_models(key).await.map(|rows| rows.into_iter().map(|row| row.id).collect()),
                "ollama" => {
                    let endpoint = SettingsRepository::get_model_config(pool).await.ok().flatten().and_then(|s| s.ollama_endpoint);
                    crate::ollama::ollama::get_ollama_models(endpoint).await.map(|rows| rows.into_iter().map(|row| row.name).collect())
                }
                "openrouter" => {
                    let response = reqwest::Client::new().get("https://openrouter.ai/api/v1/models")
                        .send().await.map_err(|error| error.to_string())?
                        .error_for_status().map_err(|error| error.to_string())?
                        .json::<serde_json::Value>().await.map_err(|error| error.to_string())?;
                    Ok(response["data"].as_array().into_iter().flatten().filter_map(|row| row["id"].as_str().map(str::to_owned)).collect())
                }
                _ => Ok(Vec::new()),
            }
        };
        let models: Vec<String> = tokio::time::timeout(std::time::Duration::from_secs(6), fetch).await.ok().and_then(Result::ok).unwrap_or_default();
        (provider, models)
    })).await;
    let mut targets: Vec<ProofreadTarget> = catalogs.into_iter().flat_map(|(provider, models)| {
        models.into_iter().map(move |model| ProofreadTarget::Provider { provider: provider.into(), model })
    }).collect();
    for (provider, model) in known {
        let has_key = SettingsRepository::get_api_key(pool, &provider).await.ok().flatten().is_some_and(|key| !key.trim().is_empty());
        if has_key || matches!(provider.as_str(), "ollama" | "builtin-ai") {
            targets.push(ProofreadTarget::Provider { provider, model });
        }
    }
    targets.push(saved);
    for target in targets {
        if let ProofreadTarget::Provider { provider, model } = &target {
            if !model.trim().is_empty() && LLMProvider::from_str(provider).is_ok() && !options.iter().any(|option| option.target == target) {
                options.push(ProofreadModelOption { provider: provider.clone(), model: model.clone(), is_local: provider == "builtin-ai" || provider == "ollama", target });
            }
        }
    }

    Ok(options)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofreadEdit {
    pub segment_id: String,
    pub original: String,
    pub suggested: String,
    pub expected_text: String,
    pub start_char: usize,
    pub end_char: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofreadResponse {
    pub provider: String,
    pub model: String,
    pub total_segments: usize,
    pub reviewed_segments: usize,
    pub truncated: bool,
    pub candidates: Vec<ProofreadCandidate>,
    pub elapsed_ms: u64,
    /// 逐批送审时出现的失败信息（例如某批超时），不阻断整体
    pub warnings: Vec<String>,
    /// 提示词版本（诊断用）
    pub prompt_version: String,
    /// 最终仍没给出结论的片段编号（0 起、按会议顺序）
    pub missing_segments: Vec<usize>,
    /// 本次校对的诊断文件（会议文件夹内，含模型原始输出）
    pub diagnostics_file: Option<String>,
    /// 本次实际用的是哪个模型（跟随摘要模型 / 指定供应商）
    pub target: ProofreadTarget,
    pub start_index: usize,
    pub next_start_index: Option<usize>,
    pub source_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofreadApplyResponse {
    pub file_sync_pending: bool,
    pub applied_edits: usize,
    pub skipped_edits: usize,
    pub updated_segments: usize,
    pub backup_file: Option<String>,
}

fn build_context_line(context: Option<&RecognitionContext>) -> String {
    match context {
        Some(context) => {
            let names = context.canonical_names.join("、");
            let terms = context.canonical_terms.join("、");
            let mut parts = Vec::new();
            if !names.is_empty() {
                parts.push(format!("参会人/人名：{names}"));
            }
            if !terms.is_empty() {
                parts.push(format!("术语：{terms}"));
            }
            if parts.is_empty() {
                "（这场会议没有登记人名或术语）".to_string()
            } else {
                parts.join("；")
            }
        }
        None => "（这场会议没有登记人名或术语）".to_string(),
    }
}

/// 用当前配置的摘要模型逐段校对，只返回候选（不写库）
///
/// `target` 指定本次用哪个模型跑（不传就用存下来的偏好，默认跟随摘要模型）。
/// 支持任何已配置好的供应商：OpenAI / Claude / Groq / OpenRouter / Ollama /
/// 自定义 OpenAI 兼容服务 —— 本地小模型看不见的错误，用 API 模型跑一遍就能抓到。
#[tauri::command]
pub async fn api_review_transcript_with_llm<R: Runtime>(
    app: AppHandle<R>,
    app_state: State<'_, AppState>,
    meeting_id: String,
    meeting_folder_path: String,
    target: Option<ProofreadTarget>,
    start_index: Option<usize>,
    expected_source_hash: Option<String>,
) -> Result<ProofreadResponse, String> {
    let started = std::time::Instant::now();
    let pool = app_state.db_manager.pool();

    let setting = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "还没有配置摘要模型（设置 → 摘要）".to_string())?;
    let custom_config = match SettingsRepository::get_custom_openai_config(pool).await {
        Ok(config) => config,
        Err(error) => {
            warn!("Failed to read custom OpenAI config: {error}");
            None
        }
    };

    // 本次用哪个模型：界面显式传的 > 存下来的偏好（默认=跟随摘要模型）
    let resolved_target = match target {
        Some(explicit) => explicit,
        None => load_proofread_target(&app).await,
    };

    let (provider_name, model_name) = match &resolved_target {
        ProofreadTarget::Summary => (setting.provider.clone(), setting.model.clone()),
        ProofreadTarget::Provider { provider, model } => (provider.clone(), model.clone()),
    };
    let provider = LLMProvider::from_str(&provider_name).map_err(|error| error)?;

    // Never silently route transcript content to a different provider.
    if model_name.trim().is_empty() {
        return Err("proofread_model_unavailable".into());
    }
    if provider == LLMProvider::CustomOpenAI && !custom_config.as_ref().is_some_and(|config| !config.endpoint.trim().is_empty() && config.model == model_name) {
        return Err("proofread_model_unavailable".into());
    }

    let ollama_endpoint = if provider == LLMProvider::Ollama {
        setting.ollama_endpoint.clone()
    } else {
        None
    };
    let custom_endpoint = if provider == LLMProvider::CustomOpenAI {
        custom_config.as_ref().map(|config| config.endpoint.clone())
    } else {
        None
    };
    let effective_api_key = match provider {
        LLMProvider::BuiltInAI => String::new(),
        LLMProvider::Ollama => SettingsRepository::get_api_key(pool, &provider_name).await.map_err(|error| error.to_string())?.unwrap_or_default(),
        // CustomOpenAI 的 key 存在它自己的配置里（跟摘要链路保持一致，否则会 401）
        LLMProvider::CustomOpenAI => custom_config
            .as_ref()
            .and_then(|config| config.api_key.clone())
            .unwrap_or_default(),
        _ => SettingsRepository::get_api_key(pool, &provider_name)
            .await
            .map_err(|error| error.to_string())?
            .filter(|key| !key.is_empty())
            .ok_or_else(|| "proofread_model_unavailable".to_string())?,
    };

    let custom_max_tokens = custom_config
        .as_ref()
        .filter(|_| provider == LLMProvider::CustomOpenAI)
        .and_then(|config| config.max_tokens.map(|tokens| tokens as u32));
    let custom_temperature = custom_config
        .as_ref()
        .filter(|_| provider == LLMProvider::CustomOpenAI)
        .and_then(|config| config.temperature);
    let custom_top_p = custom_config
        .as_ref()
        .filter(|_| provider == LLMProvider::CustomOpenAI)
        .and_then(|config| config.top_p);

    let summary_models_dir = app
        .state::<crate::storage::StorageLayoutState>()
        .layout()
        .summary_models_dir()
        .to_path_buf();

    let folder = PathBuf::from(&meeting_folder_path);
    let context = load_recognition_context(&folder, RecognitionContextSelection::Current)
        .ok()
        .flatten();
    let context_line = build_context_line(context.as_ref());

    let rows = load_current_segments(pool, &meeting_id)
        .await
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Err("这场会议还没有转写内容".to_string());
    }

    use sha2::{Digest, Sha256};
    let source = rows.iter().map(|row| (&row.id, &row.transcript)).collect::<Vec<_>>();
    let source_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&source).map_err(|error| error.to_string())?));
    if expected_source_hash.as_ref().is_some_and(|expected| expected != &source_hash) {
        return Err("proofread_source_changed".into());
    }
    let start_index = start_index.unwrap_or(0);
    if start_index >= rows.len() {
        return Err("proofread_invalid_range".into());
    }

    let client = reqwest::Client::new();
    let mut candidates: Vec<ProofreadCandidate> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut reviewed_segments = 0usize;
    let mut missing_segments: Vec<usize> = Vec::new();
    let mut diagnostics: Vec<serde_json::Value> = Vec::new();

    let rows: Vec<ProofreadRow> = rows.iter().map(|row| ProofreadRow {
        id: row.id.clone(), transcript: row.transcript.clone(), audio_start_time: row.audio_start_time,
    }).collect();
    let batches = build_review_batches(&rows, start_index);
    let next_index = start_index + batches.iter().map(Vec::len).sum::<usize>();
    let next_start_index = (next_index < rows.len()).then_some(next_index);

    for (batch_index, current_batch) in batches.iter().enumerate() {
        let user_prompt = build_batch_user_prompt(current_batch, &context_line);
        let started_batch = std::time::Instant::now();
        let outcome = generate_summary(
            &client,
            &provider,
            &model_name,
            &effective_api_key,
            SYSTEM_PROMPT,
            &user_prompt,
            ollama_endpoint.as_deref(),
            custom_endpoint.as_deref(),
            custom_max_tokens,
            custom_temperature,
            custom_top_p,
            Some(&summary_models_dir),
            None,
        )
        .await;

        let mut parsed = match outcome {
            Ok(raw) => {
                let parsed = parse_candidates(&raw, current_batch, &context_line);
                reviewed_segments += current_batch.len();
                diagnostics.push(batch_diagnostic(
                    batch_index,
                    "primary",
                    &provider_name,
                    &model_name,
                    current_batch,
                    &raw,
                    &parsed,
                    started_batch.elapsed().as_millis() as u64,
                ));
                parsed
            }
            Err(error) => {
                missing_segments.extend(current_batch.iter().map(|(index, _)| *index));
                warnings.push(format!(
                    "第 {} 批校对失败：{}",
                    batch_index + 1,
                    error
                ));
                diagnostics.push(serde_json::json!({
                    "batch_index": batch_index,
                    "attempt": "primary",
                    "provider": provider_name,
                    "model": model_name,
                    "error": error,
                    "segments": current_batch.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                }));
                continue;
            }
        };

        // 逐段必答：漏答的片段单独再问一次（4B 常见"只挑几段回答"，不能整段放弃）
        if !parsed.missing.is_empty() {
            let retry_rows: Vec<(usize, &ProofreadRow)> = current_batch
                .iter()
                .filter(|(index, _)| parsed.missing.contains(index))
                .map(|(index, row)| (*index, *row))
                .collect();
            let retry_prompt = build_retry_user_prompt(&retry_rows, &context_line);
            let started_retry = std::time::Instant::now();
            match generate_summary(
                &client,
                &provider,
                &model_name,
                &effective_api_key,
                SYSTEM_PROMPT,
                &retry_prompt,
                ollama_endpoint.as_deref(),
                custom_endpoint.as_deref(),
                custom_max_tokens,
                custom_temperature,
                custom_top_p,
                Some(&summary_models_dir),
                None,
            )
            .await
            {
                Ok(raw) => {
                    let mut retry_parsed = parse_candidates(&raw, &retry_rows, &context_line);
                    diagnostics.push(batch_diagnostic(
                        batch_index,
                        "retry",
                        &provider_name,
                        &model_name,
                        &retry_rows,
                        &raw,
                        &retry_parsed,
                        started_retry.elapsed().as_millis() as u64,
                    ));
                    parsed.candidates.append(&mut retry_parsed.candidates);
                    parsed.reassigned += retry_parsed.reassigned;
                    parsed.dropped.append(&mut retry_parsed.dropped);
                    parsed.missing = retry_parsed.missing;
                }
                Err(error) => {
                    diagnostics.push(serde_json::json!({
                        "batch_index": batch_index,
                        "attempt": "retry",
                        "provider": provider_name,
                        "model": model_name,
                        "error": error,
                        "segments": retry_rows.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    }));
                }
            }
        }

        if !parsed.missing.is_empty() {
            missing_segments.extend(parsed.missing.iter().copied());
        }
        candidates.extend(parsed.candidates);
    }

    missing_segments.sort_unstable();
    missing_segments.dedup();
    if !missing_segments.is_empty() {
        warnings.push(format!(
            "有 {} 段模型没有给出结论（编号 {}）",
            missing_segments.len(),
            missing_segments
                .iter()
                .map(|index| index.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // 诊断落盘：没有它就无法事后区分"模型没提 / JSON 坏了 / 候选被校验丢掉"
    let diagnostics_file = write_diagnostics(&folder, &diagnostics, PROMPT_VERSION)
        .map_err(|error| {
            warn!("Failed to write proofread diagnostics: {error}");
            error
        })
        .ok();

    if !warnings.is_empty() {
        warn!("Transcript proofread warnings: {:?}", warnings);
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    info!(
        "Transcript proofread finished ({}): {} candidate(s) from {} segment(s) in {} ms, {} segment(s) unanswered, responded {} / reassigned {}",
        PROMPT_VERSION,
        candidates.len(),
        reviewed_segments,
        elapsed_ms,
        missing_segments.len(),
        diagnostics.len(),
        diagnostics_reassigned(&diagnostics),
    );

    Ok(ProofreadResponse {
        provider: provider_name,
        model: model_name,
        total_segments: rows.len(),
        reviewed_segments,
        truncated: next_start_index.is_some(),
        candidates,
        elapsed_ms,
        warnings,
        prompt_version: PROMPT_VERSION.to_string(),
        missing_segments,
        diagnostics_file,
        target: resolved_target,
        start_index,
        next_start_index,
        source_hash,
    })
}

/// 诊断记录里"片段编号认错、已自动纠正"的总次数（汇总日志用）
fn diagnostics_reassigned(diagnostics: &[serde_json::Value]) -> u64 {
    diagnostics
        .iter()
        .filter_map(|entry| entry.get("reassigned").and_then(|value| value.as_u64()))
        .sum()
}

/// 一批送审的诊断记录：模型原始输出 + 解析结果 + 丢弃原因 + 覆盖情况
fn batch_diagnostic(
    batch_index: usize,
    attempt: &str,
    provider: &str,
    model: &str,
    batch: &[(usize, &ProofreadRow)],
    raw: &str,
    parsed: &ParsedBatch,
    elapsed_ms: u64,
) -> serde_json::Value {
    const MAX_RAW_CHARS: usize = 20000;
    let raw: String = raw.chars().take(MAX_RAW_CHARS).collect();
    serde_json::json!({
        "batch_index": batch_index,
        "attempt": attempt,
        "provider": provider,
        "model": model,
        "segments": batch
            .iter()
            .map(|(index, row)| serde_json::json!({
                "index": index,
                "audio_start_time": row.audio_start_time,
                "text": row.transcript,
            }))
            .collect::<Vec<_>>(),
        "answered": parsed.answered,
        "missing": parsed.missing,
        "reassigned": parsed.reassigned,
        "dropped": parsed.dropped,
        "candidates": parsed
            .candidates
            .iter()
            .map(|candidate| serde_json::json!({
                "segment_index": candidate.segment_index,
                "original": candidate.original,
                "suggested": candidate.suggested,
                "reason": candidate.reason,
                "confidence": candidate.confidence,
            }))
            .collect::<Vec<_>>(),
        "raw_response": raw,
        "elapsed_ms": elapsed_ms,
    })
}

/// 把本次校对的诊断写进会议文件夹，返回文件名（失败不阻断校对本身）
fn write_diagnostics(
    folder: &PathBuf,
    diagnostics: &[serde_json::Value],
    prompt_version: &str,
) -> Result<String> {
    if diagnostics.is_empty() {
        return Err(anyhow!("no diagnostics to write"));
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let file_name = format!("proofread-diagnostics-{stamp}-{}.jsonl", uuid::Uuid::new_v4());
    let path = folder.join(&file_name);
    let mut body = String::new();
    body.push_str(&serde_json::json!({
        "kind": "proofread-diagnostics-header",
        "prompt_version": prompt_version,
        "created_at": chrono::Utc::now().to_rfc3339(),
    })
    .to_string());
    body.push('\n');
    for entry in diagnostics {
        body.push_str(&entry.to_string());
        body.push('\n');
    }
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(file_name)
}

/// 把用户勾选的候选写回（写回前自动备份；找不到 original 的候选会被跳过）
#[tauri::command]
pub async fn api_apply_transcript_proofread_edits(
    app_state: State<'_, AppState>,
    meeting_id: String,
    meeting_folder_path: String,
    edits: Vec<ProofreadEdit>,
) -> Result<ProofreadApplyResponse, String> {
    if edits.is_empty() {
        return Err("没有选中的修改".to_string());
    }
    let folder = PathBuf::from(&meeting_folder_path);
    let pool = app_state.db_manager.pool();
    let _write_guard = transcript_file_store::WRITE_LOCK.lock().await;
    transcript_file_store::ensure_ready(pool, &meeting_id).await?;
    let rows = load_current_segments(pool, &meeting_id)
        .await
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Err("这场会议还没有转写内容".to_string());
    }

    let mut text_by_id: HashMap<String, String> = HashMap::new();
    let applied_edits = edits.len();
    let skipped_edits = 0usize;
    for edit in &edits {
        if !rows.iter().any(|row| row.id == edit.segment_id) {
            return Err("proofread_source_changed".into());
        }
    }
    for row in &rows {
        let selected: Vec<_> = edits.iter().filter(|edit| edit.segment_id == row.id)
            .map(|edit| TextEdit {
                expected_text: &edit.expected_text, original: &edit.original, suggested: &edit.suggested,
                start_char: edit.start_char, end_char: edit.end_char,
            }).collect();
        if !selected.is_empty() {
            text_by_id.insert(row.id.clone(), apply_text_edits(&row.transcript, &selected)?);
        }
    }
    let updated_segments = text_by_id.len();

    let backup_file = write_rows_backup(
        &folder,
        &meeting_id,
        &rows,
        PROOFREAD_BACKUP_FILE_PREFIX,
        "pre-proofread-backup",
    )
    .map_err(|error| error.to_string())?;

    let mut conn = pool.acquire().await.map_err(|error| error.to_string())?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|error| error.to_string())?;
    for row in &rows {
        let Some(next) = text_by_id.get(&row.id) else {
            continue;
        };
        if next == &row.transcript {
            continue;
        }
        let updated = sqlx::query("UPDATE transcripts SET transcript = ? WHERE id = ? AND meeting_id = ? AND transcript = ?")
            .bind(next)
            .bind(&row.id)
            .bind(&meeting_id)
            .bind(&row.transcript)
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
        if updated.rows_affected() != 1 {
            return Err("proofread_source_changed".into());
        }
    }
    transcript_file_store::stage(&mut tx, &meeting_id, &folder, serde_json::json!({})).await.map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;

    let file_sync_pending = transcript_file_store::finish(pool, &meeting_id).await;

    info!(
        "Applied {} proofread edit(s) to {} segment(s) of meeting {} (skipped {})",
        applied_edits,
        updated_segments,
        meeting_id,
        skipped_edits
    );

    Ok(ProofreadApplyResponse {
        file_sync_pending,
        applied_edits,
        skipped_edits,
        updated_segments,
        backup_file,
    })
}

