#!/usr/bin/env python3
"""Materialize and score one formally bound R5 MOSS candidate.

The product alignment and machine term suggestions are consumed before any
human truth is loaded.  Frozen human truth is loaded only for the final score.
All gates fail closed and both output files must be new paths.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
import sys
from pathlib import Path
from types import ModuleType
from typing import Any


class R5ScoreError(RuntimeError):
    pass


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise R5ScoreError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(
            path.read_text(encoding="utf-8-sig"),
            object_pairs_hook=strict_object,
            parse_constant=lambda item: (_ for _ in ()).throw(
                R5ScoreError(f"invalid JSON number: {item}")
            ),
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise R5ScoreError(f"cannot read JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise R5ScoreError(f"JSON root is not an object: {path}")
    return value


def json_bytes(value: Any, *, pretty: bool = False, sort_keys: bool = False) -> bytes:
    options: dict[str, Any] = {
        "ensure_ascii": False,
        "allow_nan": False,
        "sort_keys": sort_keys,
    }
    if pretty:
        options["indent"] = 2
        encoded = json.dumps(value, **options) + "\n"
    else:
        options["separators"] = (",", ":")
        encoded = json.dumps(value, **options)
    return encoded.encode("utf-8")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_dict(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise R5ScoreError(f"{label} is not an object")
    return value


def require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise R5ScoreError(f"{label} is not an array")
    return value


def require_sha256(value: Any, label: str) -> str:
    text = str(value)
    if len(text) != 64 or any(character not in "0123456789abcdefABCDEF" for character in text):
        raise R5ScoreError(f"{label} is not SHA-256")
    return text.lower()


def require_bool(value: Any, label: str) -> bool:
    if not isinstance(value, bool):
        raise R5ScoreError(f"{label} is not boolean")
    return value


def require_int(value: Any, label: str, minimum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise R5ScoreError(f"{label} is not an integer")
    if minimum is not None and value < minimum:
        raise R5ScoreError(f"{label} is below its minimum")
    return value


def require_number(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise R5ScoreError(f"{label} is not numeric")
    result = float(value)
    if not math.isfinite(result):
        raise R5ScoreError(f"{label} is not finite")
    return result


def load_frozen_scorer(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("moss_r5_frozen_scorer", path)
    if spec is None or spec.loader is None:
        raise R5ScoreError("cannot load frozen scorer")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def manifest_entries(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    rows = require_list(manifest.get("files"), "manifest files")
    result: dict[str, dict[str, Any]] = {}
    for row_value in rows:
        row = require_dict(row_value, "manifest entry")
        relative_path = str(row.get("relative_path", ""))
        if not relative_path or relative_path in result:
            raise R5ScoreError("manifest contains an invalid or duplicate path")
        result[relative_path] = row
    return result


def verify_manifest_file(
    entries: dict[str, dict[str, Any]], path: Path
) -> dict[str, Any]:
    row = entries.get(path.name)
    if row is None:
        raise R5ScoreError(f"file is not present in frozen manifest: {path.name}")
    actual_hash = sha256_file(path)
    actual_bytes = path.stat().st_size
    expected_hash = require_sha256(row.get("sha256"), f"manifest hash for {path.name}")
    expected_bytes = require_int(row.get("bytes"), f"manifest bytes for {path.name}", 0)
    if actual_hash != expected_hash or actual_bytes != expected_bytes:
        raise R5ScoreError(f"frozen file changed: {path.name}")
    return {
        "path": str(path),
        "bytes": actual_bytes,
        "sha256": actual_hash,
        "manifest_verified": True,
    }


def gate(status: str, actual: Any, threshold: Any = None) -> dict[str, Any]:
    if status not in {"PASS", "FAIL", "NOT_RUN"}:
        raise R5ScoreError(f"invalid gate status: {status}")
    value: dict[str, Any] = {"status": status, "actual": actual}
    if threshold is not None:
        value["threshold"] = threshold
    return value


def input_lock_rows(value: dict[str, Any]) -> dict[str, dict[str, Any]]:
    if value.get("role") != "MOSS_R5_INPUT_LOCK":
        raise R5ScoreError("R5 input lock role is invalid")
    rows = require_list(value.get("frozen_inputs"), "R5 frozen inputs")
    result: dict[str, dict[str, Any]] = {}
    for row_value in rows:
        row = require_dict(row_value, "R5 frozen input")
        role = str(row.get("role", ""))
        if not role or role in result:
            raise R5ScoreError("R5 input lock has an invalid or duplicate role")
        require_sha256(row.get("sha256"), f"R5 lock {role}")
        require_int(row.get("bytes"), f"R5 lock bytes {role}", 0)
        result[role] = row
    return result


def verify_rust_value_payload_hash(evidence: dict[str, Any]) -> None:
    expected = require_sha256(evidence.get("payload_sha256"), "formal payload hash")
    payload = dict(evidence)
    payload.pop("payload_sha256", None)
    actual = sha256_bytes(json_bytes(payload, sort_keys=True))
    if actual != expected:
        raise R5ScoreError("formal payload hash does not recompute")


def ordered_fields(value: dict[str, Any], names: tuple[str, ...], label: str) -> dict[str, Any]:
    if set(value) != set(names):
        raise R5ScoreError(f"{label} fields are not exact")
    return {name: value[name] for name in names}


def verify_audio_token_track(evidence: dict[str, Any]) -> dict[str, Any]:
    track = require_dict(evidence.get("audio_token_track"), "audio token track")
    if require_int(track.get("schema_version"), "track schema") != 1:
        raise R5ScoreError("audio token schema is not frozen version 1")
    parameters = require_dict(track.get("parameters"), "token parameters")
    parameter_names = (
        "schema_version",
        "language",
        "sample_rate_hz",
        "channels",
        "vad_redemption_time_ms",
        "maximum_segment_samples",
        "minimum_segment_samples",
        "timestamp_tick_ms",
        "no_timestamps",
        "token_timestamps",
        "split_on_word",
        "initial_prompt_used",
        "synthetic_character_timing",
        "human_truth_used",
    )
    ordered_parameters = ordered_fields(parameters, parameter_names, "token parameters")
    parameter_hash = sha256_bytes(json_bytes(ordered_parameters))
    if parameter_hash != require_sha256(track.get("parameters_sha256"), "parameter hash"):
        raise R5ScoreError("token parameter hash does not recompute")
    expected_parameters = {
        "schema_version": 1,
        "language": "zh",
        "sample_rate_hz": 16000,
        "channels": 1,
        "vad_redemption_time_ms": 2000,
        "maximum_segment_samples": 400000,
        "minimum_segment_samples": 1600,
        "timestamp_tick_ms": 10,
        "no_timestamps": False,
        "token_timestamps": True,
        "split_on_word": True,
        "initial_prompt_used": False,
        "synthetic_character_timing": False,
        "human_truth_used": False,
    }
    if ordered_parameters != expected_parameters:
        raise R5ScoreError("token parameters differ from the frozen R5 product path")

    chunks = require_list(track.get("source_chunks"), "token source chunks")
    tokens = require_list(track.get("tokens"), "tokens")
    if not chunks or not tokens:
        raise R5ScoreError("audio token track is empty")
    duration_ms = require_int(track.get("audio_duration_ms"), "audio duration", 1)
    chunk_names = (
        "chunk_index",
        "start_ms",
        "end_ms",
        "sample_count",
        "text_sha256",
        "first_global_token_index",
        "token_count",
    )
    token_names = (
        "global_token_index",
        "chunk_index",
        "whisper_segment_index",
        "whisper_token_index",
        "start_ms",
        "end_ms",
        "text",
        "probability",
    )
    ordered_chunks: list[dict[str, Any]] = []
    ordered_tokens: list[dict[str, Any]] = []
    previous_token_start = -1
    for index, token_value in enumerate(tokens):
        token = require_dict(token_value, f"token {index}")
        ordered = ordered_fields(token, token_names, f"token {index}")
        if require_int(token.get("global_token_index"), "global token index") != index:
            raise R5ScoreError("global token indices are not exact")
        start_ms = require_int(token.get("start_ms"), "token start", 0)
        end_ms = require_int(token.get("end_ms"), "token end", 0)
        probability = require_number(token.get("probability"), "token probability")
        if (
            end_ms < start_ms
            or end_ms > duration_ms
            or start_ms < previous_token_start
            or not 0.0 <= probability <= 1.0
            or not str(token.get("text", "")).strip()
        ):
            raise R5ScoreError("audio token is invalid")
        previous_token_start = start_ms
        ordered_tokens.append(ordered)

    next_token = 0
    previous_chunk_end = -1
    for index, chunk_value in enumerate(chunks):
        chunk = require_dict(chunk_value, f"source chunk {index}")
        ordered = ordered_fields(chunk, chunk_names, f"source chunk {index}")
        chunk_index = require_int(chunk.get("chunk_index"), "chunk index")
        start_ms = require_int(chunk.get("start_ms"), "chunk start", 0)
        end_ms = require_int(chunk.get("end_ms"), "chunk end", 1)
        first = require_int(chunk.get("first_global_token_index"), "chunk first token", 0)
        count = require_int(chunk.get("token_count"), "chunk token count", 1)
        if (
            chunk_index != index
            or first != next_token
            or start_ms < previous_chunk_end
            or end_ms <= start_ms
            or end_ms > duration_ms
            or first + count > len(tokens)
        ):
            raise R5ScoreError("source chunk range is invalid")
        chunk_tokens = tokens[first : first + count]
        if any(
            require_int(item.get("chunk_index"), "token chunk index") != index
            or require_int(item.get("start_ms"), "token start") < start_ms
            or require_int(item.get("end_ms"), "token end") > end_ms
            for item in chunk_tokens
        ):
            raise R5ScoreError("source chunk token membership is invalid")
        chunk_text = "".join(str(item["text"]) for item in chunk_tokens)
        actual_text_hash = sha256_bytes(json_bytes(chunk_text))
        if actual_text_hash != require_sha256(chunk.get("text_sha256"), "chunk text hash"):
            raise R5ScoreError("source chunk text hash does not recompute")
        next_token = first + count
        previous_chunk_end = end_ms
        ordered_chunks.append(ordered)
    if next_token != len(tokens):
        raise R5ScoreError("source chunks do not cover the exact token track")

    # Rust hashes the direct struct, so f32 probabilities use Rust's shortest
    # round-trip representation.  The evidence JSON stores those same f32s as
    # f64 JSON numbers; recomputing that byte stream in Python would be a
    # different serialization.  The formal producer and the new realigner both
    # call Rust validate_audio_token_track(), while this scorer independently
    # checks every index, time, chunk, text hash and binding above.
    require_sha256(track.get("token_track_sha256"), "token track hash")
    return track


def verify_formal_bindings(
    evidence_path: Path,
    evidence: dict[str, Any],
    lock_path: Path,
    lock: dict[str, Any],
    r4_alignment_path: Path,
    r4_alignment: dict[str, Any],
) -> dict[str, Any]:
    if evidence.get("role") != "FORMAL_MEETILY_R5_PRODUCT_ALIGNMENT":
        raise R5ScoreError("formal R5 evidence role is invalid")
    if evidence.get("human_truth_used_for_alignment_or_correction") is not False:
        raise R5ScoreError("formal R5 producer consumed human truth")
    invariants = require_dict(evidence.get("invariants"), "formal invariants")
    required_invariants = (
        "output_indices_exact",
        "output_starts_monotonic",
        "provenance_count_exact",
        "provenance_valid",
        "outer_ranges_preserved",
        "speaker_labels_preserved",
        "text_preserved_exactly",
        "piece_raw_ranges_exact",
        "token_boundaries_valid",
        "token_track_bound",
        "machine_suggestions_valid",
        "human_truth_used",
        "all_pass",
    )
    for name in required_invariants:
        expected = False if name == "human_truth_used" else True
        if require_bool(invariants.get(name), f"formal invariant {name}") is not expected:
            raise R5ScoreError(f"formal invariant failed: {name}")
    verify_rust_value_payload_hash(evidence)

    rows = input_lock_rows(lock)
    required_lock_roles = (
        "r4_alignment_evidence",
        "frozen_real_audio",
        "whisper_alignment_model",
        "stable_release_executable_not_to_replace_before_go",
    )
    if any(role not in rows for role in required_lock_roles):
        raise R5ScoreError("R5 input lock lacks a required role")
    if sha256_file(r4_alignment_path) != require_sha256(
        rows["r4_alignment_evidence"].get("sha256"), "locked R4 alignment"
    ):
        raise R5ScoreError("R4 alignment evidence changed after the R5 input lock")

    inputs = require_dict(evidence.get("inputs"), "formal inputs")
    audio = require_dict(inputs.get("audio"), "formal audio input")
    model = require_dict(inputs.get("model"), "formal model input")
    raw_input = require_dict(inputs.get("raw_moss_candidate"), "formal raw candidate input")
    source_input = require_dict(inputs.get("source_transcript"), "formal source input")
    metadata_input = require_dict(inputs.get("meeting_metadata"), "formal metadata input")
    program = require_dict(inputs.get("program"), "formal program input")
    alignment_program = require_dict(
        inputs.get("alignment_program"), "formal alignment program input"
    )
    token_origin = require_dict(inputs.get("token_track_origin"), "token track origin")
    if require_sha256(audio.get("sha256"), "formal audio hash") != require_sha256(
        rows["frozen_real_audio"].get("sha256"), "locked audio"
    ):
        raise R5ScoreError("formal audio does not match the R5 input lock")
    if require_sha256(model.get("sha256"), "formal model hash") != require_sha256(
        rows["whisper_alignment_model"].get("sha256"), "locked model"
    ) or model.get("name") != "large-v3-turbo-q5_0":
        raise R5ScoreError("formal model does not match the R5 input lock")

    r4_inputs = require_dict(r4_alignment.get("inputs"), "R4 inputs")
    r4_raw = require_dict(r4_inputs.get("raw_moss_candidate"), "R4 raw candidate")
    r4_source = require_dict(r4_inputs.get("source_transcript_anchors"), "R4 source transcript")
    if require_sha256(raw_input.get("sha256"), "formal raw candidate hash") != require_sha256(
        r4_raw.get("expected_sha256"), "R4 raw candidate hash"
    ):
        raise R5ScoreError("formal raw candidate differs from frozen R4")
    if require_sha256(source_input.get("sha256"), "formal source transcript hash") != require_sha256(
        r4_source.get("expected_sha256"), "R4 source transcript hash"
    ):
        raise R5ScoreError("formal source transcript differs from frozen R4")

    small_input_rows = (
        ("raw_moss_candidate", raw_input),
        ("source_transcript", source_input),
        ("meeting_metadata", metadata_input),
        ("program", program),
        ("alignment_program", alignment_program),
    )
    verified_files: dict[str, Any] = {}
    for label, row in small_input_rows:
        path = Path(str(row.get("path", ""))).resolve(strict=True)
        actual = sha256_file(path)
        expected = require_sha256(row.get("sha256"), f"formal {label} hash")
        if actual != expected:
            raise R5ScoreError(f"formal {label} file changed after production")
        verified_files[label] = {
            "path": str(path),
            "bytes": path.stat().st_size,
            "sha256": actual,
        }

    track = verify_audio_token_track(evidence)
    for label, track_key, input_value in (
        ("audio", "audio_sha256", audio.get("sha256")),
        ("model", "model_sha256", model.get("sha256")),
        ("program", "program_sha256", program.get("sha256")),
        ("parameters", "parameters_sha256", inputs.get("parameters_sha256")),
        ("token track", "token_track_sha256", inputs.get("token_track_sha256")),
    ):
        if require_sha256(track.get(track_key), f"track {label}") != require_sha256(
            input_value, f"formal input {label}"
        ):
            raise R5ScoreError(f"audio token track {label} binding is invalid")

    origin_mode = str(token_origin.get("mode", ""))
    if origin_mode not in {"GENERATED_IN_THIS_RUN", "REUSED_VERIFIED_FORMAL_TOKEN_TRACK"}:
        raise R5ScoreError("formal token track origin mode is invalid")
    if origin_mode == "REUSED_VERIFIED_FORMAL_TOKEN_TRACK":
        if (
            token_origin.get("prior_product_alignment_reused") is not False
            or token_origin.get("audio_inference_repeated") is not False
        ):
            raise R5ScoreError("reused token track origin flags are invalid")
        prior_path = Path(str(token_origin.get("evidence_path", ""))).resolve(strict=True)
        if sha256_file(prior_path) != require_sha256(
            token_origin.get("evidence_sha256"), "prior token evidence hash"
        ):
            raise R5ScoreError("prior formal token evidence changed")
        prior = load_json(prior_path)
        verify_rust_value_payload_hash(prior)
        if (
            prior.get("role") != "FORMAL_MEETILY_R5_PRODUCT_ALIGNMENT"
            or prior.get("human_truth_used_for_alignment_or_correction") is not False
            or prior.get("audio_token_track") != track
            or require_sha256(prior.get("payload_sha256"), "prior payload hash")
            != require_sha256(
                token_origin.get("evidence_payload_sha256"), "origin payload hash"
            )
        ):
            raise R5ScoreError("reused token track does not exactly match prior formal evidence")
    elif token_origin.get("audio_inference_repeated") is not True:
        raise R5ScoreError("fresh token track origin flags are invalid")

    return {
        "formal_evidence": {
            "path": str(evidence_path),
            "bytes": evidence_path.stat().st_size,
            "sha256": sha256_file(evidence_path),
            "payload_sha256": evidence["payload_sha256"],
        },
        "r5_input_lock": {
            "path": str(lock_path),
            "bytes": lock_path.stat().st_size,
            "sha256": sha256_file(lock_path),
        },
        "r4_alignment_evidence": {
            "path": str(r4_alignment_path),
            "sha256": sha256_file(r4_alignment_path),
            "locked": True,
        },
        "verified_formal_files": verified_files,
        "large_frozen_inputs": {
            "audio_sha256_matched_to_r5_lock": True,
            "model_sha256_matched_to_r5_lock": True,
            "rehash_policy": "reused formal producer hashes and R5 input lock; no duplicate full rehash",
        },
        "token_track_origin": token_origin,
    }


def verify_raw_source_and_context(evidence: dict[str, Any]) -> None:
    inputs = require_dict(evidence.get("inputs"), "formal inputs")
    raw_input = require_dict(inputs.get("raw_moss_candidate"), "formal raw candidate")
    source_input = require_dict(inputs.get("source_transcript"), "formal source transcript")
    metadata_input = require_dict(inputs.get("meeting_metadata"), "formal metadata")
    raw_file = load_json(Path(str(raw_input["path"])).resolve(strict=True))
    source_file = load_json(Path(str(source_input["path"])).resolve(strict=True))
    metadata = load_json(Path(str(metadata_input["path"])).resolve(strict=True))

    raw_turns = require_list(raw_file.get("global_turns"), "raw MOSS turns")
    raw_segments = require_list(evidence.get("raw_segments"), "formal raw segments")
    if len(raw_turns) != len(raw_segments) or not raw_segments:
        raise R5ScoreError("formal raw segment count differs from raw MOSS")
    for index, (turn_value, segment_value) in enumerate(zip(raw_turns, raw_segments)):
        turn = require_dict(turn_value, f"raw turn {index}")
        segment = require_dict(segment_value, f"formal raw segment {index}")
        expected = {
            "segment_index": index,
            "start_ms": turn.get("global_start_ms"),
            "end_ms": turn.get("global_end_ms"),
            "speaker_label": turn.get("speaker_label"),
            "text": turn.get("text"),
        }
        if segment != expected:
            raise R5ScoreError("formal raw segments do not exactly reproduce raw MOSS")

    source_segments = require_list(source_file.get("segments"), "source transcript segments")
    snapshot = require_dict(
        evidence.get("source_transcript_snapshot"), "source transcript snapshot"
    )
    anchors = require_list(snapshot.get("anchors"), "source transcript anchors")
    if len(anchors) != len(source_segments) or not anchors:
        raise R5ScoreError("formal source anchor count differs from source transcript")
    for index, (row_value, anchor_value) in enumerate(zip(source_segments, anchors)):
        row = require_dict(row_value, f"source transcript row {index}")
        anchor = require_dict(anchor_value, f"source anchor {index}")
        if row.get("sequence_id") != index:
            raise R5ScoreError("source transcript sequence is not exact")
        expected = {
            "anchor_id": f"source-transcript-{index:06d}",
            "start_ms": math.floor(
                require_number(row.get("audio_start_time"), "source start") * 1000 + 0.5
            ),
            "end_ms": math.floor(
                require_number(row.get("audio_end_time"), "source end") * 1000 + 0.5
            ),
            "text": row.get("text"),
        }
        if anchor != expected:
            raise R5ScoreError("formal source anchor differs from source transcript")

    container = require_dict(metadata.get("meeting_context"), "metadata meeting context")
    if container.get("recording_context_id") != container.get("current_context_id"):
        raise R5ScoreError("metadata current context is not the recording-start context")
    contexts = require_list(container.get("contexts"), "metadata contexts")
    current = next(
        (
            require_dict(item, "metadata context")
            for item in contexts
            if isinstance(item, dict) and item.get("context_id") == container.get("current_context_id")
        ),
        None,
    )
    if current is None or current.get("reason") != "recording_start":
        raise R5ScoreError("pretranscription context is missing")
    formal_context = require_dict(evidence.get("pretranscription_context"), "formal context")
    for name in ("context_id", "revision", "reason", "context_sha256", "source", "terms"):
        if formal_context.get(name) != current.get(name):
            raise R5ScoreError(f"formal pretranscription context differs at {name}")
    if formal_context.get("machine_term_input_only") is not True:
        raise R5ScoreError("formal context role is invalid")


def materialize_candidate(evidence: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    alignment = require_dict(evidence.get("product_alignment"), "product alignment")
    segments = require_list(alignment.get("segments"), "product segments")
    provenance = require_list(alignment.get("provenance"), "product provenance")
    ranges = require_list(alignment.get("piece_raw_ranges"), "piece raw ranges")
    boundaries = require_list(alignment.get("token_boundaries"), "token boundaries")
    suggestions = require_list(
        evidence.get("machine_term_suggestions"), "machine term suggestions"
    )
    if not segments or not (len(segments) == len(provenance) == len(ranges)):
        raise R5ScoreError("product alignment arrays are not exact")
    track = require_dict(evidence.get("audio_token_track"), "audio token track")
    track_hash = require_sha256(track.get("token_track_sha256"), "token track")
    model_hash = require_sha256(track.get("model_sha256"), "token model")
    context = require_dict(evidence.get("pretranscription_context"), "pretranscription context")
    context_hash = require_sha256(context.get("context_sha256"), "context hash")
    terms = {
        str(require_dict(item, "context term").get("term_id")): str(
            require_dict(item, "context term").get("canonical", "")
        )
        for item in require_list(context.get("terms"), "context terms")
    }
    if not terms or any(not key or not canonical for key, canonical in terms.items()):
        raise R5ScoreError("pretranscription terms are invalid")

    boundary_by_segment: dict[int, dict[str, Any]] = {}
    for value in boundaries:
        boundary = require_dict(value, "token boundary")
        segment_index = require_int(boundary.get("segment_index"), "boundary segment", 0)
        if segment_index in boundary_by_segment or segment_index >= len(segments):
            raise R5ScoreError("token boundary segment is invalid or duplicated")
        if require_sha256(boundary.get("token_track_sha256"), "boundary track") != track_hash:
            raise R5ScoreError("token boundary track binding is invalid")
        boundary_by_segment[segment_index] = boundary

    suggestions_by_segment: dict[int, list[dict[str, Any]]] = {}
    term_counts: dict[str, int] = {}
    for value in suggestions:
        suggestion = require_dict(value, "machine term suggestion")
        segment_index = require_int(suggestion.get("segment_index"), "suggestion segment", 0)
        if segment_index >= len(segments):
            raise R5ScoreError("machine suggestion segment is outside the candidate")
        term_id = str(suggestion.get("term_id", ""))
        if terms.get(term_id) != suggestion.get("replacement_text"):
            raise R5ScoreError("machine suggestion is not an exact pretranscription term")
        if (
            require_sha256(suggestion.get("context_sha256"), "suggestion context") != context_hash
            or require_sha256(suggestion.get("token_track_sha256"), "suggestion track") != track_hash
            or require_sha256(suggestion.get("model_sha256"), "suggestion model") != model_hash
        ):
            raise R5ScoreError("machine suggestion binding is invalid")
        boundary = boundary_by_segment.get(segment_index)
        if boundary is None:
            raise R5ScoreError("machine suggestion has no exact token boundary")
        first = require_int(suggestion.get("first_token_index"), "suggestion first token", 0)
        last = require_int(suggestion.get("last_token_index"), "suggestion last token", 0)
        if (
            first > last
            or first < require_int(boundary.get("first_token_index"), "boundary first token")
            or last > require_int(boundary.get("last_token_index"), "boundary last token")
            or require_number(suggestion.get("confidence"), "suggestion confidence") < 0.5
        ):
            raise R5ScoreError("machine suggestion token range or confidence is invalid")
        suggestions_by_segment.setdefault(segment_index, []).append(suggestion)
        term_counts[term_id] = term_counts.get(term_id, 0) + 1

    candidate_segments: list[dict[str, Any]] = []
    applied: list[dict[str, Any]] = []
    for index, (segment_value, proof_value, range_value) in enumerate(
        zip(segments, provenance, ranges)
    ):
        segment = require_dict(segment_value, f"product segment {index}")
        proof = require_dict(proof_value, f"product provenance {index}")
        piece_range = require_dict(range_value, f"piece raw range {index}")
        if (
            require_int(segment.get("segment_index"), "segment index") != index
            or require_int(proof.get("segment_index"), "proof index") != index
            or require_int(piece_range.get("output_segment_index"), "piece index") != index
        ):
            raise R5ScoreError("product indices are not exact")
        start_ms = require_int(segment.get("start_ms"), "segment start", 0)
        end_ms = require_int(segment.get("end_ms"), "segment end", 1)
        speaker = str(segment.get("speaker_label", ""))
        raw_text = str(segment.get("text", ""))
        if end_ms <= start_ms or not speaker or not raw_text.strip():
            raise R5ScoreError("product segment is invalid")
        corrected = raw_text
        local_corrections = sorted(
            suggestions_by_segment.get(index, []),
            key=lambda item: require_int(item.get("start_char"), "suggestion start"),
            reverse=True,
        )
        previous_start = len(raw_text) + 1
        materialized_local: list[dict[str, Any]] = []
        for suggestion in local_corrections:
            start = require_int(suggestion.get("start_char"), "suggestion start", 0)
            end = require_int(suggestion.get("end_char"), "suggestion end", 1)
            original = str(suggestion.get("original_text", ""))
            replacement = str(suggestion.get("replacement_text", ""))
            if end > previous_start or end <= start or end > len(raw_text):
                raise R5ScoreError("machine suggestions overlap or exceed the segment")
            if raw_text[start:end] != original:
                raise R5ScoreError("machine suggestion original text is not exact")
            corrected = corrected[:start] + replacement + corrected[end:]
            previous_start = start
            correction = {
                "segment_index": index,
                "start_char": start,
                "end_char": end,
                "original_text": original,
                "replacement_text": replacement,
                "term_id": suggestion["term_id"],
                "context_sha256": context_hash,
                "token_track_sha256": track_hash,
                "model_sha256": model_hash,
                "first_token_index": suggestion["first_token_index"],
                "last_token_index": suggestion["last_token_index"],
                "confidence": suggestion["confidence"],
            }
            materialized_local.append(correction)
            applied.append(correction)
        materialized_local.reverse()
        candidate_segments.append(
            {
                "source_sequence_id": f"R5-{index + 1:03d}",
                "clip_start_seconds": start_ms / 1000.0,
                "clip_end_seconds": end_ms / 1000.0,
                "speaker_id": speaker,
                "model_speaker_label": speaker,
                "speaker_identity_scope": "candidate_only",
                "text": corrected,
                "raw_machine_text": raw_text,
                "human_checked": False,
                "is_ground_truth": False,
                "alignment": {
                    "method": proof.get("alignment_method"),
                    "confidence": proof.get("confidence"),
                    "raw_segment_index": proof.get("raw_segment_index"),
                    "raw_start_ms": proof.get("raw_start_ms"),
                    "raw_end_ms": proof.get("raw_end_ms"),
                    "raw_text_sha256": proof.get("raw_text_sha256"),
                    "source_anchor_ids": proof.get("source_anchor_ids"),
                    "piece_raw_range": piece_range,
                    "audio_token_boundary": boundary_by_segment.get(index),
                },
                "machine_term_corrections": materialized_local,
            }
        )

    evidence_hash = sha256_file(Path(str(evidence["__evidence_path"])))
    candidate = {
        "schema_version": 1,
        "stage": "MOSS-R5-FORMAL-AUDIO-TOKEN-ALIGNED-CANDIDATE",
        "status": "MACHINE_CANDIDATE_NOT_GROUND_TRUTH",
        "source_formal_evidence_sha256": evidence_hash,
        "source_raw_candidate_sha256": require_dict(
            require_dict(evidence.get("inputs"), "formal inputs").get("raw_moss_candidate"),
            "raw candidate input",
        ).get("sha256"),
        "source_transcript_sha256": require_dict(
            require_dict(evidence.get("inputs"), "formal inputs").get("source_transcript"),
            "source transcript input",
        ).get("sha256"),
        "audio_token_track_sha256": track_hash,
        "pretranscription_context_sha256": context_hash,
        "human_truth_used_for_materialization": False,
        "raw_alignment_text_preservation": "EXACT",
        "machine_term_corrections_reconstructible": True,
        "machine_term_correction_count": len(applied),
        "machine_term_correction_counts_by_term_id": term_counts,
        "turn_count": len(candidate_segments),
        "segments": candidate_segments,
    }
    audit = {
        "candidate_segment_count": len(candidate_segments),
        "machine_term_correction_count": len(applied),
        "machine_term_correction_counts_by_term_id": term_counts,
        "applied_machine_term_corrections": applied,
        "all_suggestions_applied_exactly_once": len(applied) == len(suggestions),
        "human_truth_used": False,
    }
    if not audit["all_suggestions_applied_exactly_once"]:
        raise R5ScoreError("not every formal machine suggestion was materialized")
    return candidate, audit


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fail-closed R5 materialization and scoring for formal MOSS evidence"
    )
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--input-lock", type=Path, required=True)
    parser.add_argument("--r4-alignment-evidence", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--scope", type=Path, required=True)
    parser.add_argument("--verbatim", type=Path, required=True)
    parser.add_argument("--turns", type=Path, required=True)
    parser.add_argument("--review", type=Path, required=True)
    parser.add_argument("--positive-terms", type=Path, required=True)
    parser.add_argument("--negative-blocker", type=Path, required=True)
    parser.add_argument("--term-manifest", type=Path, required=True)
    parser.add_argument("--rules", type=Path, required=True)
    parser.add_argument("--independent-audit", type=Path, required=True)
    parser.add_argument("--frozen-scorer", type=Path, required=True)
    parser.add_argument("--candidate-out", type=Path, required=True)
    parser.add_argument("--score-out", type=Path, required=True)
    return parser.parse_args()


def atomic_write_pair(
    candidate_path: Path,
    candidate_bytes: bytes,
    score_path: Path,
    score_bytes: bytes,
) -> None:
    if candidate_path == score_path or candidate_path.exists() or score_path.exists():
        raise R5ScoreError("R5 output paths must be different and new")
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    score_path.parent.mkdir(parents=True, exist_ok=True)
    candidate_partial = candidate_path.with_name(f".{candidate_path.name}.{os.getpid()}.partial")
    score_partial = score_path.with_name(f".{score_path.name}.{os.getpid()}.partial")
    if candidate_partial.exists() or score_partial.exists():
        raise R5ScoreError("R5 partial output already exists")
    try:
        for path, value in (
            (candidate_partial, candidate_bytes),
            (score_partial, score_bytes),
        ):
            with path.open("xb") as handle:
                handle.write(value)
                handle.flush()
                os.fsync(handle.fileno())
        os.rename(candidate_partial, candidate_path)
        os.rename(score_partial, score_path)
    finally:
        for partial in (candidate_partial, score_partial):
            try:
                partial.unlink()
            except FileNotFoundError:
                pass


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    args = parse_args()
    input_paths = {
        name: value.resolve(strict=True)
        for name, value in vars(args).items()
        if name not in {"candidate_out", "score_out"}
    }
    candidate_path = args.candidate_out.resolve()
    score_path = args.score_out.resolve()

    evidence = load_json(input_paths["evidence"])
    evidence["__evidence_path"] = str(input_paths["evidence"])
    lock = load_json(input_paths["input_lock"])
    r4_alignment = load_json(input_paths["r4_alignment_evidence"])
    formal_bindings = verify_formal_bindings(
        input_paths["evidence"],
        {key: value for key, value in evidence.items() if key != "__evidence_path"},
        input_paths["input_lock"],
        lock,
        input_paths["r4_alignment_evidence"],
        r4_alignment,
    )
    verify_raw_source_and_context(evidence)
    candidate, materialization_audit = materialize_candidate(evidence)

    manifest = load_json(input_paths["manifest"])
    entries = manifest_entries(manifest)
    frozen_inputs = {
        name: verify_manifest_file(entries, input_paths[name])
        for name in (
            "scope",
            "verbatim",
            "turns",
            "review",
            "rules",
            "independent_audit",
            "frozen_scorer",
        )
    }
    frozen_inputs["manifest"] = {
        "path": str(input_paths["manifest"]),
        "bytes": input_paths["manifest"].stat().st_size,
        "sha256": sha256_file(input_paths["manifest"]),
    }

    positive_terms = load_json(input_paths["positive_terms"])
    negative_blocker = load_json(input_paths["negative_blocker"])
    term_manifest = load_json(input_paths["term_manifest"])
    if term_manifest.get("status") != "FROZEN":
        raise R5ScoreError("term evidence manifest is not frozen")
    term_entries = require_list(term_manifest.get("entries"), "term manifest entries")
    term_roles = {
        "positive_terms": "evidence:F2-terms/01-local-positive-terms-frozen.json",
        "negative_blocker": "evidence:F2-terms/02-negative-term-review-blocker.json",
    }
    for name, role in term_roles.items():
        row = next(
            (
                require_dict(item, "term manifest entry")
                for item in term_entries
                if isinstance(item, dict) and item.get("role") == role
            ),
            None,
        )
        if row is None:
            raise R5ScoreError(f"term manifest does not contain {name}")
        actual_hash = sha256_file(input_paths[name])
        if (
            actual_hash != require_sha256(row.get("sha256"), f"term manifest {name}")
            or input_paths[name].stat().st_size
            != require_int(row.get("bytes"), f"term manifest bytes {name}", 0)
        ):
            raise R5ScoreError(f"term manifest does not bind {name}")
        frozen_inputs[name] = {
            "path": str(input_paths[name]),
            "bytes": input_paths[name].stat().st_size,
            "sha256": actual_hash,
            "frozen_term_manifest_verified": True,
        }
    frozen_inputs["term_manifest"] = {
        "path": str(input_paths["term_manifest"]),
        "bytes": input_paths["term_manifest"].stat().st_size,
        "sha256": sha256_file(input_paths["term_manifest"]),
        "status": "FROZEN",
    }

    scope = load_json(input_paths["scope"])
    review = load_json(input_paths["review"])
    rules = load_json(input_paths["rules"])
    independent_audit = load_json(input_paths["independent_audit"])
    if review.get("approved_as_ground_truth") is not True:
        raise R5ScoreError("human review is not approved")
    if require_dict(review.get("playback_coverage"), "playback coverage").get("complete") is not True:
        raise R5ScoreError("human review playback is incomplete")
    if independent_audit.get("structural_status") != "PASS":
        raise R5ScoreError("independent human-truth audit is not PASS")

    positive_bindings = require_dict(
        positive_terms.get("source_bindings"), "positive term source bindings"
    )
    if positive_terms.get("status") != "HUMAN_TRUTH_DERIVED_POSITIVE_FROZEN":
        raise R5ScoreError("positive terms are not the frozen human-reviewed set")
    for name, key in (
        ("verbatim", "human_verbatim_sha256"),
        ("review", "human_review_sha256"),
        ("rules", "scoring_rules_sha256"),
    ):
        if sha256_file(input_paths[name]) != require_sha256(
            positive_bindings.get(key), f"positive term binding {name}"
        ):
            raise R5ScoreError(f"positive term truth is not bound to {name}")

    frozen = load_frozen_scorer(input_paths["frozen_scorer"])
    window = require_dict(scope.get("reference_window"), "reference window")
    source = require_dict(scope.get("source"), "scope source")
    window_start = require_number(window.get("source_start_seconds"), "window start")
    window_end = require_number(window.get("source_end_seconds"), "window end")
    window_duration = require_number(window.get("duration_seconds"), "window duration")
    full_duration = require_number(source.get("duration_seconds"), "full duration")
    if not math.isclose(window_end - window_start, window_duration, abs_tol=1e-6):
        raise R5ScoreError("scope window duration is inconsistent")

    full_segments = frozen.parse_hypothesis_segments(candidate)
    timestamp_validation = frozen.validate_prediction_timestamps(full_segments, full_duration)
    if not any(str(segment.get("speaker", "")).strip() for segment in full_segments):
        raise R5ScoreError("R5 candidate has no scorer-visible speaker labels")
    window_segments = frozen.derive_window_segments(full_segments, window_start, window_end)
    truth_segments = frozen.truth_segments_from_tsv(input_paths["verbatim"])
    truth_turn_data = frozen.truth_turn_data_from_tsv(input_paths["turns"])
    reference_text = "".join(str(segment["text"]) for segment in truth_segments)
    hypothesis_text = "".join(str(segment["text"]) for segment in window_segments)
    cer_result = frozen.cer(reference_text, hypothesis_text)
    speaker_result = frozen.score_speaker_turns(
        truth_turn_data["scored_turns"],
        window_segments,
        truth_turn_data["ignore_intervals"],
    )
    boundary_result = frozen.boundary_spill_metrics(full_segments, window_start, window_end)
    full_timeline = frozen.output_timeline_metrics(full_segments)
    positive_targets = require_list(
        positive_terms.get("positive_spoken_terms"), "positive term targets"
    )
    if len(positive_targets) < 3:
        raise R5ScoreError("positive term target set is incomplete")
    positive_result = frozen.score_positive_terms_by_alignment(
        truth_segments, window_segments, positive_targets
    )
    positive_pass = (
        int(positive_result["target_count"]) == len(positive_targets)
        and int(positive_result["exact_occurrence_target_count"]) == len(positive_targets)
    )

    negative_human_ready = (
        negative_blocker.get("current_human_attestation_present") is True
        and isinstance(negative_blocker.get("negative_unspoken_terms"), list)
        and len(negative_blocker["negative_unspoken_terms"]) >= 2
    )
    negative_reason = (
        "confirmed negative schema still lacks a separately frozen R5 scoring adapter"
        if negative_human_ready
        else "full-audio human negative-term attestation is still missing"
    )

    candidate_bytes = json_bytes(candidate, pretty=True)
    candidate_hash = sha256_bytes(candidate_bytes)
    audio_alignment = require_dict(evidence.get("product_alignment"), "product alignment")
    coverage = require_number(
        audio_alignment.get("token_global_match_coverage"), "token global match coverage"
    )
    token_aligned_count = require_int(
        audio_alignment.get("token_aligned_segment_count"), "token-aligned count", 0
    )
    suggestion_count = require_int(
        candidate.get("machine_term_correction_count"), "machine correction count", 0
    )
    maximum_spill = float(rules["window_derivation"]["maximum_boundary_spill_seconds"])
    cer_maximum = float(rules["cer"]["moss_cer_absolute_maximum"])
    speaker_rules = rules["speaker_scoring"]
    full_rules = rules["full_run_gates"]
    activity = require_dict(r4_alignment.get("objective_audio_timing"), "objective audio timing")
    last_active_seconds = require_number(activity.get("last_active_ms"), "last active ms") / 1000.0
    tolerance_seconds = (
        require_number(
            require_dict(r4_alignment.get("product_constants"), "R4 product constants").get(
                "alignment_time_tolerance_ms"
            ),
            "alignment tolerance",
        )
        / 1000.0
    )
    last_output_seconds = float(full_timeline["last_segment_end_seconds"])
    tail_delta_seconds = last_output_seconds - last_active_seconds
    tail_pass = 0.0 <= tail_delta_seconds <= tolerance_seconds

    gates = {
        "formal_r5_structure": gate("PASS", True, True),
        "formal_payload_hash": gate("PASS", evidence["payload_sha256"], evidence["payload_sha256"]),
        "raw_alignment_text_preserved": gate("PASS", True, True),
        "machine_candidate_reconstructible": gate(
            "PASS" if materialization_audit["all_suggestions_applied_exactly_once"] else "FAIL",
            materialization_audit["all_suggestions_applied_exactly_once"],
            True,
        ),
        "audio_token_global_coverage": gate(
            "PASS" if coverage >= 0.5 else "FAIL", coverage, {"minimum": 0.5}
        ),
        "audio_token_alignment_exercised": gate(
            "PASS" if token_aligned_count > 0 else "FAIL",
            token_aligned_count,
            {"minimum": 1},
        ),
        "machine_term_correction_exercised": gate(
            "PASS" if suggestion_count > 0 else "FAIL",
            suggestion_count,
            {"minimum": 1},
        ),
        "candidate_hash_binding": gate("PASS", candidate_hash, candidate_hash),
        "timestamps_valid": gate(
            "PASS" if timestamp_validation["valid"] else "FAIL",
            timestamp_validation["invalid_segment_count"],
            0,
        ),
        "window_boundary_spill": gate(
            "PASS" if boundary_result["maximum_spill_seconds"] <= maximum_spill else "FAIL",
            boundary_result["maximum_spill_seconds"],
            {"maximum_seconds": maximum_spill},
        ),
        "cer": gate(
            "PASS" if float(cer_result["cer"]) <= cer_maximum else "FAIL",
            cer_result["cer"],
            {"maximum": cer_maximum},
        ),
        "speaker_turn_error_rate": gate(
            "PASS"
            if float(speaker_result["error_rate"])
            <= float(speaker_rules["gate_max_error_rate"])
            else "FAIL",
            speaker_result["error_rate"],
            {"maximum": speaker_rules["gate_max_error_rate"]},
        ),
        "speaker_duration_error_rate": gate(
            "PASS"
            if float(speaker_result["duration_error_rate"])
            <= float(speaker_rules["gate_max_duration_error_rate"])
            else "FAIL",
            speaker_result["duration_error_rate"],
            {"maximum": speaker_rules["gate_max_duration_error_rate"]},
        ),
        "speaker_false_alarm_seconds": gate(
            "PASS"
            if float(speaker_result["false_alarm_seconds"])
            <= float(speaker_rules["gate_max_false_alarm_seconds"])
            else "FAIL",
            speaker_result["false_alarm_seconds"],
            {"maximum": speaker_rules["gate_max_false_alarm_seconds"]},
        ),
        "speaker_false_alarm_rate": gate(
            "PASS"
            if float(speaker_result["false_alarm_rate"])
            <= float(speaker_rules["gate_max_false_alarm_rate"])
            else "FAIL",
            speaker_result["false_alarm_rate"],
            {"maximum": speaker_rules["gate_max_false_alarm_rate"]},
        ),
        "positive_terms_exact": gate(
            "PASS" if positive_pass else "FAIL",
            positive_result["exact_occurrence_target_count"],
            {"target_count": len(positive_targets)},
        ),
        "negative_terms_no_insertion": gate(
            "NOT_RUN",
            negative_reason,
            {"minimum_human_confirmed_unspoken_terms": 2},
        ),
        "r5_effective_audio_tail": gate(
            "PASS" if tail_pass else "FAIL",
            {
                "last_output_seconds": last_output_seconds,
                "last_active_seconds": last_active_seconds,
                "delta_seconds": tail_delta_seconds,
            },
            {"minimum_delta_seconds": 0.0, "maximum_delta_seconds": tolerance_seconds},
        ),
        "legacy_r3_last_segment_end": gate(
            "PASS"
            if last_output_seconds >= float(full_rules["minimum_last_segment_end_seconds"])
            else "FAIL",
            last_output_seconds,
            {"minimum": full_rules["minimum_last_segment_end_seconds"]},
        ),
        "full_speech_union": gate(
            "PASS"
            if float(full_timeline["speech_union_seconds"])
            >= float(full_rules["minimum_output_speech_union_seconds"])
            else "FAIL",
            full_timeline["speech_union_seconds"],
            {"minimum": full_rules["minimum_output_speech_union_seconds"]},
        ),
        "maximum_segment_duration": gate(
            "PASS"
            if float(full_timeline["maximum_segment_duration_seconds"])
            <= float(full_rules["maximum_segment_duration_seconds"])
            else "FAIL",
            full_timeline["maximum_segment_duration_seconds"],
            {"maximum": full_rules["maximum_segment_duration_seconds"]},
        ),
    }
    all_required_pass = all(item["status"] == "PASS" for item in gates.values())
    report = {
        "schema_version": 1,
        "stage": "MOSS-R5-FORMAL-PRODUCT-CANDIDATE-HUMAN-TRUTH-SCORE",
        "status": "PASS" if all_required_pass else "NO-GO",
        "generated_by": "scripts/qa/moss_r5_materialize_and_score.py",
        "truth_boundary": (
            "Formal alignment and machine term materialization completed before frozen human truth "
            "was loaded. Human truth is used only for this final score."
        ),
        "inputs": {
            "formal_bindings": formal_bindings,
            "frozen_human_truth": frozen_inputs,
            "candidate": {
                "path": str(candidate_path),
                "bytes": len(candidate_bytes),
                "sha256": candidate_hash,
                "role": "MACHINE_CANDIDATE_NOT_GROUND_TRUTH",
            },
        },
        "materialization": materialization_audit,
        "scope": {
            "window_start_seconds": window_start,
            "window_end_seconds": window_end,
            "window_duration_seconds": window_duration,
            "full_duration_seconds": full_duration,
        },
        "metrics": {
            "cer": cer_result,
            "speaker": speaker_result,
            "boundary_spill": boundary_result,
            "full_timeline": full_timeline,
            "timestamp_validation": timestamp_validation,
            "terms": {
                "positive": positive_result,
                "positive_status": "PASS" if positive_pass else "FAIL",
                "negative": {
                    "status": "NOT_RUN",
                    "reason": negative_reason,
                    "machine_search_is_not_human_confirmation": True,
                },
            },
            "audio_token_alignment": {
                "global_match_coverage": coverage,
                "token_aligned_segment_count": token_aligned_count,
                "fallback_raw_segment_count": audio_alignment.get(
                    "token_fallback_raw_segment_count"
                ),
                "fallback_reason": audio_alignment.get("token_fallback_reason"),
                "token_track_sha256": audio_alignment.get("token_track_sha256"),
            },
        },
        "gates": gates,
        "decision": {
            "r5_product_candidate": "PASS" if all_required_pass else "NO-GO",
            "raw_moss_gate": "NO-GO_PRESERVED_FROM_R3",
            "formal_release": (
                "PENDING_RELEASE_SIGNING_AND_PACKAGING" if all_required_pass else "NO-GO"
            ),
            "failed_or_not_run_gates": [
                name for name, item in gates.items() if item["status"] != "PASS"
            ],
            "stable_executable_replacement_authorized": False,
        },
    }
    score_bytes = json_bytes(report, pretty=True)
    atomic_write_pair(candidate_path, candidate_bytes, score_path, score_bytes)
    print(f"R5_CANDIDATE={candidate_path}")
    print(f"R5_CANDIDATE_SHA256={candidate_hash}")
    print(f"R5_SCORE={score_path}")
    print(f"R5_SCORE_STATUS={report['status']}")
    print(
        "R5_FAILED_OR_NOT_RUN="
        + ",".join(report["decision"]["failed_or_not_run_gates"])
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except R5ScoreError as error:
        print(f"R5_SCORE_ERROR={error}", file=sys.stderr)
        raise SystemExit(2)
