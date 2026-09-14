#!/usr/bin/env python3
"""Score an R4 product-aligned candidate without changing frozen R3 evidence."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import sys
from pathlib import Path
from types import ModuleType
from typing import Any


class R4ScoreError(RuntimeError):
    pass


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise R4ScoreError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(
            path.read_text(encoding="utf-8-sig"),
            object_pairs_hook=strict_object,
            parse_constant=lambda item: (_ for _ in ()).throw(
                R4ScoreError(f"invalid JSON number: {item}")
            ),
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise R4ScoreError(f"cannot read JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise R4ScoreError(f"JSON root is not an object: {path}")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def load_frozen_scorer(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("moss_r4_frozen_scorer", path)
    if spec is None or spec.loader is None:
        raise R4ScoreError("cannot load frozen scorer")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def manifest_entries(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    rows = manifest.get("files")
    if not isinstance(rows, list):
        raise R4ScoreError("manifest files is not an array")
    result: dict[str, dict[str, Any]] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise R4ScoreError("manifest contains a non-object entry")
        relative_path = str(row.get("relative_path", ""))
        if not relative_path or relative_path in result:
            raise R4ScoreError("manifest contains an invalid or duplicate path")
        result[relative_path] = row
    return result


def verify_manifest_file(
    entries: dict[str, dict[str, Any]], path: Path
) -> dict[str, Any]:
    row = entries.get(path.name)
    if row is None:
        raise R4ScoreError(f"file is not present in frozen manifest: {path.name}")
    actual_hash = sha256_file(path)
    actual_bytes = path.stat().st_size
    expected_hash = str(row.get("sha256", "")).upper()
    expected_bytes = int(row.get("bytes", -1))
    if actual_hash != expected_hash or actual_bytes != expected_bytes:
        raise R4ScoreError(f"frozen file changed: {path.name}")
    return {
        "path": str(path),
        "bytes": actual_bytes,
        "sha256": actual_hash,
        "manifest_verified": True,
    }


def gate(status: str, actual: Any, threshold: Any = None) -> dict[str, Any]:
    if status not in {"PASS", "FAIL", "NOT_RUN"}:
        raise R4ScoreError(f"invalid gate status: {status}")
    value: dict[str, Any] = {"status": status, "actual": actual}
    if threshold is not None:
        value["threshold"] = threshold
    return value


def require_bool(value: Any, label: str) -> bool:
    if not isinstance(value, bool):
        raise R4ScoreError(f"{label} is not boolean")
    return value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Fail-closed R4 scoring for a product-aligned MOSS candidate"
    )
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--alignment-evidence", type=Path, required=True)
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
    parser.add_argument("--out", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    args = parse_args()
    paths = {
        name: value.resolve(strict=True)
        for name, value in vars(args).items()
        if name != "out"
    }
    output_path = args.out.resolve()

    manifest = load_json(paths["manifest"])
    entries = manifest_entries(manifest)
    frozen_inputs = {
        name: verify_manifest_file(entries, paths[name])
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
        "path": str(paths["manifest"]),
        "bytes": paths["manifest"].stat().st_size,
        "sha256": sha256_file(paths["manifest"]),
    }

    candidate = load_json(paths["candidate"])
    alignment = load_json(paths["alignment_evidence"])
    scope = load_json(paths["scope"])
    review = load_json(paths["review"])
    positive_terms = load_json(paths["positive_terms"])
    negative_blocker = load_json(paths["negative_blocker"])
    term_manifest = load_json(paths["term_manifest"])
    rules = load_json(paths["rules"])
    independent_audit = load_json(paths["independent_audit"])
    frozen = load_frozen_scorer(paths["frozen_scorer"])

    if term_manifest.get("status") != "FROZEN" or not isinstance(
        term_manifest.get("entries"), list
    ):
        raise R4ScoreError("term evidence manifest is not frozen")
    term_roles = {
        "positive_terms": "evidence:F2-terms/01-local-positive-terms-frozen.json",
        "negative_blocker": "evidence:F2-terms/02-negative-term-review-blocker.json",
    }
    for name, role in term_roles.items():
        row = next(
            (
                item
                for item in term_manifest["entries"]
                if isinstance(item, dict) and item.get("role") == role
            ),
            None,
        )
        if row is None:
            raise R4ScoreError(f"term manifest does not contain {name}")
        actual_hash = sha256_file(paths[name])
        expected_hash = str(row.get("sha256", "")).upper()
        expected_bytes = int(row.get("bytes", -1))
        if actual_hash != expected_hash or paths[name].stat().st_size != expected_bytes:
            raise R4ScoreError(f"term manifest does not bind {name}")
        frozen_inputs[name] = {
            "path": str(paths[name]),
            "bytes": paths[name].stat().st_size,
            "sha256": actual_hash,
            "frozen_term_manifest_verified": True,
        }
    frozen_inputs["term_manifest"] = {
        "path": str(paths["term_manifest"]),
        "bytes": paths["term_manifest"].stat().st_size,
        "sha256": sha256_file(paths["term_manifest"]),
        "status": "FROZEN",
    }

    candidate_hash = sha256_file(paths["candidate"])
    expected_candidate_hash = str(
        alignment.get("alignment_result", {}).get("candidate_sha256", "")
    ).upper()
    if candidate_hash != expected_candidate_hash:
        raise R4ScoreError("candidate hash does not match alignment evidence")
    if candidate.get("status") != "MACHINE_CANDIDATE_NOT_GROUND_TRUTH":
        raise R4ScoreError("candidate is not explicitly marked as machine-only")
    if alignment.get("decision", {}).get("r4_structural_evidence") != "PASS":
        raise R4ScoreError("R4 structural evidence is not PASS")
    if review.get("approved_as_ground_truth") is not True:
        raise R4ScoreError("human review is not approved")
    if review.get("playback_coverage", {}).get("complete") is not True:
        raise R4ScoreError("human review playback is incomplete")
    if independent_audit.get("structural_status") != "PASS":
        raise R4ScoreError("independent human-truth audit is not PASS")

    window = scope.get("reference_window")
    source = scope.get("source")
    if not isinstance(window, dict) or not isinstance(source, dict):
        raise R4ScoreError("scope lacks source or reference window")
    window_start = float(window["source_start_seconds"])
    window_end = float(window["source_end_seconds"])
    window_duration = float(window["duration_seconds"])
    full_duration = float(source["duration_seconds"])
    if not math.isclose(window_end - window_start, window_duration, abs_tol=1e-6):
        raise R4ScoreError("scope window duration is inconsistent")

    full_segments = frozen.parse_hypothesis_segments(candidate)
    timestamp_validation = frozen.validate_prediction_timestamps(
        full_segments, full_duration
    )
    if not any(str(segment.get("speaker", "")).strip() for segment in full_segments):
        raise R4ScoreError("candidate has no scorer-visible speaker labels")
    window_segments = frozen.derive_window_segments(
        full_segments, window_start, window_end
    )
    truth_segments = frozen.truth_segments_from_tsv(paths["verbatim"])
    truth_turn_data = frozen.truth_turn_data_from_tsv(paths["turns"])
    reference_text = "".join(str(segment["text"]) for segment in truth_segments)
    hypothesis_text = "".join(str(segment["text"]) for segment in window_segments)
    cer_result = frozen.cer(reference_text, hypothesis_text)
    speaker_result = frozen.score_speaker_turns(
        truth_turn_data["scored_turns"],
        window_segments,
        truth_turn_data["ignore_intervals"],
    )
    boundary_result = frozen.boundary_spill_metrics(
        full_segments, window_start, window_end
    )
    full_timeline = frozen.output_timeline_metrics(full_segments)

    cer_maximum = float(rules["cer"]["moss_cer_absolute_maximum"])
    speaker_rules = rules["speaker_scoring"]
    full_rules = rules["full_run_gates"]
    maximum_spill = float(rules["window_derivation"]["maximum_boundary_spill_seconds"])
    activity = alignment["objective_audio_timing"]
    last_active_seconds = float(activity["last_active_ms"]) / 1000.0
    alignment_tolerance_seconds = (
        float(alignment["product_constants"]["alignment_time_tolerance_ms"]) / 1000.0
    )
    last_output_seconds = float(full_timeline["last_segment_end_seconds"])
    tail_delta_seconds = last_output_seconds - last_active_seconds
    effective_tail_pass = 0.0 <= tail_delta_seconds <= alignment_tolerance_seconds

    positive_targets = positive_terms.get("positive_spoken_terms")
    if (
        positive_terms.get("status") != "HUMAN_TRUTH_DERIVED_POSITIVE_FROZEN"
        or not isinstance(positive_targets, list)
        or len(positive_targets) < 3
    ):
        raise R4ScoreError("positive term truth is not the frozen human-reviewed set")
    positive_source_bindings = positive_terms.get("source_bindings")
    if not isinstance(positive_source_bindings, dict):
        raise R4ScoreError("positive term truth lacks source bindings")
    for name, binding_key in (
        ("verbatim", "human_verbatim_sha256"),
        ("review", "human_review_sha256"),
        ("rules", "scoring_rules_sha256"),
    ):
        if sha256_file(paths[name]) != str(
            positive_source_bindings.get(binding_key, "")
        ).upper():
            raise R4ScoreError(f"positive term truth is not bound to {name}")
    positive_term_result = frozen.score_positive_terms_by_alignment(
        truth_segments,
        window_segments,
        positive_targets,
    )
    positive_term_pass = (
        int(positive_term_result["target_count"]) == len(positive_targets)
        and int(positive_term_result["exact_occurrence_target_count"])
        == len(positive_targets)
    )
    negative_human_ready = (
        negative_blocker.get("current_human_attestation_present") is True
        and isinstance(negative_blocker.get("negative_unspoken_terms"), list)
        and len(negative_blocker["negative_unspoken_terms"]) >= 2
    )
    if negative_human_ready:
        negative_term_result: dict[str, Any] = {
            "status": "NOT_RUN",
            "reason": "confirmed negative schema requires a separately frozen R4 scorer adapter",
        }
    else:
        negative_term_result = {
            "status": "NOT_RUN",
            "reason": "full-audio human negative-term attestation is still missing",
            "machine_search_is_not_human_confirmation": True,
        }
    term_result = {
        "positive": positive_term_result,
        "positive_status": "PASS" if positive_term_pass else "FAIL",
        "negative": negative_term_result,
    }

    gates = {
        "alignment_structure": gate("PASS", True, True),
        "candidate_hash_binding": gate("PASS", candidate_hash, expected_candidate_hash),
        "text_preserved_exactly": gate(
            "PASS"
            if require_bool(
                alignment["invariants"]["text_preserved_exactly"],
                "text_preserved_exactly",
            )
            else "FAIL",
            alignment["invariants"]["text_preserved_exactly"],
            True,
        ),
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
            "PASS" if positive_term_pass else "FAIL",
            positive_term_result["exact_occurrence_target_count"],
            {"target_count": len(positive_targets)},
        ),
        "negative_terms_no_insertion": gate(
            "NOT_RUN",
            negative_term_result["reason"],
            {"minimum_human_confirmed_unspoken_terms": 2},
        ),
        "r4_effective_audio_tail": gate(
            "PASS" if effective_tail_pass else "FAIL",
            {"last_output_seconds": last_output_seconds, "last_active_seconds": last_active_seconds, "delta_seconds": tail_delta_seconds},
            {"minimum_delta_seconds": 0.0, "maximum_delta_seconds": alignment_tolerance_seconds},
        ),
        "legacy_r3_last_segment_end": gate(
            "PASS"
            if last_output_seconds
            >= float(full_rules["minimum_last_segment_end_seconds"])
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
        "stage": "MOSS-R4-PRODUCT-CANDIDATE-HUMAN-TRUTH-SCORE",
        "status": "PASS" if all_required_pass else "NO-GO",
        "generated_by": "scripts/qa/moss_r4_score_product_candidate.py",
        "truth_boundary": (
            "The frozen human transcript is used only for scoring. The runtime alignment used the "
            "separately hash-bound machine source transcript and did not consume human truth."
        ),
        "inputs": {
            "frozen_manifest": frozen_inputs,
            "candidate": {
                "path": str(paths["candidate"]),
                "sha256": candidate_hash,
                "alignment_evidence_hash_verified": True,
                "role": "MACHINE_CANDIDATE_NOT_GROUND_TRUTH",
            },
            "alignment_evidence": {
                "path": str(paths["alignment_evidence"]),
                "sha256": sha256_file(paths["alignment_evidence"]),
                "structural_status": "PASS",
            },
        },
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
            "terms": term_result,
        },
        "gates": gates,
        "decision": {
            "r4_product_candidate": "PASS" if all_required_pass else "NO-GO",
            "raw_moss_gate": "NO-GO_PRESERVED_FROM_R3",
            "formal_release": "NO-GO" if not all_required_pass else "PENDING_RELEASE_SIGNING_AND_PACKAGING",
            "failed_or_not_run_gates": [
                name for name, item in gates.items() if item["status"] != "PASS"
            ],
        },
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"R4_SCORE={output_path}")
    print(f"R4_SCORE_STATUS={report['status']}")
    print(
        "R4_FAILED_OR_NOT_RUN="
        + ",".join(report["decision"]["failed_or_not_run_gates"])
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
