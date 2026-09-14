use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::database::moss::{
    CandidateAlignmentInput, CandidateSegmentInput, SourceTranscriptAnchor,
    SourceTranscriptAnchorSnapshot,
};
use crate::moss_helper::manager::{ManagedSegment, ManagedTranscription};

pub const ALIGNMENT_METHOD_RAW: &str = "moss_segment";
pub const ALIGNMENT_METHOD_SOURCE: &str = "source_transcript_segment";
pub const ALIGNMENT_TIME_TOLERANCE_MS: i64 = 2_000;
pub const MIN_GLOBAL_MATCH_COVERAGE: f64 = 0.65;
pub const MIN_ANCHOR_MATCH_COVERAGE: f64 = 0.50;
pub const MIN_RAW_SEGMENT_MATCH_COVERAGE: f64 = 0.65;
pub const MIN_MATCHED_CHARACTERS_PER_ANCHOR: usize = 4;
const MAX_ALIGNMENT_CELLS: usize = 24_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAlignment {
    pub segments: Vec<CandidateSegmentInput>,
    pub provenance: Vec<CandidateAlignmentInput>,
    pub aligned_segment_count: u32,
    pub fallback_segment_count: u32,
    pub source_anchor_count: u32,
    pub source_hash_verified: bool,
    pub source_expected_sha256: String,
    pub source_actual_sha256: String,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AlignmentError {
    #[error("managed MOSS result is invalid")]
    InvalidManagedResult,
}

#[derive(Debug, Clone)]
struct NormalizedChar {
    value: char,
    original_char_index: usize,
    anchor_index: usize,
}

#[derive(Debug)]
struct SegmentAlignment {
    pieces: Vec<(String, SourceTranscriptAnchor, f64)>,
}

pub fn align_managed_transcription(
    result: &ManagedTranscription,
    snapshot: &SourceTranscriptAnchorSnapshot,
) -> Result<ProductAlignment, AlignmentError> {
    let raw_segments = raw_candidate_segments(&result.segments)?;
    align_candidate_segments(&raw_segments, snapshot)
}

pub fn align_candidate_segments(
    raw_segments: &[CandidateSegmentInput],
    snapshot: &SourceTranscriptAnchorSnapshot,
) -> Result<ProductAlignment, AlignmentError> {
    let raw_segments = validated_candidate_segments(raw_segments)?;
    let fallback_reason = if !snapshot.hash_verified {
        Some("SOURCE_TRANSCRIPT_HASH_MISMATCH".to_owned())
    } else if snapshot.invalid_timed_rows > 0 || !source_anchors_are_valid(&snapshot.anchors) {
        Some("SOURCE_TRANSCRIPT_TIME_INVALID".to_owned())
    } else if snapshot.anchors.is_empty() {
        Some("SOURCE_TRANSCRIPT_ANCHORS_MISSING".to_owned())
    } else {
        None
    };
    if fallback_reason.is_some() {
        return Ok(raw_outcome(raw_segments, snapshot, fallback_reason));
    }
    let proposed_alignments = match align_segments_globally(&raw_segments, &snapshot.anchors) {
        Ok(value) => value,
        Err(reason) => {
            return Ok(raw_outcome(raw_segments, snapshot, Some(reason.to_owned())));
        }
    };

    let mut segments = Vec::new();
    let mut provenance = Vec::new();
    let mut aligned_segment_count = 0u32;
    let mut fallback_segment_count = 0u32;
    for (raw_index, (raw, proposed)) in raw_segments.iter().zip(proposed_alignments).enumerate() {
        match proposed {
            Some(aligned)
                if alignment_fits_sequence(
                    &aligned,
                    raw,
                    segments
                        .last()
                        .map(|segment: &CandidateSegmentInput| segment.start_ms),
                    raw_segments
                        .get(raw_index.saturating_add(1))
                        .map(|segment| segment.start_ms),
                ) =>
            {
                let resolved_times = resolved_alignment_times(raw, &aligned)
                    .ok_or(AlignmentError::InvalidManagedResult)?;
                for ((text, anchor, confidence), (start_ms, end_ms)) in
                    aligned.pieces.into_iter().zip(resolved_times)
                {
                    let segment_index = u32::try_from(segments.len())
                        .map_err(|_| AlignmentError::InvalidManagedResult)?;
                    segments.push(CandidateSegmentInput {
                        segment_index,
                        start_ms,
                        end_ms,
                        speaker_label: raw.speaker_label.clone(),
                        text,
                    });
                    provenance.push(CandidateAlignmentInput {
                        segment_index,
                        raw_segment_index: raw.segment_index,
                        raw_start_ms: raw.start_ms,
                        raw_end_ms: raw.end_ms,
                        raw_text_sha256: sha256_text(&raw.text),
                        alignment_method: ALIGNMENT_METHOD_SOURCE.to_owned(),
                        confidence: Some(confidence),
                        source_anchor_ids: vec![anchor.anchor_id],
                    });
                    aligned_segment_count = aligned_segment_count.saturating_add(1);
                }
            }
            Some(_) | None => {
                let raw_segment_index = raw.segment_index;
                let segment_index = u32::try_from(segments.len())
                    .map_err(|_| AlignmentError::InvalidManagedResult)?;
                let mut raw = raw.clone();
                raw.segment_index = segment_index;
                segments.push(raw.clone());
                provenance.push(raw_provenance(&raw, raw_segment_index));
                fallback_segment_count = fallback_segment_count.saturating_add(1);
            }
        }
    }

    if !timestamps_are_monotonic(&segments)
        || concatenate_text(&segments) != concatenate_text(&raw_segments)
        || segments.len() != provenance.len()
    {
        return Ok(raw_outcome(
            raw_segments,
            snapshot,
            Some("ALIGNMENT_INVARIANT_FALLBACK".to_owned()),
        ));
    }

    Ok(ProductAlignment {
        segments,
        provenance,
        aligned_segment_count,
        fallback_segment_count,
        source_anchor_count: snapshot.anchors.len().try_into().unwrap_or(u32::MAX),
        source_hash_verified: true,
        source_expected_sha256: snapshot.expected_sha256.clone(),
        source_actual_sha256: snapshot.actual_sha256.clone(),
        fallback_reason: if aligned_segment_count == 0 {
            Some("NO_SEGMENT_MET_ALIGNMENT_COVERAGE".to_owned())
        } else if fallback_segment_count > 0 {
            Some("PARTIAL_ALIGNMENT_FALLBACK".to_owned())
        } else {
            None
        },
    })
}

fn validated_candidate_segments(
    segments: &[CandidateSegmentInput],
) -> Result<Vec<CandidateSegmentInput>, AlignmentError> {
    if segments.is_empty()
        || segments.iter().enumerate().any(|(index, segment)| {
            usize::try_from(segment.segment_index).ok() != Some(index)
                || segment.start_ms < 0
                || segment.end_ms <= segment.start_ms
                || segment.text.trim().is_empty()
                || !segment
                    .speaker_label
                    .strip_prefix('S')
                    .is_some_and(|digits| {
                        digits.len() >= 2 && digits.bytes().all(|value| value.is_ascii_digit())
                    })
        })
        || segments
            .windows(2)
            .any(|pair| pair[1].start_ms < pair[0].start_ms)
    {
        return Err(AlignmentError::InvalidManagedResult);
    }
    Ok(segments.to_vec())
}

fn source_anchors_are_valid(anchors: &[SourceTranscriptAnchor]) -> bool {
    anchors.iter().all(|anchor| {
        anchor.start_ms >= 0
            && anchor.end_ms > anchor.start_ms
            && !anchor.anchor_id.trim().is_empty()
            && !anchor.text.trim().is_empty()
    }) && anchors
        .windows(2)
        .all(|pair| pair[0].start_ms <= pair[1].start_ms)
}

fn alignment_fits_sequence(
    alignment: &SegmentAlignment,
    raw: &CandidateSegmentInput,
    previous_output_start_ms: Option<i64>,
    next_raw_start_ms: Option<i64>,
) -> bool {
    let Some(resolved_times) = resolved_alignment_times(raw, alignment) else {
        return false;
    };
    let first_start_ms = resolved_times[0].0;
    let last_start_ms = resolved_times[resolved_times.len() - 1].0;
    if first_start_ms < previous_output_start_ms.unwrap_or(i64::MIN)
        || last_start_ms > next_raw_start_ms.unwrap_or(i64::MAX)
    {
        return false;
    }
    alignment
        .pieces
        .iter()
        .all(|piece| !piece.0.trim().is_empty())
}

fn resolved_alignment_times(
    raw: &CandidateSegmentInput,
    alignment: &SegmentAlignment,
) -> Option<Vec<(i64, i64)>> {
    if raw.end_ms <= raw.start_ms || alignment.pieces.is_empty() {
        return None;
    }

    // The source transcript may safely provide boundaries between pieces, but it
    // must never expand, shorten, or otherwise replace the original MOSS time
    // range.  Partitioning the raw range keeps speaker coverage and gaps exactly
    // as MOSS produced them while still giving long text a reviewable split.
    let mut boundaries = Vec::with_capacity(alignment.pieces.len().saturating_add(1));
    boundaries.push(raw.start_ms);
    boundaries.extend(
        alignment
            .pieces
            .iter()
            .skip(1)
            .map(|piece| piece.1.start_ms.clamp(raw.start_ms, raw.end_ms)),
    );
    boundaries.push(raw.end_ms);
    if boundaries.len() != alignment.pieces.len().saturating_add(1)
        || boundaries.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return None;
    }
    Some(
        boundaries
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect(),
    )
}

fn raw_candidate_segments(
    segments: &[ManagedSegment],
) -> Result<Vec<CandidateSegmentInput>, AlignmentError> {
    if segments.is_empty() {
        return Err(AlignmentError::InvalidManagedResult);
    }
    let mut previous_start = i64::MIN;
    segments
        .iter()
        .enumerate()
        .map(|(expected_index, segment)| {
            if usize::try_from(segment.segment_index).ok() != Some(expected_index)
                || segment.t0_ms < 0
                || segment.t1_ms < segment.t0_ms
                || segment.t0_ms < previous_start
                || segment.speaker_id < 0
                || segment.text.trim().is_empty()
            {
                return Err(AlignmentError::InvalidManagedResult);
            }
            previous_start = segment.t0_ms;
            let speaker_number = segment
                .speaker_id
                .checked_add(1)
                .ok_or(AlignmentError::InvalidManagedResult)?;
            Ok(CandidateSegmentInput {
                segment_index: segment.segment_index,
                start_ms: segment.t0_ms,
                end_ms: segment.t1_ms,
                speaker_label: format!("S{speaker_number:02}"),
                text: segment.text.clone(),
            })
        })
        .collect()
}

fn align_segments_globally(
    raw_segments: &[CandidateSegmentInput],
    anchors: &[SourceTranscriptAnchor],
) -> Result<Vec<Option<SegmentAlignment>>, &'static str> {
    let mut moss = Vec::new();
    let mut raw_lengths = Vec::with_capacity(raw_segments.len());
    for (raw_index, raw) in raw_segments.iter().enumerate() {
        let normalized = normalize_text(&raw.text, raw_index);
        raw_lengths.push(normalized.len());
        moss.extend(normalized);
    }
    let mut source = Vec::new();
    let mut source_lengths = Vec::with_capacity(anchors.len());
    for (anchor_index, anchor) in anchors.iter().enumerate() {
        let normalized = normalize_text(&anchor.text, anchor_index);
        source_lengths.push(normalized.len());
        source.extend(normalized);
    }
    if moss.is_empty() || source.is_empty() {
        return Err("GLOBAL_ALIGNMENT_TEXT_EMPTY");
    }
    let cells = moss
        .len()
        .checked_add(1)
        .and_then(|height| {
            source
                .len()
                .checked_add(1)
                .and_then(|width| height.checked_mul(width))
        })
        .ok_or("GLOBAL_ALIGNMENT_LIMIT_EXCEEDED")?;
    if cells > MAX_ALIGNMENT_CELLS {
        return Err("GLOBAL_ALIGNMENT_LIMIT_EXCEEDED");
    }
    let matches =
        exact_matches_from_edit_alignment(&moss, &source).ok_or("GLOBAL_ALIGNMENT_FAILED")?;
    let global_coverage =
        (matches.len() as f64 / moss.len() as f64).min(matches.len() as f64 / source.len() as f64);
    if global_coverage < MIN_GLOBAL_MATCH_COVERAGE {
        return Err("GLOBAL_ALIGNMENT_LOW_COVERAGE");
    }

    let mut source_match_counts = vec![0usize; anchors.len()];
    let mut raw_matches = vec![BTreeMap::<usize, Vec<usize>>::new(); raw_segments.len()];
    for (moss_index, source_index) in matches {
        let raw_index = moss[moss_index].anchor_index;
        let anchor_index = source[source_index].anchor_index;
        source_match_counts[anchor_index] = source_match_counts[anchor_index].saturating_add(1);
        raw_matches[raw_index]
            .entry(anchor_index)
            .or_default()
            .push(moss[moss_index].original_char_index);
    }

    let mut results = Vec::with_capacity(raw_segments.len());
    for (raw_index, raw) in raw_segments.iter().enumerate() {
        let normalized_raw_length = raw_lengths[raw_index];
        if normalized_raw_length == 0 {
            results.push(None);
            continue;
        }
        let mut selected = Vec::<(usize, Vec<usize>, f64)>::new();
        for (anchor_index, matched_original_indices) in &raw_matches[raw_index] {
            let anchor = &anchors[*anchor_index];
            let source_length = source_lengths[*anchor_index];
            if source_length == 0
                || matched_original_indices.len() < MIN_MATCHED_CHARACTERS_PER_ANCHOR
                || (source_match_counts[*anchor_index] as f64 / source_length as f64)
                    < MIN_ANCHOR_MATCH_COVERAGE
                || anchor.end_ms <= raw.start_ms
                || anchor.start_ms >= raw.end_ms
            {
                continue;
            }
            selected.push((
                *anchor_index,
                matched_original_indices.clone(),
                source_match_counts[*anchor_index] as f64 / source_length as f64,
            ));
        }
        let selected_match_count = selected
            .iter()
            .map(|(_, indices, _)| indices.len())
            .sum::<usize>();
        let raw_coverage = selected_match_count as f64 / normalized_raw_length as f64;
        if selected.is_empty() || raw_coverage < MIN_RAW_SEGMENT_MATCH_COVERAGE {
            results.push(None);
            continue;
        }
        if selected.windows(2).any(|pair| {
            pair[0].1.last().copied().unwrap_or(usize::MAX)
                >= pair[1].1.first().copied().unwrap_or(0)
        }) {
            results.push(None);
            continue;
        }

        let raw_chars = raw.text.chars().collect::<Vec<_>>();
        let mut boundaries = Vec::with_capacity(selected.len().saturating_add(1));
        boundaries.push(0usize);
        boundaries.extend(
            selected
                .iter()
                .skip(1)
                .filter_map(|(_, indices, _)| indices.first().copied()),
        );
        boundaries.push(raw_chars.len());
        if boundaries.len() != selected.len().saturating_add(1)
            || boundaries.windows(2).any(|pair| pair[0] >= pair[1])
        {
            results.push(None);
            continue;
        }

        let mut pieces = Vec::with_capacity(selected.len());
        for (piece_index, (anchor_index, _, source_coverage)) in selected.into_iter().enumerate() {
            let text = raw_chars[boundaries[piece_index]..boundaries[piece_index + 1]]
                .iter()
                .collect::<String>();
            if text.trim().is_empty() {
                pieces.clear();
                break;
            }
            pieces.push((
                text,
                anchors[anchor_index].clone(),
                global_coverage.min(raw_coverage).min(source_coverage),
            ));
        }
        if pieces.is_empty()
            || pieces
                .iter()
                .map(|piece| piece.0.as_str())
                .collect::<String>()
                != raw.text
        {
            results.push(None);
        } else {
            results.push(Some(SegmentAlignment { pieces }));
        }
    }
    Ok(results)
}

fn normalize_text(text: &str, anchor_index: usize) -> Vec<NormalizedChar> {
    text.chars()
        .enumerate()
        .filter(|(_, value)| value.is_alphanumeric())
        .map(|(original_char_index, value)| NormalizedChar {
            value: if value.is_ascii() {
                value.to_ascii_lowercase()
            } else {
                value
            },
            original_char_index,
            anchor_index,
        })
        .collect()
}

fn exact_matches_from_edit_alignment(
    left: &[NormalizedChar],
    right: &[NormalizedChar],
) -> Option<Vec<(usize, usize)>> {
    let width = right.len().checked_add(1)?;
    let height = left.len().checked_add(1)?;
    let mut distance = vec![0u32; width.checked_mul(height)?];
    for i in 0..height {
        distance[i * width] = u32::try_from(i).ok()?;
    }
    for j in 0..width {
        distance[j] = u32::try_from(j).ok()?;
    }
    for i in 1..height {
        for j in 1..width {
            let substitution = distance[(i - 1) * width + j - 1]
                + u32::from(left[i - 1].value != right[j - 1].value);
            let deletion = distance[(i - 1) * width + j] + 1;
            let insertion = distance[i * width + j - 1] + 1;
            distance[i * width + j] = substitution.min(deletion).min(insertion);
        }
    }

    let mut i = left.len();
    let mut j = right.len();
    let mut matches = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = u32::from(left[i - 1].value != right[j - 1].value);
            if distance[i * width + j] == distance[(i - 1) * width + j - 1] + cost {
                if cost == 0 {
                    matches.push((i - 1, j - 1));
                }
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && distance[i * width + j] == distance[(i - 1) * width + j] + 1 {
            i -= 1;
        } else if j > 0 {
            j -= 1;
        } else {
            return None;
        }
    }
    matches.reverse();
    Some(matches)
}

fn timestamps_are_monotonic(segments: &[CandidateSegmentInput]) -> bool {
    segments
        .iter()
        .all(|segment| segment.start_ms >= 0 && segment.end_ms >= segment.start_ms)
        && segments
            .windows(2)
            .all(|pair| pair[0].start_ms <= pair[1].start_ms)
}

fn concatenate_text(segments: &[CandidateSegmentInput]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

fn raw_provenance(
    segment: &CandidateSegmentInput,
    raw_segment_index: u32,
) -> CandidateAlignmentInput {
    CandidateAlignmentInput {
        segment_index: segment.segment_index,
        raw_segment_index,
        raw_start_ms: segment.start_ms,
        raw_end_ms: segment.end_ms,
        raw_text_sha256: sha256_text(&segment.text),
        alignment_method: ALIGNMENT_METHOD_RAW.to_owned(),
        confidence: None,
        source_anchor_ids: Vec::new(),
    }
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn raw_outcome(
    mut segments: Vec<CandidateSegmentInput>,
    snapshot: &SourceTranscriptAnchorSnapshot,
    fallback_reason: Option<String>,
) -> ProductAlignment {
    let provenance = segments
        .iter_mut()
        .enumerate()
        .map(|(index, segment)| {
            segment.segment_index = index.try_into().unwrap_or(u32::MAX);
            raw_provenance(segment, segment.segment_index)
        })
        .collect::<Vec<_>>();
    ProductAlignment {
        fallback_segment_count: segments.len().try_into().unwrap_or(u32::MAX),
        segments,
        provenance,
        aligned_segment_count: 0,
        source_anchor_count: snapshot.anchors.len().try_into().unwrap_or(u32::MAX),
        source_hash_verified: snapshot.hash_verified,
        source_expected_sha256: snapshot.expected_sha256.clone(),
        source_actual_sha256: snapshot.actual_sha256.clone(),
        fallback_reason,
    }
}

#[cfg(test)]
mod tests {
    use moss_helper::protocol::{
        CompletedMessage, CompletionStatus, NativeSessionLimits, NativeTimings,
    };

    use super::*;

    fn managed(segments: Vec<ManagedSegment>) -> ManagedTranscription {
        ManagedTranscription {
            request_id: "798d8c63-5ff1-40e3-9db8-0f706aeb930a".to_owned(),
            context_sha256: "a".repeat(64),
            raw_text: segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect(),
            clean_text: segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect(),
            segments,
            completed: CompletedMessage {
                v: moss_helper::protocol::PROTOCOL_VERSION,
                seq: 1,
                request_id: "798d8c63-5ff1-40e3-9db8-0f706aeb930a".to_owned(),
                context_sha256: "a".repeat(64),
                terminal: true,
                status: CompletionStatus::Ok,
                backend: "Vulkan0".to_owned(),
                device_description: "Intel(R) Arc(TM) Graphics".to_owned(),
                raw_text_sha256: "b".repeat(64),
                clean_text_sha256: "c".repeat(64),
                segment_count: 1,
                last_timestamp_ms: 2_000,
                native_run_elapsed_ms: 1,
                native_rtf: 0.1,
                wall_elapsed_ms: 1,
                wall_rtf: 0.1,
                native_timings: NativeTimings {
                    load_ms: 0.0,
                    mel_ms: 0.0,
                    encode_ms: 0.0,
                    decode_ms: 0.0,
                },
                was_aborted: false,
                was_truncated: false,
                native_session_limits: NativeSessionLimits {
                    effective_n_ctx: moss_helper::native::MOSS_SESSION_N_CTX,
                    effective_max_audio_ms: 1_200_000,
                    max_kv_bytes: 1_879_048_192,
                },
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            },
            heartbeat_count: 0,
            supervisor_wall_elapsed_ms: 1,
            supervisor_wall_rtf: 0.1,
            helper_process_id: 1,
            helper_total_processes: 1,
            helper_peak_job_memory_bytes: 1,
            residual_process_count: 0,
            helper_runs: Vec::new(),
            audio_activity: None,
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    fn snapshot(anchors: Vec<SourceTranscriptAnchor>) -> SourceTranscriptAnchorSnapshot {
        SourceTranscriptAnchorSnapshot {
            expected_sha256: "d".repeat(64),
            actual_sha256: "d".repeat(64),
            hash_verified: true,
            invalid_timed_rows: 0,
            anchors,
        }
    }

    fn anchor(id: &str, start_ms: i64, end_ms: i64, text: &str) -> SourceTranscriptAnchor {
        SourceTranscriptAnchor {
            anchor_id: id.to_owned(),
            start_ms,
            end_ms,
            text: text.to_owned(),
        }
    }

    #[test]
    fn splits_on_real_source_boundaries_and_preserves_text_exactly() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 0,
            t1_ms: 2_000,
            speaker_id: 0,
            text: "牌照站使用 CGS；Google 包继续测试。".to_owned(),
        }]);
        let result = align_managed_transcription(
            &input,
            &snapshot(vec![
                anchor("t1", 0, 900, "牌照站使用CGS"),
                anchor("t2", 900, 2_000, "Google包继续测试"),
            ]),
        )
        .unwrap();
        assert_eq!(result.segments.len(), 2);
        assert_eq!(concatenate_text(&result.segments), input.segments[0].text);
        assert_eq!(
            (result.segments[0].start_ms, result.segments[0].end_ms),
            (0, 900)
        );
        assert_eq!(
            (result.segments[1].start_ms, result.segments[1].end_ms),
            (900, 2_000)
        );
        assert!(result
            .provenance
            .iter()
            .all(|value| value.alignment_method == ALIGNMENT_METHOD_SOURCE));
    }

    #[test]
    fn low_coverage_falls_back_without_changing_text() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 0,
            t1_ms: 2_000,
            speaker_id: 0,
            text: "这是完全不同的内容".to_owned(),
        }]);
        let result = align_managed_transcription(
            &input,
            &snapshot(vec![anchor("t1", 0, 2_000, "Google package testing")]),
        )
        .unwrap();
        assert_eq!(result.segments[0].text, input.segments[0].text);
        assert_eq!(result.provenance[0].alignment_method, ALIGNMENT_METHOD_RAW);
    }

    #[test]
    fn hash_mismatch_never_uses_source_timestamps() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 50,
            t1_ms: 2_000,
            speaker_id: 0,
            text: "Google 包继续测试".to_owned(),
        }]);
        let mut source = snapshot(vec![anchor("t1", 0, 1_000, "Google 包继续测试")]);
        source.actual_sha256 = "e".repeat(64);
        source.hash_verified = false;
        let result = align_managed_transcription(&input, &source).unwrap();
        assert_eq!(
            (result.segments[0].start_ms, result.segments[0].end_ms),
            (50, 2_000)
        );
        assert_eq!(
            result.fallback_reason.as_deref(),
            Some("SOURCE_TRANSCRIPT_HASH_MISMATCH")
        );
    }

    #[test]
    fn invalid_or_missing_timestamps_force_raw_fallback() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 50,
            t1_ms: 2_000,
            speaker_id: 0,
            text: "PWA 继续测试".to_owned(),
        }]);
        let mut source = snapshot(Vec::new());
        source.invalid_timed_rows = 1;
        let result = align_managed_transcription(&input, &source).unwrap();
        assert_eq!(result.provenance[0].alignment_method, ALIGNMENT_METHOD_RAW);
        assert_eq!(
            result.fallback_reason.as_deref(),
            Some("SOURCE_TRANSCRIPT_TIME_INVALID")
        );
    }

    #[test]
    fn overlapping_raw_segments_are_kept_when_alignment_would_break_order() {
        let input = managed(vec![
            ManagedSegment {
                segment_index: 0,
                t0_ms: 0,
                t1_ms: 2_000,
                speaker_id: 0,
                text: "甲方先说".to_owned(),
            },
            ManagedSegment {
                segment_index: 1,
                t0_ms: 1_000,
                t1_ms: 2_500,
                speaker_id: 1,
                text: "乙方同时说".to_owned(),
            },
        ]);
        let source = snapshot(vec![
            anchor("t1", 1_500, 2_000, "甲方先说"),
            anchor("t2", 1_000, 2_500, "乙方同时说"),
        ]);
        let result = align_managed_transcription(&input, &source).unwrap();
        assert!(timestamps_are_monotonic(&result.segments));
        assert_eq!(
            concatenate_text(&result.segments),
            concatenate_text(&raw_candidate_segments(&input.segments).unwrap())
        );
        assert_eq!(result.provenance[0].alignment_method, ALIGNMENT_METHOD_RAW);
        assert_eq!(result.provenance[1].alignment_method, ALIGNMENT_METHOD_RAW);
        assert_eq!(result.aligned_segment_count, 0);
        assert_eq!(result.fallback_segment_count, 2);
    }

    #[test]
    fn source_boundaries_partition_raw_range_without_losing_speaker_time() {
        let input = managed(vec![
            ManagedSegment {
                segment_index: 0,
                t0_ms: 1_500,
                t1_ms: 1_600,
                speaker_id: 0,
                text: "无法匹配的第一句".to_owned(),
            },
            ManagedSegment {
                segment_index: 1,
                t0_ms: 1_600,
                t1_ms: 1_800,
                speaker_id: 0,
                text: "会导致时间倒退".to_owned(),
            },
            ManagedSegment {
                segment_index: 2,
                t0_ms: 5_000,
                t1_ms: 6_000,
                speaker_id: 1,
                text: "后面的安全对齐仍然保留".to_owned(),
            },
        ]);
        let result = align_managed_transcription(
            &input,
            &snapshot(vec![
                anchor("rewind", 1_400, 1_700, "会导致时间倒退"),
                anchor("safe", 5_000, 6_000, "后面的安全对齐仍然保留"),
            ]),
        )
        .unwrap();

        assert!(timestamps_are_monotonic(&result.segments));
        assert_eq!(
            concatenate_text(&result.segments),
            concatenate_text(&raw_candidate_segments(&input.segments).unwrap())
        );
        assert_eq!(result.provenance[0].alignment_method, ALIGNMENT_METHOD_RAW);
        assert_eq!(
            result.provenance[1].alignment_method,
            ALIGNMENT_METHOD_SOURCE
        );
        assert_eq!(
            (result.segments[1].start_ms, result.segments[1].end_ms),
            (1_600, 1_800)
        );
        assert_eq!(
            result.provenance[2].alignment_method,
            ALIGNMENT_METHOD_SOURCE
        );
        assert_eq!(result.aligned_segment_count, 2);
        assert_eq!(result.fallback_segment_count, 1);
        assert_eq!(
            result.fallback_reason.as_deref(),
            Some("PARTIAL_ALIGNMENT_FALLBACK")
        );
    }
}
