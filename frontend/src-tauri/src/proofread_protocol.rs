//! 客户端和离线回归共用的校对提示词、分批和解析协议。
use serde::{Deserialize, Serialize};
use log::warn;
use crate::transcript_text_edit::{apply_text_edits, TextEdit};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofreadRow {
    pub id: String,
    pub transcript: String,
    pub audio_start_time: Option<f64>,
}

/// 每次送审的片段数（本地 4B 模型上下文有限，宁少勿多）
const PROOFREAD_BATCH_SIZE: usize = 8;
/// 单次送审的文本上限（字符）
const MAX_BATCH_CHARS: usize = 6000;
/// 单次运行最多审多少段（避免长会议跑几十分钟）
const MAX_SEGMENTS_PER_RUN: usize = 60;

/// 提示词版本。改提示词或输出协议时同步改这里 —— 诊断日志会写下这个串，
/// 出问题能一眼看出"是哪版提示词跑出来的结果"。
pub const PROMPT_VERSION: &str = "proofread-v3.1-validated-ranges";

/// v3 提示词：把"开放式找错"改成"逐段必答 + 受约束判断"。
///
/// 当前协议要求逐段有效结论、唯一原文位置和受约束的改动。
/// 旧回归脚本曾漏计未答和误改，旧模型统计不能用来证明这一版的准确率。
/// 当前测试只验证协议、位置和判分；真实模型质量仍需独立标注集验证。
pub const SYSTEM_PROMPT: &str = r#"你是一个严谨的中文/英文会议转写校对员。你只做一件事：
逐段检查转写，找出【同音字/近音字写错、专有名词或术语写错、明显的错别字】。

必须按这个顺序判断：
1. 先通读一段，看"这句话读起来通不通"。读起来别扭、讲不通的地方才值得怀疑。
2. 再判断这个别扭是不是【同音字/近音字、术语、错别字】造成的；是才提改动。
3. 最后给这一段一个结论：有问题的写 suspect，没问题的写 ok。

转写里最容易出错的位置，重点看这几类：人称代词（你/您/我/他/她）、数字和量词（一/1/几/两）、
连接词、专有名词。这些位置一旦读不通，基本就是同音字写错。

硬性规则（违反的会被程序丢掉）：
1. 只输出 JSON，不要解释，不要 Markdown 代码块。
2. JSON 结构：
{"segments":[{"segment":片段编号,"verdict":"ok|suspect","edits":[{"original":"原文里连续出现的一段","suggested":"替换成什么","reason":"homophone|term|typo","confidence":"high|medium|low"}]}]}
3. 必须给每一个片段都输出一条记录，一个都不能少；没问题的写 {"segment":编号,"verdict":"ok","edits":[]}。
4. original 必须是该片段里逐字出现的连续文本，必须只出现一次；重复词请带上相邻文字以确定位置，尽量短，不要整句改写。
5. 一段里有几处就列几处，不要只列一处。
6. 同义词、近义词、说法习惯差异【不算错】，一律不许改 —— 换了说法这句话本来也通顺，那就不是错别字。
7. 不要改标点、不要调语序、不要删语气词、不要合并或拆分句子、不要翻译。
8. 不确定就不要提；但"明显读不通"的地方必须提，宁可多提一条也不要漏。
9. 一次只处理给你的这几个片段，segment 必须是片段前面的编号。

示例 A（这段话通顺 → ok）：
输入：[0] (00:00) 他们当中有大学生啊，上班族没有技术背景的，就是靠一些很简单的商业逻辑。
输出：{"segments":[{"segment":0,"verdict":"ok","edits":[]}]}

示例 B（读不通、确实是同音字错 → suspect）：
输入：[1] (00:20) 他们当中有人挣脱了铁链，走到了洞学外面。
输出：{"segments":[{"segment":1,"verdict":"suspect","edits":[{"original":"洞学外面","suggested":"洞穴外面","reason":"homophone","confidence":"high"}]}]}

示例 C（只是换了种说法 → 必须判 ok）：
输入：[2] (00:40) 我们就马上开始吧，先看第一个例子。
输出：{"segments":[{"segment":2,"verdict":"ok","edits":[]}]}"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofreadCandidate {
    pub segment_id: String,
    pub segment_index: usize,
    pub audio_start_time: Option<f64>,
    pub original: String,
    pub suggested: String,
    pub start_char: usize,
    pub end_char: usize,
    pub reason: String,
    pub confidence: String,
    /// 原片段整段文本（前端预览）
    pub segment_text: String,
    /// 采纳这条之后的整段文本
    pub proposed_text: String,
}

#[derive(Debug, Deserialize)]
struct RawEdit {
    #[serde(default)]
    segment: Option<usize>,
    #[serde(default)]
    original: String,
    #[serde(default)]
    suggested: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    confidence: String,
}

/// v2 协议里每个片段的一条结论
#[derive(Debug, Deserialize)]
struct RawSegmentResult {
    #[serde(default)]
    segment: Option<usize>,
    #[serde(default)]
    verdict: String,
    #[serde(default)]
    edits: Vec<RawEdit>,
}

/// v2 协议：{"segments":[{segment,verdict,edits[]}]}；
/// 同时兼容 v1 的 {"edits":[...]}（小模型不一定听话，不能因此丢掉整批结果）
#[derive(Debug, Deserialize)]
struct RawProofreadResult {
    #[serde(default)]
    segments: Vec<RawSegmentResult>,
    #[serde(default)]
    edits: Vec<RawEdit>,
}

/// 一批的解析结果：候选 + 覆盖情况 + 丢弃原因（后者全部进诊断日志）
#[derive(Debug, Default, Serialize)]
pub struct ParsedBatch {
    pub candidates: Vec<ProofreadCandidate>,
    /// 模型给出结论的片段编号（批内下标）
    pub answered: Vec<usize>,
    /// 模型没给结论的片段编号（批内下标）
    pub missing: Vec<usize>,
    /// 被丢弃的候选与原因，例如 not-in-segment / ambiguous-segment / same-as-original
    pub dropped: Vec<String>,
    /// 模型报的片段编号和原文实际所在片段不一致、已自动纠正的次数
    pub reassigned: usize,
}

/// 从模型输出里抠出 JSON（容忍 ```json 代码块和前后废话）
pub fn extract_json_object(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&trimmed[start..=end])
}

pub fn build_batch_user_prompt(batch: &[(usize, &ProofreadRow)], context_line: &str) -> String {
    let mut body = String::new();
    for (index, row) in batch {
        let stamp = row.audio_start_time.unwrap_or(0.0);
        body.push_str(&format!(
            "[{index}] ({:02}:{:02}) {}\n",
            (stamp as i64) / 60,
            (stamp as i64) % 60,
            row.transcript
        ));
    }
    format!(
        "会议上下文参考：{context_line}\n\n\
         请逐段校对下面的转写片段，按 JSON 输出；每个片段都要有一条记录，编号一个都不能少：\n{body}"
    )
}

/// 上一轮没给结论的片段，单独再问一次（提示词更硬、范围更小）
pub fn build_retry_user_prompt(
    retry: &[(usize, &ProofreadRow)],
    context_line: &str,
) -> String {
    let mut body = String::new();
    for (index, row) in retry {
        let stamp = row.audio_start_time.unwrap_or(0.0);
        body.push_str(&format!(
            "[{index}] ({:02}:{:02}) {}\n",
            (stamp as i64) / 60,
            (stamp as i64) % 60,
            row.transcript
        ));
    }
    format!(
        "会议上下文参考：{context_line}\n\n\
         下面这些片段你上一轮漏掉了，请重新逐段检查。\n\
         每个片段都必须输出一条记录（segment = 片段编号，没有问题也要写 verdict=ok），\
         按 JSON 输出，不要漏任何一段：\n{body}"
    )
}

pub fn parse_candidates(
    raw: &str,
    batch: &[(usize, &ProofreadRow)],
    context_line: &str,
) -> ParsedBatch {
    let _ = context_line; // 目前只用于提示词，保留参数便于以后做上下文校验

    let mut result = ParsedBatch {
        missing: batch.iter().map(|(index, _)| *index).collect(),
        ..Default::default()
    };

    let Some(json) = extract_json_object(raw) else {
        warn!("Proofread response has no JSON object: {}", raw);
        result.dropped.push("no-json-object".to_string());
        return result;
    };
    let parsed: RawProofreadResult = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(error) => {
            warn!("Proofread JSON parse failed: {error}; raw={raw}");
            result.dropped.push(format!("json-parse-failed: {error}"));
            return result;
        }
    };

    let mut answered: Vec<usize> = Vec::new();

    let collect = |edit: &RawEdit, claimed: Option<usize>, result: &mut ParsedBatch| {
        let original = edit.original.trim();
        let suggested = edit.suggested.trim();
        if original.is_empty() || suggested.is_empty() || original == suggested {
            result
                .dropped
                .push(format!("empty-or-same: {original:?} -> {suggested:?}"));
            return;
        }

        // 模型常把片段编号写错（实测：把第 3 段的话标成第 4 段）。
        // 与其丢掉这条候选，不如按"这段原文到底在哪个片段里"重新定位；
        // 只有到处都找不到、或者多处都有（可能改错地方）时才丢弃。
        let claimed_row = claimed.and_then(|index| {
            batch
                .iter()
                .find(|(candidate, _)| *candidate == index)
                .map(|(index, row)| (*index, *row))
        });
        let (segment_index, row) = match claimed_row {
            Some((index, row)) if row.transcript.contains(original) => (index, row),
            _ => {
                let matches: Vec<(usize, &ProofreadRow)> = batch
                    .iter()
                    .map(|(index, row)| (*index, *row))
                    .filter(|(_, row)| row.transcript.contains(original))
                    .collect();
                match matches.len() {
                    1 => {
                        result.reassigned += 1;
                        matches[0]
                    }
                    0 => {
                        result.dropped.push(format!(
                            "not-in-batch: {original:?} (claimed segment {claimed:?})"
                        ));
                        return;
                    }
                    _ => {
                        result.dropped.push(format!(
                            "ambiguous-segment: {original:?} 出现在 {} 个片段里",
                            matches.len()
                        ));
                        return;
                    }
                }
            }
        };
        let occurrences: Vec<_> = row.transcript.match_indices(original).collect();
        if occurrences.len() != 1 {
            result.dropped.push(format!("ambiguous-occurrence: {original:?}"));
            return;
        }
        let start_char = row.transcript[..occurrences[0].0].chars().count();
        let end_char = start_char + original.chars().count();
        let proposed_text = apply_text_edits(&row.transcript, &[TextEdit {
            expected_text: &row.transcript, original, suggested, start_char, end_char,
        }]).expect("validated candidate range");
        result.candidates.push(ProofreadCandidate {
            segment_id: row.id.clone(),
            segment_index,
            audio_start_time: row.audio_start_time,
            original: original.to_string(),
            suggested: suggested.to_string(),
            start_char,
            end_char,
            reason: if edit.reason.is_empty() {
                "typo".to_string()
            } else {
                edit.reason.clone()
            },
            confidence: if edit.confidence.is_empty() {
                "medium".to_string()
            } else {
                edit.confidence.clone()
            },
            segment_text: row.transcript.clone(),
            proposed_text,
        });
    };

    for entry in &parsed.segments {
        let valid_verdict = match entry.verdict.as_str() {
            "ok" => entry.edits.is_empty(),
            "suspect" => !entry.edits.is_empty(),
            _ => false,
        };
        if !valid_verdict {
            result.dropped.push(format!("invalid-verdict: segment {:?}", entry.segment));
            continue;
        }
        if entry.verdict == "ok" {
            if let Some(index) = entry.segment {
                answered.push(index);
            }
        }
        for edit in &entry.edits {
            let before = result.candidates.len();
            collect(edit, entry.segment.or(edit.segment), &mut result);
            answered.extend(result.candidates[before..].iter().map(|candidate| candidate.segment_index));
        }
    }
    for edit in &parsed.edits {
        let before = result.candidates.len();
        collect(edit, edit.segment, &mut result);
        answered.extend(result.candidates[before..].iter().map(|candidate| candidate.segment_index));
    }
    answered.sort_unstable();
    answered.dedup();
    answered.retain(|index| batch.iter().any(|(candidate, _)| candidate == index));
    result.missing = batch.iter().map(|(index, _)| *index)
        .filter(|index| !answered.contains(index)).collect();
    result.answered = answered;
    result
}

pub fn build_review_batches(rows: &[ProofreadRow], start_index: usize) -> Vec<Vec<(usize, &ProofreadRow)>> {
    let mut batch: Vec<(usize, &ProofreadRow)> = Vec::new();
    let mut batch_chars = 0usize;
    let mut batches: Vec<Vec<(usize, &ProofreadRow)>> = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(start_index).take(MAX_SEGMENTS_PER_RUN) {
        let text_len = row.transcript.chars().count();
        if !batch.is_empty()
            && (batch.len() >= PROOFREAD_BATCH_SIZE || batch_chars + text_len > MAX_BATCH_CHARS)
        {
            batches.push(std::mem::take(&mut batch));
            batch_chars = 0;
        }
        batch.push((index, row));
        batch_chars += text_len;
    }
    if !batch.is_empty() {
        batches.push(batch);
    }

    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, text: &str, start: f64) -> ProofreadRow {
        ProofreadRow {
            id: id.to_string(),
            transcript: text.to_string(),
            audio_start_time: Some(start),
        }
    }

    #[test]
    fn batching_counts_real_segments_and_resumes_after_limit() {
        for width in [20, 5000] {
            let rows: Vec<_> = (0..100).map(|i| row(&i.to_string(), &"中".repeat(width), i as f64)).collect();
            let first = build_review_batches(&rows, 0);
            assert_eq!(first.iter().map(Vec::len).sum::<usize>(), 60);
            let second = build_review_batches(&rows, 60);
            assert_eq!(second.iter().map(Vec::len).sum::<usize>(), 40);
            assert_eq!(second[0][0].0, 60);
        }
    }

    #[test]
    fn uses_parent_segment_for_repeated_terms() {
        let a = row("seg-a", "他说马上报复", 0.0);
        let b = row("seg-b", "大家想马上报复", 20.0);
        let batch = vec![(0usize, &a), (1usize, &b)];
        let raw = r#"{"segments":[
            {"segment":0,"verdict":"suspect","edits":[{"original":"报复","suggested":"暴富"}]},
            {"segment":1,"verdict":"suspect","edits":[{"original":"报复","suggested":"暴富"}]}
        ]}"#;
        let parsed = parse_candidates(raw, &batch, "");
        assert_eq!(parsed.candidates.len(), 2);
        assert_eq!(parsed.candidates[0].segment_id, "seg-a");
        assert_eq!(parsed.candidates[1].segment_id, "seg-b");
        assert_eq!(parsed.reassigned, 0);
        assert!(parsed.missing.is_empty());
    }

    #[test]
    fn missing_or_invalid_verdict_is_unanswered() {
        let a = row("seg-a", "第一段", 0.0);
        let b = row("seg-b", "第二段", 20.0);
        let batch = vec![(0usize, &a), (1usize, &b)];
        let raw = r#"{"segments":[{"segment":0},{"segment":1,"verdict":"unknown"}]}"#;
        let parsed = parse_candidates(raw, &batch, "");
        assert_eq!(parsed.missing, vec![0, 1]);
        assert!(parsed.answered.is_empty());
    }

    #[test]
    fn contradictory_verdict_does_not_count_as_answered() {
        let a = row("seg-a", "马上报复", 0.0);
        let batch = vec![(0usize, &a)];
        for raw in [
            r#"{"segments":[{"segment":0,"verdict":"suspect","edits":[]}]}"#,
            r#"{"segments":[{"segment":0,"verdict":"ok","edits":[{"original":"报复","suggested":"暴富"}]}]}"#,
        ] {
            assert_eq!(parse_candidates(raw, &batch, "").missing, vec![0]);
        }
    }

    #[test]
    fn extracts_json_from_fenced_output() {
        let raw = "好的，结果如下：\n```json\n{\"edits\":[{\"segment\":0,\"original\":\"洞学里\",\"suggested\":\"洞穴里\"}]}\n```\n以上。";
        let json = extract_json_object(raw).expect("json");
        assert!(json.starts_with('{'));
        assert!(json.ends_with('}'));
    }

    #[test]
    fn keeps_only_edits_whose_original_is_present() {
        let a = row("seg-a", "洞学里的人没有一个人想出去", 0.0);
        let batch = vec![(0usize, &a)];
        let raw = r#"{"edits":[
            {"segment":0,"original":"洞学里","suggested":"洞穴里","reason":"homophone","confidence":"high"},
            {"segment":0,"original":"不存在的片段","suggested":"随便","reason":"typo","confidence":"high"}
        ]}"#;
        let parsed = parse_candidates(raw, &batch, "（无）");
        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.candidates[0].original, "洞学里");
        assert_eq!(parsed.candidates[0].suggested, "洞穴里");
        assert!(parsed.candidates[0].proposed_text.starts_with("洞穴里"));
        assert!(parsed
            .dropped
            .iter()
            .any(|reason| reason.starts_with("not-in-batch")));
    }

    #[test]
    fn reassigns_edit_to_the_segment_that_contains_the_text() {
        // 实测发生过：模型把第 3 段的话标成第 4 段。原文只在一个片段里出现时自动纠正归属，
        // 而不是把候选丢掉（丢掉就是静默漏改）。
        let a = row("seg-a", "年想赚到100万美金", 0.0);
        let b = row("seg-b", "这部影片我也不保证", 20.0);
        let batch = vec![(0usize, &a), (1usize, &b)];
        let raw = r#"{"segments":[
            {"segment":9,"verdict":"suspect","edits":[{"original":"年想赚到","suggested":"你想赚到","reason":"typo","confidence":"high"}]}
        ]}"#;
        let parsed = parse_candidates(raw, &batch, "（无）");
        assert_eq!(parsed.candidates.len(), 1);
        assert_eq!(parsed.candidates[0].segment_index, 0);
        assert_eq!(parsed.candidates[0].segment_id, "seg-a");
        assert_eq!(parsed.reassigned, 1);
    }

    #[test]
    fn drops_edits_that_could_belong_to_two_segments() {
        let a = row("seg-a", "这句话重复出现", 0.0);
        let b = row("seg-b", "这句话重复出现", 20.0);
        let batch = vec![(0usize, &a), (1usize, &b)];
        let raw = r#"{"edits":[{"segment":5,"original":"这句话","suggested":"那句话","reason":"typo","confidence":"high"}]}"#;
        let parsed = parse_candidates(raw, &batch, "（无）");
        assert!(parsed.candidates.is_empty());
        assert!(parsed
            .dropped
            .iter()
            .any(|reason| reason.starts_with("ambiguous-segment")));
    }

    #[test]
    fn drops_edits_that_change_nothing() {
        let a = row("seg-a", "这一句话没有错字", 0.0);
        let batch = vec![(0usize, &a)];
        let raw = r#"{"edits":[{"segment":0,"original":"没有错字","suggested":"没有错字","reason":"typo","confidence":"low"}]}"#;
        assert!(parse_candidates(raw, &batch, "（无）").candidates.is_empty());
    }

    #[test]
    fn tolerates_garbage_output() {
        let a = row("seg-a", "随便一段", 0.0);
        let batch = vec![(0usize, &a)];
        let parsed = parse_candidates("模型今天不想输出 JSON", &batch, "（无）");
        assert!(parsed.candidates.is_empty());
        assert!(parsed.dropped.iter().any(|reason| reason == "no-json-object"));
        // 解析失败时必须把片段标成"没回答"，不能被当成"没问题"
        assert_eq!(parsed.missing, vec![0]);
    }

    #[test]
    fn v2_reports_unanswered_segments() {
        let a = row("seg-a", "第一段的文本", 0.0);
        let b = row("seg-b", "第二段的文本", 20.0);
        let batch = vec![(0usize, &a), (1usize, &b)];
        let raw = r#"{"segments":[
            {"segment":0,"verdict":"ok","edits":[]},
            {"segment":1,"verdict":"suspect","edits":[{"original":"第二段","suggested":"第二断","reason":"typo","confidence":"low"}]}
        ]}"#;
        let parsed = parse_candidates(raw, &batch, "（无）");
        assert!(parsed.missing.is_empty());
        assert_eq!(parsed.answered, vec![0, 1]);
        assert_eq!(parsed.candidates.len(), 1);

        let partial = r#"{"segments":[{"segment":0,"verdict":"ok","edits":[]}]}"#;
        let parsed = parse_candidates(partial, &batch, "（无）");
        assert_eq!(parsed.missing, vec![1]);
    }
}

