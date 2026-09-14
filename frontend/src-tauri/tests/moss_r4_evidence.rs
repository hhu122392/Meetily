use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use std::path::{Path, PathBuf};

use app_lib::database::moss::{SourceTranscriptAnchor, SourceTranscriptAnchorSnapshot};
use app_lib::moss_alignment::{
    align_managed_transcription, ALIGNMENT_METHOD_RAW, ALIGNMENT_METHOD_SOURCE,
    ALIGNMENT_TIME_TOLERANCE_MS, MIN_ANCHOR_MATCH_COVERAGE, MIN_GLOBAL_MATCH_COVERAGE,
    MIN_MATCHED_CHARACTERS_PER_ANCHOR, MIN_RAW_SEGMENT_MATCH_COVERAGE,
};
use app_lib::moss_helper::manager::{ManagedSegment, ManagedTranscription};
use moss_helper::pcm::{
    analyze_activity, ACTIVITY_FRAME_MS, ACTIVITY_THRESHOLD_DBFS, REQUIRED_CHANNELS,
    REQUIRED_SAMPLE_RATE_HZ,
};
use moss_helper::protocol::{
    CompletedMessage, CompletionStatus, NativeSessionLimits, NativeTimings, PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PRODUCT_MAX_AUDIO_MS: i64 = 1_200_000;

#[derive(Debug, Deserialize)]
struct RawCandidate {
    schema_version: Value,
    stage: Option<String>,
    #[serde(alias = "run_state")]
    status: Option<String>,
    #[serde(default)]
    segments: Vec<RawSegment>,
    #[serde(default)]
    global_turns: Vec<RawGlobalTurn>,
}

#[derive(Debug, Deserialize)]
struct RawSegment {
    source_sequence_id: String,
    clip_start_seconds: f64,
    clip_end_seconds: f64,
    text: String,
    model_speaker_label: String,
}

#[derive(Debug, Deserialize)]
struct RawGlobalTurn {
    global_start_ms: i64,
    global_end_ms: i64,
    text: String,
    speaker_label: String,
}

impl RawCandidate {
    fn normalize_segments(&mut self) -> Result<(), Box<dyn Error>> {
        if !self.segments.is_empty() && !self.global_turns.is_empty() {
            return Err(invalid_data("raw candidate contains two segment payloads").into());
        }
        if self.segments.is_empty() {
            self.segments = self
                .global_turns
                .iter()
                .enumerate()
                .map(|(index, segment)| RawSegment {
                    source_sequence_id: format!("R1-{:03}", index + 1),
                    clip_start_seconds: segment.global_start_ms as f64 / 1_000.0,
                    clip_end_seconds: segment.global_end_ms as f64 / 1_000.0,
                    text: segment.text.clone(),
                    model_speaker_label: segment.speaker_label.clone(),
                })
                .collect();
        }
        if self.segments.is_empty() {
            return Err(invalid_data("raw candidate has no segments").into());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct SourceTranscript {
    #[serde(alias = "version")]
    schema_version: Value,
    role: Option<String>,
    run_id: Option<String>,
    source_producer_role: Option<String>,
    model_name: Option<String>,
    backend: Option<String>,
    segments: Vec<SourceSegment>,
}

#[derive(Debug, Deserialize)]
struct SourceSegment {
    #[serde(alias = "audio_start_time")]
    start: f64,
    #[serde(alias = "audio_end_time")]
    end: f64,
    text: String,
}

#[derive(Debug, Clone)]
struct MetricSegment {
    start_ms: i64,
    end_ms: i64,
    speaker_label: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct CoverageMetrics {
    segment_count: usize,
    first_segment_start_ms: Option<i64>,
    last_segment_end_ms: Option<i64>,
    speech_union_ms: i64,
    maximum_internal_gap_ms: i64,
    maximum_segment_duration_ms: i64,
    segments_over_maximum_duration: usize,
    invalid_segment_count: usize,
    nondecreasing_start: bool,
    distinct_speaker_labels: usize,
    normalized_character_count: usize,
}

#[derive(Debug, Serialize)]
struct FullRunGateSubset {
    maximum_first_segment_start_ms: i64,
    minimum_last_segment_end_ms: i64,
    minimum_output_speech_union_ms: i64,
    maximum_internal_gap_ms: i64,
    minimum_distinct_speaker_labels: usize,
    minimum_output_segments: usize,
    minimum_normalized_characters: usize,
    maximum_segment_duration_ms: i64,
    first_segment_start_pass: bool,
    last_segment_end_pass: bool,
    output_speech_union_pass: bool,
    internal_gap_pass: bool,
    distinct_speaker_labels_pass: bool,
    output_segment_count_pass: bool,
    normalized_characters_pass: bool,
    maximum_segment_duration_pass: bool,
    invalid_segment_count_pass: bool,
    objective_subset_pass: bool,
}

#[derive(Debug, Serialize)]
struct BoundarySpill {
    window_start_ms: i64,
    window_end_ms: i64,
    overlapping_segment_count: usize,
    covering_start_ms: Option<i64>,
    covering_end_ms: Option<i64>,
    left_spill_ms: Option<i64>,
    right_spill_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct WavInfo {
    audio_format: u16,
    channels: u16,
    sample_rate_hz: u32,
    bits_per_sample: u16,
    sample_count: usize,
}

#[derive(Debug)]
struct GateThresholds {
    maximum_boundary_spill_ms: i64,
    maximum_first_segment_start_ms: i64,
    minimum_last_segment_end_ms: i64,
    minimum_output_speech_union_ms: i64,
    maximum_internal_gap_ms: i64,
    minimum_distinct_speaker_labels: usize,
    minimum_output_segments: usize,
    minimum_normalized_characters: usize,
    maximum_segment_duration_ms: i64,
}

#[test]
#[ignore = "requires the frozen 737.728-second business meeting evidence"]
fn frozen_real_meeting_product_alignment_evidence() -> Result<(), Box<dyn Error>> {
    let audio_path = required_path("MOSS_R4_AUDIO")?;
    let raw_path = required_path("MOSS_R4_RAW_CANDIDATE")?;
    let source_path = required_path("MOSS_R4_SOURCE_ANCHORS")?;
    let source_lock_path = required_path("MOSS_R4_SOURCE_LOCK")?;
    let r3_manifest_path = required_path("MOSS_R4_R3_MANIFEST")?;
    let rules_path = required_path("MOSS_R4_SCORING_RULES")?;
    let scope_path = required_path("MOSS_R4_HUMAN_SCOPE")?;
    let human_review_path = required_path("MOSS_R4_HUMAN_REVIEW")?;
    let independent_audit_path = required_path("MOSS_R4_INDEPENDENT_AUDIT")?;
    let candidate_output_path = required_path("MOSS_R4_CANDIDATE_OUT")?;
    let evidence_output_path = required_path("MOSS_R4_EVIDENCE_OUT")?;

    let expected_audio_hash = required_hash("MOSS_R4_EXPECTED_AUDIO_SHA256")?;
    let expected_raw_hash = required_hash("MOSS_R4_EXPECTED_RAW_SHA256")?;
    let expected_source_hash = required_hash("MOSS_R4_EXPECTED_SOURCE_SHA256")?;
    let expected_source_lock_hash = required_hash("MOSS_R4_EXPECTED_SOURCE_LOCK_SHA256")?;
    let expected_r3_manifest_hash = required_hash("MOSS_R4_EXPECTED_R3_MANIFEST_SHA256")?;
    let expected_rules_hash = required_hash("MOSS_R4_EXPECTED_RULES_SHA256")?;
    let expected_scope_hash = required_hash("MOSS_R4_EXPECTED_SCOPE_SHA256")?;
    let expected_human_review_hash = required_hash("MOSS_R4_EXPECTED_HUMAN_REVIEW_SHA256")?;
    let expected_independent_audit_hash =
        required_hash("MOSS_R4_EXPECTED_INDEPENDENT_AUDIT_SHA256")?;

    let audio_bytes = fs::read(&audio_path)?;
    let raw_bytes = fs::read(&raw_path)?;
    let source_bytes = fs::read(&source_path)?;
    let source_lock_bytes = fs::read(&source_lock_path)?;
    let r3_manifest_bytes = fs::read(&r3_manifest_path)?;
    let rules_bytes = fs::read(&rules_path)?;
    let scope_bytes = fs::read(&scope_path)?;
    let human_review_bytes = fs::read(&human_review_path)?;
    let independent_audit_bytes = fs::read(&independent_audit_path)?;

    let actual_audio_hash = sha256_bytes(&audio_bytes);
    let actual_raw_hash = sha256_bytes(&raw_bytes);
    let actual_source_hash = sha256_bytes(&source_bytes);
    let actual_source_lock_hash = sha256_bytes(&source_lock_bytes);
    let actual_r3_manifest_hash = sha256_bytes(&r3_manifest_bytes);
    let actual_rules_hash = sha256_bytes(&rules_bytes);
    let actual_scope_hash = sha256_bytes(&scope_bytes);
    let actual_human_review_hash = sha256_bytes(&human_review_bytes);
    let actual_independent_audit_hash = sha256_bytes(&independent_audit_bytes);

    assert_hash("audio", &expected_audio_hash, &actual_audio_hash);
    assert_hash("raw candidate", &expected_raw_hash, &actual_raw_hash);
    assert_hash("source anchors", &expected_source_hash, &actual_source_hash);
    assert_hash(
        "source lock",
        &expected_source_lock_hash,
        &actual_source_lock_hash,
    );
    assert_hash(
        "R3 manifest",
        &expected_r3_manifest_hash,
        &actual_r3_manifest_hash,
    );
    assert_hash("scoring rules", &expected_rules_hash, &actual_rules_hash);
    assert_hash("human scope", &expected_scope_hash, &actual_scope_hash);
    assert_hash(
        "human review",
        &expected_human_review_hash,
        &actual_human_review_hash,
    );
    assert_hash(
        "independent audit",
        &expected_independent_audit_hash,
        &actual_independent_audit_hash,
    );

    let mut raw: RawCandidate = serde_json::from_slice(&raw_bytes)?;
    raw.normalize_segments()?;
    let source: SourceTranscript = serde_json::from_slice(&source_bytes)?;
    let source_lock: Value = serde_json::from_slice(&source_lock_bytes)?;
    let r3_manifest: Value = serde_json::from_slice(&r3_manifest_bytes)?;
    let rules: Value = serde_json::from_slice(&rules_bytes)?;
    let scope: Value = serde_json::from_slice(&scope_bytes)?;
    let human_review: Value = serde_json::from_slice(&human_review_bytes)?;
    let independent_audit: Value = serde_json::from_slice(&independent_audit_bytes)?;
    let thresholds = gate_thresholds(&rules)?;

    assert_eq!(raw.stage.as_deref(), Some("R1_MONOLITHIC_VULKAN16K"));
    assert_eq!(raw.status.as_deref(), Some("COMPLETED"));
    assert_eq!(
        raw.segments.len(),
        67,
        "R4 must use the R3 single-session result"
    );
    assert_eq!(
        source_lock
            .pointer("/source_transcript/sha256")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase),
        Some(actual_source_hash.to_ascii_lowercase()),
        "the product source transcript is not bound by the frozen source lock"
    );
    let r3_raw_entry = r3_manifest
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries.iter().find(|entry| {
                entry.get("role").and_then(Value::as_str) == Some("moss_p1_private_output")
            })
        })
        .ok_or_else(|| invalid_data("R3 manifest lacks the monolithic MOSS output"))?;
    assert_eq!(
        r3_raw_entry
            .get("sha256")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase),
        Some(actual_raw_hash.to_ascii_lowercase()),
        "the raw candidate is not the R3 manifest-bound single-session output"
    );

    assert_eq!(
        human_review
            .get("approved_as_ground_truth")
            .and_then(Value::as_bool),
        Some(true),
        "the hash-bound human review is not approved"
    );
    assert_eq!(
        human_review
            .pointer("/playback_coverage/complete")
            .and_then(Value::as_bool),
        Some(true),
        "the hash-bound human review lacks complete playback coverage"
    );
    assert_eq!(
        independent_audit
            .get("structural_status")
            .and_then(Value::as_str),
        Some("PASS"),
        "the independent truth audit did not pass"
    );

    let window_start_ms = seconds_value_to_ms(
        scope
            .pointer("/reference_window/source_start_seconds")
            .and_then(Value::as_f64)
            .ok_or_else(|| invalid_data("missing human window start"))?,
    )?;
    let window_end_ms = seconds_value_to_ms(
        scope
            .pointer("/reference_window/source_end_seconds")
            .and_then(Value::as_f64)
            .ok_or_else(|| invalid_data("missing human window end"))?,
    )?;
    let scope_audio_hash = scope
        .pointer("/source/sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_data("missing scope audio hash"))?;
    assert_hash("scope-bound audio", scope_audio_hash, &actual_audio_hash);

    let (samples, wav_info) = pcm16_mono_wav(&audio_bytes)?;
    let activity = analyze_activity(&samples)?;
    assert_eq!(wav_info.channels, REQUIRED_CHANNELS);
    assert_eq!(wav_info.sample_rate_hz, REQUIRED_SAMPLE_RATE_HZ);
    assert_eq!(wav_info.bits_per_sample, 16);
    assert_eq!(activity.frame_ms, ACTIVITY_FRAME_MS);
    assert_eq!(activity.threshold_dbfs, ACTIVITY_THRESHOLD_DBFS);

    let mut speaker_ids = BTreeMap::<String, i32>::new();
    let mut managed_segments = Vec::with_capacity(raw.segments.len());
    for (index, segment) in raw.segments.iter().enumerate() {
        let next_speaker_id = i32::try_from(speaker_ids.len())?;
        let speaker_id = *speaker_ids
            .entry(segment.model_speaker_label.clone())
            .or_insert(next_speaker_id);
        managed_segments.push(ManagedSegment {
            segment_index: u32::try_from(index)?,
            t0_ms: seconds_value_to_ms(segment.clip_start_seconds)?,
            t1_ms: seconds_value_to_ms(segment.clip_end_seconds)?,
            speaker_id,
            text: segment.text.clone(),
        });
    }
    let raw_text = managed_segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let raw_text_sha256 = sha256_bytes(raw_text.as_bytes());
    let model_last_timestamp_ms = managed_segments
        .iter()
        .map(|segment| segment.t1_ms)
        .max()
        .ok_or_else(|| invalid_data("raw candidate has no segments"))?;
    let request_id = "frozen-r4-product-alignment-20260831".to_owned();
    let context_sha256 = "0".repeat(64);
    let managed = ManagedTranscription {
        request_id: request_id.clone(),
        context_sha256: context_sha256.clone(),
        raw_text: raw_text.clone(),
        clean_text: raw_text.clone(),
        segments: managed_segments.clone(),
        completed: CompletedMessage {
            v: PROTOCOL_VERSION,
            seq: 1,
            request_id,
            context_sha256,
            terminal: true,
            status: CompletionStatus::Ok,
            backend: "frozen_evidence_import".to_owned(),
            device_description: "existing hash-bound MOSS full candidate".to_owned(),
            native_run_elapsed_ms: 0,
            native_rtf: 0.0,
            wall_elapsed_ms: 0,
            wall_rtf: 0.0,
            last_timestamp_ms: model_last_timestamp_ms,
            segment_count: u32::try_from(managed_segments.len())?,
            raw_text_sha256: raw_text_sha256.clone(),
            clean_text_sha256: raw_text_sha256.clone(),
            was_aborted: false,
            was_truncated: false,
            native_session_limits: NativeSessionLimits {
                effective_n_ctx: 32_768,
                effective_max_audio_ms: PRODUCT_MAX_AUDIO_MS,
                max_kv_bytes: 1_879_048_192,
            },
            native_timings: NativeTimings {
                load_ms: 0.0,
                mel_ms: 0.0,
                encode_ms: 0.0,
                decode_ms: 0.0,
            },
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        },
        heartbeat_count: 0,
        supervisor_wall_elapsed_ms: 0,
        supervisor_wall_rtf: 0.0,
        helper_process_id: 0,
        helper_total_processes: 0,
        helper_peak_job_memory_bytes: 0,
        residual_process_count: 0,
        helper_runs: Vec::new(),
        audio_activity: Some(activity),
        language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
        language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
        decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
        decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
    };

    let mut invalid_timed_rows = 0u32;
    let mut anchors = Vec::new();
    for (index, segment) in source.segments.iter().enumerate() {
        let start_ms = seconds_value_to_ms(segment.start)?;
        let end_ms = seconds_value_to_ms(segment.end)?;
        if start_ms < 0 || end_ms <= start_ms || segment.text.trim().is_empty() {
            invalid_timed_rows = invalid_timed_rows.saturating_add(1);
            continue;
        }
        anchors.push(SourceTranscriptAnchor {
            anchor_id: format!("formal-whisper-{index:04}"),
            start_ms,
            end_ms,
            text: segment.text.clone(),
        });
    }
    let snapshot = SourceTranscriptAnchorSnapshot {
        expected_sha256: expected_source_hash.clone(),
        actual_sha256: actual_source_hash.clone(),
        hash_verified: expected_source_hash.eq_ignore_ascii_case(&actual_source_hash),
        invalid_timed_rows,
        anchors,
    };
    let aligned = align_managed_transcription(&managed, &snapshot)?;

    let output_text = aligned
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let output_text_sha256 = sha256_bytes(output_text.as_bytes());
    let text_exact = output_text == raw_text;
    let output_starts_monotonic = aligned
        .segments
        .windows(2)
        .all(|pair| pair[0].start_ms <= pair[1].start_ms);
    let provenance_count_exact = aligned.segments.len() == aligned.provenance.len();
    let output_indices_exact = aligned
        .segments
        .iter()
        .enumerate()
        .all(|(index, segment)| usize::try_from(segment.segment_index).ok() == Some(index));

    let anchor_by_id = snapshot
        .anchors
        .iter()
        .map(|anchor| (anchor.anchor_id.as_str(), (anchor.start_ms, anchor.end_ms)))
        .collect::<HashMap<_, _>>();
    let raw_by_index = managed_segments
        .iter()
        .map(|segment| (segment.segment_index, segment))
        .collect::<HashMap<_, _>>();
    let mut provenance_timestamps_valid = true;
    let mut provenance_raw_hashes_valid = true;
    let mut source_anchor_ids_valid = true;
    let mut raw_piece_counts = BTreeMap::<u32, usize>::new();
    let mut output_indices_by_raw = BTreeMap::<u32, Vec<usize>>::new();
    let mut aligned_raw_indices = BTreeSet::<u32>::new();
    let mut fallback_raw_indices = BTreeSet::<u32>::new();
    let mut output_evidence = Vec::with_capacity(aligned.segments.len());
    for (output_index, (output, provenance)) in
        aligned.segments.iter().zip(&aligned.provenance).enumerate()
    {
        *raw_piece_counts
            .entry(provenance.raw_segment_index)
            .or_default() += 1;
        output_indices_by_raw
            .entry(provenance.raw_segment_index)
            .or_default()
            .push(output_index);
        let Some(raw_segment) = raw_by_index.get(&provenance.raw_segment_index) else {
            provenance_timestamps_valid = false;
            provenance_raw_hashes_valid = false;
            continue;
        };
        let expected_raw_text_sha = sha256_bytes(raw_segment.text.as_bytes());
        provenance_raw_hashes_valid &=
            expected_raw_text_sha.eq_ignore_ascii_case(&provenance.raw_text_sha256);
        match provenance.alignment_method.as_str() {
            ALIGNMENT_METHOD_SOURCE => {
                aligned_raw_indices.insert(provenance.raw_segment_index);
                if provenance.source_anchor_ids.len() != 1 {
                    source_anchor_ids_valid = false;
                    provenance_timestamps_valid = false;
                } else if anchor_by_id
                    .get(provenance.source_anchor_ids[0].as_str())
                    .is_some()
                {
                    provenance_timestamps_valid &= output.start_ms >= raw_segment.t0_ms
                        && output.end_ms <= raw_segment.t1_ms
                        && output.end_ms > output.start_ms;
                } else {
                    source_anchor_ids_valid = false;
                    provenance_timestamps_valid = false;
                }
            }
            ALIGNMENT_METHOD_RAW => {
                fallback_raw_indices.insert(provenance.raw_segment_index);
                source_anchor_ids_valid &= provenance.source_anchor_ids.is_empty();
                provenance_timestamps_valid &=
                    (output.start_ms, output.end_ms) == (raw_segment.t0_ms, raw_segment.t1_ms);
            }
            _ => {
                source_anchor_ids_valid = false;
                provenance_timestamps_valid = false;
            }
        }
        output_evidence.push(json!({
            "output_segment_index": output.segment_index,
            "output_start_ms": output.start_ms,
            "output_end_ms": output.end_ms,
            "output_text_sha256": sha256_bytes(output.text.as_bytes()),
            "candidate_speaker_label": output.speaker_label,
            "raw_segment_index": provenance.raw_segment_index,
            "raw_source_sequence_id": raw.segments[usize::try_from(provenance.raw_segment_index)?].source_sequence_id,
            "raw_model_speaker_label": raw.segments[usize::try_from(provenance.raw_segment_index)?].model_speaker_label,
            "raw_start_ms": provenance.raw_start_ms,
            "raw_end_ms": provenance.raw_end_ms,
            "raw_text_sha256": provenance.raw_text_sha256,
            "alignment_method": provenance.alignment_method,
            "confidence": provenance.confidence,
            "source_anchor_ids": provenance.source_anchor_ids,
            "timestamp_derivation": if provenance.alignment_method == ALIGNMENT_METHOD_SOURCE {
                "RAW_MOSS_RANGE_PARTITIONED_AT_SOURCE_ANCHOR_BOUNDARIES"
            } else {
                "RAW_MOSS_SEGMENT"
            },
        }));
    }

    for (raw_index, output_indices) in &output_indices_by_raw {
        let Some(raw_segment) = raw_by_index.get(raw_index) else {
            provenance_timestamps_valid = false;
            continue;
        };
        let first_index = output_indices[0];
        let last_index = output_indices[output_indices.len() - 1];
        let group_outputs = output_indices
            .iter()
            .map(|index| &aligned.segments[*index])
            .collect::<Vec<_>>();
        let group_provenance = output_indices
            .iter()
            .map(|index| &aligned.provenance[*index])
            .collect::<Vec<_>>();
        let source_partition = group_provenance
            .iter()
            .all(|value| value.alignment_method == ALIGNMENT_METHOD_SOURCE);
        let raw_fallback = group_provenance
            .iter()
            .all(|value| value.alignment_method == ALIGNMENT_METHOD_RAW);
        if source_partition {
            provenance_timestamps_valid &= aligned.segments[first_index].start_ms
                == raw_segment.t0_ms
                && aligned.segments[last_index].end_ms == raw_segment.t1_ms
                && group_outputs
                    .windows(2)
                    .all(|pair| pair[0].end_ms == pair[1].start_ms);
            for (piece_index, provenance) in group_provenance.iter().enumerate().skip(1) {
                let Some(anchor_id) = provenance.source_anchor_ids.first() else {
                    provenance_timestamps_valid = false;
                    continue;
                };
                let Some(anchor_times) = anchor_by_id.get(anchor_id.as_str()) else {
                    provenance_timestamps_valid = false;
                    continue;
                };
                let expected_boundary = anchor_times.0.clamp(raw_segment.t0_ms, raw_segment.t1_ms);
                provenance_timestamps_valid &=
                    group_outputs[piece_index].start_ms == expected_boundary;
            }
        } else if raw_fallback {
            provenance_timestamps_valid &= output_indices.len() == 1
                && (
                    aligned.segments[first_index].start_ms,
                    aligned.segments[first_index].end_ms,
                ) == (raw_segment.t0_ms, raw_segment.t1_ms);
        } else {
            provenance_timestamps_valid = false;
        }
    }

    assert!(text_exact, "alignment changed the MOSS text");
    assert!(output_starts_monotonic, "output starts are not monotonic");
    assert!(
        provenance_count_exact,
        "provenance count does not match output"
    );
    assert!(output_indices_exact, "output segment indices are not exact");
    assert!(
        provenance_timestamps_valid,
        "timestamp provenance is invalid"
    );
    assert!(
        provenance_raw_hashes_valid,
        "raw text provenance hash is invalid"
    );
    assert!(
        source_anchor_ids_valid,
        "source anchor provenance is invalid"
    );
    assert!(aligned.source_hash_verified, "source hash was not verified");
    assert_eq!(
        usize::try_from(aligned.aligned_segment_count + aligned.fallback_segment_count)?,
        aligned.segments.len(),
        "alignment counts do not add up to output count"
    );

    let raw_metric_segments = managed_segments
        .iter()
        .map(|segment| MetricSegment {
            start_ms: segment.t0_ms,
            end_ms: segment.t1_ms,
            speaker_label: format!("S{:02}", segment.speaker_id + 1),
            text: segment.text.clone(),
        })
        .collect::<Vec<_>>();
    let output_metric_segments = aligned
        .segments
        .iter()
        .map(|segment| MetricSegment {
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            speaker_label: segment.speaker_label.clone(),
            text: segment.text.clone(),
        })
        .collect::<Vec<_>>();
    let raw_metrics =
        coverage_metrics(&raw_metric_segments, thresholds.maximum_segment_duration_ms);
    let output_metrics = coverage_metrics(
        &output_metric_segments,
        thresholds.maximum_segment_duration_ms,
    );
    let raw_gate_subset = gate_subset(&raw_metrics, &thresholds);
    let output_gate_subset = gate_subset(&output_metrics, &thresholds);
    let raw_boundary = boundary_spill(&raw_metric_segments, window_start_ms, window_end_ms);
    let output_boundary = boundary_spill(&output_metric_segments, window_start_ms, window_end_ms);
    let raw_boundary_pass = spill_pass(&raw_boundary, thresholds.maximum_boundary_spill_ms);
    let output_boundary_pass = spill_pass(&output_boundary, thresholds.maximum_boundary_spill_ms);

    let candidate_segments = aligned
        .segments
        .iter()
        .zip(&aligned.provenance)
        .map(|(segment, provenance)| {
            json!({
                "source_sequence_id": format!("R4-{:03}", segment.segment_index + 1),
                "clip_start_seconds": segment.start_ms as f64 / 1_000.0,
                "clip_end_seconds": segment.end_ms as f64 / 1_000.0,
                "text": segment.text,
                "model_speaker_label": segment.speaker_label,
                "speaker_id": segment.speaker_label,
                "speaker_identity_scope": "candidate_only",
                "human_checked": false,
                "is_ground_truth": false,
                "alignment": {
                    "method": provenance.alignment_method,
                    "confidence": provenance.confidence,
                    "source_anchor_ids": provenance.source_anchor_ids,
                    "raw_segment_index": provenance.raw_segment_index,
                    "raw_start_ms": provenance.raw_start_ms,
                    "raw_end_ms": provenance.raw_end_ms,
                    "raw_text_sha256": provenance.raw_text_sha256,
                }
            })
        })
        .collect::<Vec<_>>();
    let candidate_document = json!({
        "schema_version": 1,
        "stage": "MOSS-R4-PRODUCT-ALIGNED-CANDIDATE",
        "status": "MACHINE_CANDIDATE_NOT_GROUND_TRUTH",
        "source_raw_candidate_sha256": actual_raw_hash.clone(),
        "source_transcript_sha256": actual_source_hash.clone(),
        "source_transcript_role": "SECONDARY_MACHINE_ANCHOR_NOT_HUMAN_TRUTH",
        "text_preservation": "EXACT",
        "turn_count": candidate_segments.len(),
        "segments": candidate_segments,
    });
    let candidate_bytes = serde_json::to_vec_pretty(&candidate_document)?;
    write_evidence(&candidate_output_path, &candidate_bytes)?;
    let candidate_sha256 = sha256_bytes(&candidate_bytes);

    let model_minus_last_active_ms = activity
        .last_active_ms
        .and_then(|value| i64::try_from(value).ok())
        .map(|value| model_last_timestamp_ms - value);
    let audio_duration_minus_model_ms =
        i64::try_from(activity.audio_duration_ms)? - model_last_timestamp_ms;
    let split_raw_segment_count = raw_piece_counts
        .values()
        .filter(|piece_count| **piece_count > 1)
        .count();
    let product_alignment_exercised = aligned.aligned_segment_count > 0;
    let structural_pass = text_exact
        && output_starts_monotonic
        && provenance_count_exact
        && output_indices_exact
        && provenance_timestamps_valid
        && provenance_raw_hashes_valid
        && source_anchor_ids_valid
        && aligned.source_hash_verified
        && product_alignment_exercised;

    let report = json!({
        "schema_version": 1,
        "stage": "MOSS-R4-FROZEN-REAL-MEETING-PRODUCT-ALIGNMENT",
        "generated_by": "cargo test --test moss_r4_evidence -- --ignored",
        "inputs": {
            "audio": {
                "path": audio_path.display().to_string(),
                "expected_sha256": expected_audio_hash,
                "actual_sha256": actual_audio_hash,
                "hash_verified": true,
                "wav": wav_info,
            },
            "raw_moss_candidate": {
                "path": raw_path.display().to_string(),
                "schema_version": raw.schema_version,
                "stage": raw.stage,
                "status": raw.status,
                "expected_sha256": expected_raw_hash,
                "actual_sha256": actual_raw_hash,
                "hash_verified": true,
                "role": "FROZEN_RAW_MACHINE_CANDIDATE_NOT_HUMAN_TRUTH",
            },
            "source_transcript_anchors": {
                "path": source_path.display().to_string(),
                "schema_version": source.schema_version,
                "role_from_file": source.role,
                "run_id": source.run_id,
                "source_producer_role": source.source_producer_role,
                "model_name": source.model_name,
                "backend": source.backend,
                "expected_sha256": expected_source_hash,
                "actual_sha256": actual_source_hash,
                "hash_verified": aligned.source_hash_verified,
                "role_in_r4": "SECONDARY_MACHINE_ANCHOR_NOT_HUMAN_TRUTH",
                "invalid_timed_rows": invalid_timed_rows,
            },
            "source_transcript_lock": {
                "path": source_lock_path.display().to_string(),
                "expected_sha256": expected_source_lock_hash,
                "actual_sha256": actual_source_lock_hash,
                "source_transcript_sha256_from_lock": source_lock.pointer("/source_transcript/sha256"),
                "hash_verified": true,
            },
            "r3_single_session_manifest": {
                "path": r3_manifest_path.display().to_string(),
                "expected_sha256": expected_r3_manifest_hash,
                "actual_sha256": actual_r3_manifest_hash,
                "raw_candidate_sha256_from_manifest": r3_raw_entry.get("sha256"),
                "hash_verified": true,
            },
            "frozen_scoring_rules": {
                "path": rules_path.display().to_string(),
                "expected_sha256": expected_rules_hash,
                "actual_sha256": actual_rules_hash,
                "status_from_file": rules.get("status"),
                "overall_rule_from_file": rules.get("overall_rule"),
            },
            "formal_human_truth_structure": {
                "scope_path": scope_path.display().to_string(),
                "scope_sha256": actual_scope_hash,
                "human_review_path": human_review_path.display().to_string(),
                "human_review_sha256": actual_human_review_hash,
                "reviewer_id": human_review.get("reviewer_id"),
                "reviewer_role": human_review.get("reviewer_role"),
                "approved_as_ground_truth": true,
                "playback_complete": true,
                "independent_audit_path": independent_audit_path.display().to_string(),
                "independent_audit_sha256": actual_independent_audit_hash,
                "independent_structural_status": "PASS",
                "use_in_this_alignment": "BOUNDARY_AND_LATER_SCORING_ONLY_NOT_AS_MACHINE_ALIGNMENT_ANCHOR",
            },
        },
        "product_constants": {
            "alignment_time_tolerance_ms": ALIGNMENT_TIME_TOLERANCE_MS,
            "minimum_global_match_coverage": MIN_GLOBAL_MATCH_COVERAGE,
            "minimum_anchor_match_coverage": MIN_ANCHOR_MATCH_COVERAGE,
            "minimum_raw_segment_match_coverage": MIN_RAW_SEGMENT_MATCH_COVERAGE,
            "minimum_matched_characters_per_anchor": MIN_MATCHED_CHARACTERS_PER_ANCHOR,
            "source_timestamp_derivation": "RAW_MOSS_RANGE_PARTITIONED_AT_SOURCE_ANCHOR_BOUNDARIES",
            "activity_frame_ms": ACTIVITY_FRAME_MS,
            "activity_threshold_dbfs": ACTIVITY_THRESHOLD_DBFS,
        },
        "alignment_result": {
            "raw_segment_count": managed_segments.len(),
            "output_segment_count": aligned.segments.len(),
            "aligned_output_segment_count": aligned.aligned_segment_count,
            "fallback_output_segment_count": aligned.fallback_segment_count,
            "aligned_raw_segment_count": aligned_raw_indices.len(),
            "fallback_raw_segment_count": fallback_raw_indices.len(),
            "split_raw_segment_count": split_raw_segment_count,
            "source_anchor_count": aligned.source_anchor_count,
            "fallback_reason": aligned.fallback_reason,
            "product_alignment_exercised": product_alignment_exercised,
            "candidate_output_path": candidate_output_path.display().to_string(),
            "candidate_sha256": candidate_sha256,
        },
        "invariants": {
            "raw_concatenated_text_sha256": raw_text_sha256,
            "output_concatenated_text_sha256": output_text_sha256,
            "text_preserved_exactly": text_exact,
            "output_starts_monotonic": output_starts_monotonic,
            "provenance_count_exact": provenance_count_exact,
            "output_indices_exact": output_indices_exact,
            "provenance_timestamps_valid": provenance_timestamps_valid,
            "provenance_raw_hashes_valid": provenance_raw_hashes_valid,
            "source_anchor_ids_valid": source_anchor_ids_valid,
            "source_hash_verified": aligned.source_hash_verified,
        },
        "objective_audio_timing": {
            "audio_duration_ms": activity.audio_duration_ms,
            "first_active_ms": activity.first_active_ms,
            "last_active_ms": activity.last_active_ms,
            "raw_model_last_timestamp_ms": model_last_timestamp_ms,
            "model_minus_last_active_ms": model_minus_last_active_ms,
            "audio_duration_minus_model_ms": audio_duration_minus_model_ms,
            "meaning": "model_minus_last_active_ms follows the persisted product sign; a negative value means the model ends before the last objectively active frame",
        },
        "full_run_objective_subset": {
            "raw_candidate": {
                "metrics": raw_metrics,
                "gate": raw_gate_subset,
            },
            "product_aligned_candidate": {
                "metrics": output_metrics,
                "gate": output_gate_subset,
            },
        },
        "formal_human_window_boundary": {
            "maximum_allowed_spill_ms": thresholds.maximum_boundary_spill_ms,
            "raw_candidate": {
                "metrics": raw_boundary,
                "pass": raw_boundary_pass,
            },
            "product_aligned_candidate": {
                "metrics": output_boundary,
                "pass": output_boundary_pass,
            },
        },
        "per_output_provenance": output_evidence,
        "decision": {
            "r4_structural_evidence": if structural_pass { "PASS" } else { "FAIL" },
            "formal_cer": "NOT_RUN",
            "formal_speaker_error": "NOT_RUN",
            "formal_term_accuracy": "NOT_RUN",
            "formal_accuracy_release": "NO-GO_UNTIL_HUMAN_TRUTH_SCORING_AND_ALL_FAIL_CLOSED_GATES_PASS",
            "reason": "This run exercises the production alignment and objective audio timing on hash-bound real evidence. The secondary source transcript is machine output and is not a human accuracy reference.",
        },
    });
    let report_bytes = serde_json::to_vec_pretty(&report)?;
    write_evidence(&evidence_output_path, &report_bytes)?;
    println!("R4_EVIDENCE={}", evidence_output_path.display());
    println!("R4_CANDIDATE={}", candidate_output_path.display());
    println!("R4_STRUCTURAL_PASS={structural_pass}");
    println!(
        "R4_ALIGNED_OUTPUT_SEGMENTS={}",
        aligned.aligned_segment_count
    );
    println!(
        "R4_FALLBACK_OUTPUT_SEGMENTS={}",
        aligned.fallback_segment_count
    );
    Ok(())
}

fn required_path(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let value = std::env::var(name)
        .map_err(|_| invalid_data(format!("required environment variable {name} is missing")))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() || !path.is_file() && !name.ends_with("_OUT") {
        return Err(invalid_data(format!("{name} is not a valid absolute file path")).into());
    }
    Ok(path)
}

fn required_hash(name: &str) -> Result<String, Box<dyn Error>> {
    let value = std::env::var(name)
        .map_err(|_| invalid_data(format!("required environment variable {name} is missing")))?;
    if value.len() != 64 || !value.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(invalid_data(format!("{name} is not a SHA-256 value")).into());
    }
    Ok(value.to_ascii_lowercase())
}

fn assert_hash(label: &str, expected: &str, actual: &str) {
    assert!(
        expected.eq_ignore_ascii_case(actual),
        "{label} hash mismatch: expected {expected}, actual {actual}"
    );
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn seconds_value_to_ms(value: f64) -> Result<i64, Box<dyn Error>> {
    if !value.is_finite() || value < 0.0 || value > i64::MAX as f64 / 1_000.0 {
        return Err(invalid_data("invalid seconds value").into());
    }
    Ok((value * 1_000.0).round() as i64)
}

fn invalid_data(message: impl Into<String>) -> IoError {
    IoError::new(ErrorKind::InvalidData, message.into())
}

fn u16_le(bytes: &[u8], offset: usize) -> Result<u16, Box<dyn Error>> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(|| invalid_data("truncated u16"))?
            .try_into()?,
    ))
}

fn u32_le(bytes: &[u8], offset: usize) -> Result<u32, Box<dyn Error>> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid_data("truncated u32"))?
            .try_into()?,
    ))
}

fn pcm16_mono_wav(bytes: &[u8]) -> Result<(Vec<f32>, WavInfo), Box<dyn Error>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid_data("audio is not a RIFF/WAVE file").into());
    }
    let mut offset = 12usize;
    let mut format = None;
    let mut data = None;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = usize::try_from(u32_le(bytes, offset + 4)?)?;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start
            .checked_add(chunk_size)
            .ok_or_else(|| invalid_data("WAV chunk size overflow"))?;
        if chunk_end > bytes.len() {
            return Err(invalid_data("truncated WAV chunk").into());
        }
        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err(invalid_data("invalid WAV fmt chunk").into());
            }
            format = Some((
                u16_le(bytes, chunk_start)?,
                u16_le(bytes, chunk_start + 2)?,
                u32_le(bytes, chunk_start + 4)?,
                u16_le(bytes, chunk_start + 14)?,
            ));
        } else if chunk_id == b"data" {
            data = Some(&bytes[chunk_start..chunk_end]);
        }
        offset = chunk_end
            .checked_add(chunk_size % 2)
            .ok_or_else(|| invalid_data("WAV padding overflow"))?;
    }
    let (audio_format, channels, sample_rate_hz, bits_per_sample) =
        format.ok_or_else(|| invalid_data("WAV fmt chunk is missing"))?;
    let data = data.ok_or_else(|| invalid_data("WAV data chunk is missing"))?;
    if audio_format != 1
        || channels != REQUIRED_CHANNELS
        || sample_rate_hz != REQUIRED_SAMPLE_RATE_HZ
        || bits_per_sample != 16
        || data.is_empty()
        || data.len() % 2 != 0
    {
        return Err(invalid_data("WAV must be PCM16 mono 16 kHz").into());
    }
    let mut samples = Vec::with_capacity(data.len() / 2);
    for bytes in data.chunks_exact(2) {
        samples.push(i16::from_le_bytes(bytes.try_into()?) as f32 / 32_768.0);
    }
    let info = WavInfo {
        audio_format,
        channels,
        sample_rate_hz,
        bits_per_sample,
        sample_count: samples.len(),
    };
    Ok((samples, info))
}

fn gate_thresholds(rules: &Value) -> Result<GateThresholds, Box<dyn Error>> {
    Ok(GateThresholds {
        maximum_boundary_spill_ms: seconds_value_to_ms(required_f64(
            rules,
            "/window_derivation/maximum_boundary_spill_seconds",
        )?)?,
        maximum_first_segment_start_ms: seconds_value_to_ms(required_f64(
            rules,
            "/full_run_gates/maximum_first_segment_start_seconds",
        )?)?,
        minimum_last_segment_end_ms: seconds_value_to_ms(required_f64(
            rules,
            "/full_run_gates/minimum_last_segment_end_seconds",
        )?)?,
        minimum_output_speech_union_ms: seconds_value_to_ms(required_f64(
            rules,
            "/full_run_gates/minimum_output_speech_union_seconds",
        )?)?,
        maximum_internal_gap_ms: seconds_value_to_ms(required_f64(
            rules,
            "/full_run_gates/maximum_internal_gap_seconds",
        )?)?,
        minimum_distinct_speaker_labels: required_usize(
            rules,
            "/full_run_gates/minimum_distinct_speaker_labels",
        )?,
        minimum_output_segments: required_usize(rules, "/full_run_gates/minimum_output_segments")?,
        minimum_normalized_characters: required_usize(
            rules,
            "/full_run_gates/minimum_normalized_characters",
        )?,
        maximum_segment_duration_ms: seconds_value_to_ms(required_f64(
            rules,
            "/full_run_gates/maximum_segment_duration_seconds",
        )?)?,
    })
}

fn required_f64(value: &Value, pointer: &str) -> Result<f64, Box<dyn Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_f64)
        .ok_or_else(|| invalid_data(format!("missing numeric rule {pointer}")).into())
}

fn required_usize(value: &Value, pointer: &str) -> Result<usize, Box<dyn Error>> {
    let value = value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_data(format!("missing integer rule {pointer}")))?;
    Ok(usize::try_from(value)?)
}

fn coverage_metrics(segments: &[MetricSegment], maximum_duration_ms: i64) -> CoverageMetrics {
    let invalid_segment_count = segments
        .iter()
        .filter(|segment| segment.start_ms < 0 || segment.end_ms <= segment.start_ms)
        .count();
    let nondecreasing_start = segments
        .windows(2)
        .all(|pair| pair[0].start_ms <= pair[1].start_ms);
    let maximum_segment_duration_ms = segments
        .iter()
        .map(|segment| segment.end_ms.saturating_sub(segment.start_ms))
        .max()
        .unwrap_or(0);
    let segments_over_maximum_duration = segments
        .iter()
        .filter(|segment| segment.end_ms.saturating_sub(segment.start_ms) > maximum_duration_ms)
        .count();
    let distinct_speaker_labels = segments
        .iter()
        .map(|segment| segment.speaker_label.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let normalized_character_count = segments
        .iter()
        .flat_map(|segment| segment.text.chars())
        .filter(|value| value.is_alphanumeric())
        .count();
    let mut intervals = segments
        .iter()
        .filter(|segment| segment.end_ms > segment.start_ms)
        .map(|segment| (segment.start_ms, segment.end_ms))
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let first_segment_start_ms = intervals.first().map(|value| value.0);
    let last_segment_end_ms = intervals.iter().map(|value| value.1).max();
    let mut speech_union_ms = 0i64;
    let mut maximum_internal_gap_ms = 0i64;
    if let Some(&(mut current_start, mut current_end)) = intervals.first() {
        for &(start, end) in intervals.iter().skip(1) {
            if start > current_end {
                speech_union_ms =
                    speech_union_ms.saturating_add(current_end.saturating_sub(current_start));
                maximum_internal_gap_ms =
                    maximum_internal_gap_ms.max(start.saturating_sub(current_end));
                current_start = start;
                current_end = end;
            } else {
                current_end = current_end.max(end);
            }
        }
        speech_union_ms = speech_union_ms.saturating_add(current_end.saturating_sub(current_start));
    }
    CoverageMetrics {
        segment_count: segments.len(),
        first_segment_start_ms,
        last_segment_end_ms,
        speech_union_ms,
        maximum_internal_gap_ms,
        maximum_segment_duration_ms,
        segments_over_maximum_duration,
        invalid_segment_count,
        nondecreasing_start,
        distinct_speaker_labels,
        normalized_character_count,
    }
}

fn gate_subset(metrics: &CoverageMetrics, rules: &GateThresholds) -> FullRunGateSubset {
    let first_segment_start_pass = metrics
        .first_segment_start_ms
        .is_some_and(|value| value <= rules.maximum_first_segment_start_ms);
    let last_segment_end_pass = metrics
        .last_segment_end_ms
        .is_some_and(|value| value >= rules.minimum_last_segment_end_ms);
    let output_speech_union_pass = metrics.speech_union_ms >= rules.minimum_output_speech_union_ms;
    let internal_gap_pass = metrics.maximum_internal_gap_ms <= rules.maximum_internal_gap_ms;
    let distinct_speaker_labels_pass =
        metrics.distinct_speaker_labels >= rules.minimum_distinct_speaker_labels;
    let output_segment_count_pass = metrics.segment_count >= rules.minimum_output_segments;
    let normalized_characters_pass =
        metrics.normalized_character_count >= rules.minimum_normalized_characters;
    let maximum_segment_duration_pass =
        metrics.maximum_segment_duration_ms <= rules.maximum_segment_duration_ms;
    let invalid_segment_count_pass =
        metrics.invalid_segment_count == 0 && metrics.nondecreasing_start;
    let objective_subset_pass = first_segment_start_pass
        && last_segment_end_pass
        && output_speech_union_pass
        && internal_gap_pass
        && distinct_speaker_labels_pass
        && output_segment_count_pass
        && normalized_characters_pass
        && maximum_segment_duration_pass
        && invalid_segment_count_pass;
    FullRunGateSubset {
        maximum_first_segment_start_ms: rules.maximum_first_segment_start_ms,
        minimum_last_segment_end_ms: rules.minimum_last_segment_end_ms,
        minimum_output_speech_union_ms: rules.minimum_output_speech_union_ms,
        maximum_internal_gap_ms: rules.maximum_internal_gap_ms,
        minimum_distinct_speaker_labels: rules.minimum_distinct_speaker_labels,
        minimum_output_segments: rules.minimum_output_segments,
        minimum_normalized_characters: rules.minimum_normalized_characters,
        maximum_segment_duration_ms: rules.maximum_segment_duration_ms,
        first_segment_start_pass,
        last_segment_end_pass,
        output_speech_union_pass,
        internal_gap_pass,
        distinct_speaker_labels_pass,
        output_segment_count_pass,
        normalized_characters_pass,
        maximum_segment_duration_pass,
        invalid_segment_count_pass,
        objective_subset_pass,
    }
}

fn boundary_spill(
    segments: &[MetricSegment],
    window_start_ms: i64,
    window_end_ms: i64,
) -> BoundarySpill {
    let overlapping = segments
        .iter()
        .filter(|segment| segment.end_ms > window_start_ms && segment.start_ms < window_end_ms)
        .collect::<Vec<_>>();
    let covering_start_ms = overlapping.iter().map(|segment| segment.start_ms).min();
    let covering_end_ms = overlapping.iter().map(|segment| segment.end_ms).max();
    BoundarySpill {
        window_start_ms,
        window_end_ms,
        overlapping_segment_count: overlapping.len(),
        covering_start_ms,
        covering_end_ms,
        left_spill_ms: covering_start_ms.map(|value| window_start_ms.saturating_sub(value).max(0)),
        right_spill_ms: covering_end_ms.map(|value| value.saturating_sub(window_end_ms).max(0)),
    }
}

fn spill_pass(metrics: &BoundarySpill, maximum_spill_ms: i64) -> bool {
    metrics.overlapping_segment_count > 0
        && metrics
            .left_spill_ms
            .is_some_and(|value| value <= maximum_spill_ms)
        && metrics
            .right_spill_ms
            .is_some_and(|value| value <= maximum_spill_ms)
}

fn write_evidence(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}
