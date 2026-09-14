#!/usr/bin/env python3
"""Bind the frozen R2 monolithic MOSS output to the frozen P0-R score inputs.

The derived manifest intentionally keeps the historical compatibility role name
``moss_p1_private_output`` because the frozen F3/F4 scorers require that role.
The public binding report records the real source stage so that the alias cannot
be mistaken for a chunked run.
"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


MOSS_ROLE = "moss_p1_private_output"
EXPECTED_STAGE = "R1_MONOLITHIC_VULKAN16K"
EXPECTED_TURN_COUNT = 67
EXPECTED_SPEAKERS = ["S01", "S02", "S03", "S04", "S05"]
SCORING_REQUIRED_ROLES = {
    "formal_whisper_full_output",
    "formal_whisper_run_record",
    "human_review",
    "human_speaker_turns",
    "human_verbatim",
    MOSS_ROLE,
    "network_recovery_audit",
    "pre_meeting_context",
    "reference_window_audio",
    "scoring_rules_draft",
    "source_full_audio",
    "strict_s8_cuda_scorer",
}
PERMITTED_NON_SCORING_DRIFT = {"closeout_plan"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_sha256(value: Any) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(payload, dict):
        raise ValueError(f"json_root_must_be_object:{path}")
    return payload


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    temporary.replace(path)


def write_sha256_sidecar(path: Path) -> str:
    digest = sha256_file(path)
    sidecar = path.with_suffix(path.suffix + ".sha256")
    sidecar.write_text(
        f"{digest.upper()}  {path.name}\n", encoding="ascii", newline="\n"
    )
    return digest


def normalize_sha256(value: str) -> str:
    normalized = value.strip().lower()
    if len(normalized) != 64 or any(char not in "0123456789abcdef" for char in normalized):
        raise ValueError("expected_moss_sha256_invalid")
    return normalized


def validate_manifest(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    entries = manifest.get("entries")
    if not isinstance(entries, list) or not entries:
        raise ValueError("manifest_entries_missing_or_empty")
    if manifest.get("entry_count") != len(entries):
        raise ValueError("manifest_entry_count_mismatch")

    roles: list[str] = []
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            raise ValueError(f"manifest_entry_not_object:{index}")
        role = entry.get("role")
        if not isinstance(role, str) or not role:
            raise ValueError(f"manifest_entry_role_invalid:{index}")
        roles.append(role)
        path = entry.get("path")
        size = entry.get("bytes")
        digest = entry.get("sha256")
        if not isinstance(path, str) or not path:
            raise ValueError(f"manifest_entry_path_invalid:{role}")
        if not isinstance(size, int) or size < 0:
            raise ValueError(f"manifest_entry_bytes_invalid:{role}")
        if not isinstance(digest, str):
            raise ValueError(f"manifest_entry_sha256_invalid:{role}")
        normalize_sha256(digest)

    duplicate_roles = sorted({role for role in roles if roles.count(role) > 1})
    if duplicate_roles:
        raise ValueError(f"manifest_duplicate_roles:{','.join(duplicate_roles)}")
    if roles.count(MOSS_ROLE) != 1:
        raise ValueError(f"manifest_requires_exactly_one_role:{MOSS_ROLE}")
    return entries


def rehash_entries(
    entries: list[dict[str, Any]],
) -> tuple[dict[str, dict[str, Any]], list[str]]:
    audit: dict[str, dict[str, Any]] = {}
    mismatches: list[str] = []
    for entry in entries:
        role = str(entry["role"])
        path = Path(str(entry["path"]))
        exists = path.is_file()
        actual_bytes = path.stat().st_size if exists else None
        actual_sha256 = sha256_file(path) if exists else None
        matches = (
            exists
            and actual_bytes == entry["bytes"]
            and actual_sha256 == str(entry["sha256"]).lower()
        )
        audit[role] = {
            "exists": exists,
            "expected_bytes": entry["bytes"],
            "actual_bytes": actual_bytes,
            "expected_sha256": str(entry["sha256"]).lower(),
            "actual_sha256": actual_sha256,
            "matches": matches,
        }
        if not matches:
            mismatches.append(role)
    return audit, mismatches


def validate_rehash_scope(
    *,
    roles: set[str],
    mismatches: list[str],
    phase: str,
) -> list[str]:
    missing_scoring_roles = sorted(SCORING_REQUIRED_ROLES - roles)
    if missing_scoring_roles:
        raise ValueError(
            f"{phase}_missing_scoring_roles:{','.join(missing_scoring_roles)}"
        )
    scoring_mismatches = sorted(set(mismatches) & SCORING_REQUIRED_ROLES)
    if scoring_mismatches:
        raise ValueError(
            f"{phase}_scoring_input_rehash_failed:{','.join(scoring_mismatches)}"
        )
    unexpected_non_scoring_drift = sorted(
        set(mismatches) - PERMITTED_NON_SCORING_DRIFT
    )
    if unexpected_non_scoring_drift:
        raise ValueError(
            f"{phase}_unexpected_non_scoring_drift:"
            f"{','.join(unexpected_non_scoring_drift)}"
        )
    return sorted(mismatches)


def validate_monolithic_adapter(
    payload: dict[str, Any],
    *,
    actual_sha256: str,
    expected_sha256: str,
) -> dict[str, Any]:
    if actual_sha256 != expected_sha256:
        raise ValueError(
            f"moss_output_hash_mismatch:expected={expected_sha256}:actual={actual_sha256}"
        )
    if payload.get("schema_version") != 1:
        raise ValueError("moss_output_schema_version_mismatch")
    if payload.get("stage") != EXPECTED_STAGE:
        raise ValueError("moss_output_stage_mismatch")
    if payload.get("run_state") != "COMPLETED":
        raise ValueError("moss_output_run_not_completed")
    if payload.get("release_go") is not False:
        raise ValueError("moss_output_release_go_must_remain_false")
    if payload.get("cross_chunk_speaker_identity_proven") is not True:
        raise ValueError("moss_output_speaker_identity_scope_not_proven")

    turns = payload.get("global_turns")
    if not isinstance(turns, list) or len(turns) != EXPECTED_TURN_COUNT:
        raise ValueError("moss_output_turn_count_mismatch")
    if any(not isinstance(turn, dict) for turn in turns):
        raise ValueError("moss_output_turn_not_object")

    chunk_indices = sorted({turn.get("chunk_index") for turn in turns})
    if chunk_indices != [0]:
        raise ValueError("moss_output_not_single_chunk_zero")
    speakers = sorted(
        {
            str(turn.get("speaker_label", "")).strip()
            for turn in turns
            if str(turn.get("speaker_label", "")).strip()
        }
    )
    if speakers != EXPECTED_SPEAKERS:
        raise ValueError("moss_output_speaker_labels_mismatch")

    for index, turn in enumerate(turns):
        start_ms = turn.get("global_start_ms")
        end_ms = turn.get("global_end_ms")
        text = turn.get("text")
        if not isinstance(start_ms, int) or not isinstance(end_ms, int):
            raise ValueError(f"moss_output_turn_timestamp_invalid:{index}")
        if start_ms < 0 or end_ms <= start_ms:
            raise ValueError(f"moss_output_turn_timestamp_order_invalid:{index}")
        if not isinstance(text, str):
            raise ValueError(f"moss_output_turn_text_invalid:{index}")

    global_gate = payload.get("global_gate")
    if not isinstance(global_gate, dict) or global_gate.get("state") != "PASS":
        raise ValueError("moss_output_global_gate_not_pass")
    total_rtf = global_gate.get("total_rtf")
    if not isinstance(total_rtf, (int, float)) or isinstance(total_rtf, bool):
        raise ValueError("moss_output_total_rtf_invalid")
    if not math.isfinite(float(total_rtf)) or not (0 <= float(total_rtf) < 1):
        raise ValueError("moss_output_total_rtf_not_below_one")
    if global_gate.get("source_structural_status") != "PASS":
        raise ValueError("moss_output_source_structure_not_pass")

    source_binding = payload.get("source_binding")
    if not isinstance(source_binding, dict):
        raise ValueError("moss_output_source_binding_missing")
    configuration = source_binding.get("configuration")
    if not isinstance(configuration, dict):
        raise ValueError("moss_output_configuration_missing")
    if configuration.get("n_ctx") != 16_384:
        raise ValueError("moss_output_n_ctx_mismatch")
    if configuration.get("backend") != "vulkan":
        raise ValueError("moss_output_backend_mismatch")

    first_start_ms = min(turn["global_start_ms"] for turn in turns)
    last_end_ms = max(turn["global_end_ms"] for turn in turns)
    return {
        "schema_version": payload["schema_version"],
        "stage": payload["stage"],
        "run_state": payload["run_state"],
        "release_go": payload["release_go"],
        "turn_count": len(turns),
        "chunk_indices": chunk_indices,
        "speaker_labels": speakers,
        "speaker_label_count": len(speakers),
        "cross_chunk_speaker_identity_proven": True,
        "global_gate_state": global_gate["state"],
        "source_structural_status": global_gate["source_structural_status"],
        "total_rtf": float(total_rtf),
        "first_segment_start_ms": first_start_ms,
        "last_segment_end_ms": last_end_ms,
        "backend": configuration["backend"],
        "n_ctx": configuration["n_ctx"],
        "model_sha256": str(source_binding.get("model_sha256", "")).upper(),
        "audio_sha256": str(source_binding.get("audio_sha256", "")).upper(),
    }


def derive_binding(
    *,
    source_manifest_path: Path,
    moss_output_path: Path,
    expected_moss_sha256: str,
    private_manifest_path: Path,
    private_derivation_path: Path,
    public_binding_path: Path,
) -> dict[str, Any]:
    source_manifest_path = source_manifest_path.resolve()
    moss_output_path = moss_output_path.resolve()
    expected_moss_sha256 = normalize_sha256(expected_moss_sha256)

    if not source_manifest_path.is_file():
        raise FileNotFoundError(f"source_manifest_missing:{source_manifest_path}")
    if not moss_output_path.is_file():
        raise FileNotFoundError(f"moss_output_missing:{moss_output_path}")

    source_manifest = load_json(source_manifest_path)
    source_entries = validate_manifest(source_manifest)
    source_roles = {str(entry["role"]) for entry in source_entries}
    rehash_audit, source_mismatches = rehash_entries(source_entries)
    recorded_source_drift = validate_rehash_scope(
        roles=source_roles,
        mismatches=source_mismatches,
        phase="source_manifest",
    )

    moss_actual_sha256 = sha256_file(moss_output_path)
    moss_payload = load_json(moss_output_path)
    adapter_validation = validate_monolithic_adapter(
        moss_payload,
        actual_sha256=moss_actual_sha256,
        expected_sha256=expected_moss_sha256,
    )

    derived_manifest = copy.deepcopy(source_manifest)
    derived_entries = validate_manifest(derived_manifest)
    changed_indices = [
        index for index, entry in enumerate(derived_entries) if entry["role"] == MOSS_ROLE
    ]
    if len(changed_indices) != 1:
        raise ValueError("derived_manifest_moss_role_count_mismatch")
    changed_index = changed_indices[0]
    original_moss_entry = copy.deepcopy(source_entries[changed_index])
    derived_entries[changed_index]["path"] = str(moss_output_path)
    derived_entries[changed_index]["bytes"] = moss_output_path.stat().st_size
    derived_entries[changed_index]["sha256"] = moss_actual_sha256

    unchanged_role_checks: dict[str, bool] = {}
    changed_roles: list[str] = []
    for original, derived in zip(source_entries, derived_entries, strict=True):
        role = str(original["role"])
        same = original == derived
        if role == MOSS_ROLE:
            if same:
                raise ValueError("derived_manifest_moss_role_was_not_changed")
            changed_roles.append(role)
        else:
            unchanged_role_checks[role] = same
    if changed_roles != [MOSS_ROLE] or not all(unchanged_role_checks.values()):
        raise ValueError("derived_manifest_changed_more_than_moss_role")

    write_json(private_manifest_path, derived_manifest)
    derived_manifest_sha256 = sha256_file(private_manifest_path)
    source_manifest_sha256 = sha256_file(source_manifest_path)

    derived_manifest_reloaded = load_json(private_manifest_path)
    derived_entries_reloaded = validate_manifest(derived_manifest_reloaded)
    derived_roles = {str(entry["role"]) for entry in derived_entries_reloaded}
    derived_rehash_audit, derived_mismatches = rehash_entries(
        derived_entries_reloaded
    )
    recorded_derived_drift = validate_rehash_scope(
        roles=derived_roles,
        mismatches=derived_mismatches,
        phase="derived_manifest",
    )
    derived_moss_entry = next(
        entry for entry in derived_entries_reloaded if entry["role"] == MOSS_ROLE
    )

    unchanged_role_count = len(unchanged_role_checks)
    now = datetime.now(timezone.utc).isoformat()
    binding_status = (
        "PASS_WITH_RECORDED_NON_SCORING_DOCUMENT_DRIFT"
        if recorded_source_drift or recorded_derived_drift
        else "PASS"
    )
    private_derivation = {
        "schema_version": 1,
        "role": "MOSS_R3_PRIVATE_SCORING_MANIFEST_DERIVATION",
        "created_at": now,
        "source_manifest_path": str(source_manifest_path),
        "source_manifest_sha256": source_manifest_sha256,
        "derived_manifest_path": str(private_manifest_path.resolve()),
        "derived_manifest_sha256": derived_manifest_sha256,
        "moss_output_path": str(moss_output_path),
        "moss_output_sha256": moss_actual_sha256,
        "source_entry_count": len(source_entries),
        "source_manifest_all_entries_rehashed": len(rehash_audit) == len(source_entries),
        "source_manifest_rehash_mismatch_count": len(recorded_source_drift),
        "source_manifest_recorded_non_scoring_drift_roles": recorded_source_drift,
        "all_f3_f4_scoring_inputs_rehashed_and_matched": True,
        "derived_manifest_all_entries_rehashed": len(derived_rehash_audit)
        == len(derived_entries_reloaded),
        "derived_manifest_rehash_mismatch_count": len(recorded_derived_drift),
        "derived_manifest_recorded_non_scoring_drift_roles": recorded_derived_drift,
        "changed_role_count": len(changed_roles),
        "changed_roles": changed_roles,
        "unchanged_role_count": unchanged_role_count,
        "all_non_moss_entries_logically_unchanged": all(
            unchanged_role_checks.values()
        ),
        "original_moss_entry": original_moss_entry,
        "derived_moss_entry": derived_moss_entry,
        "original_moss_entry_canonical_sha256": canonical_sha256(original_moss_entry),
        "derived_moss_entry_canonical_sha256": canonical_sha256(derived_moss_entry),
        "adapter_validation": adapter_validation,
        "compatibility_role_note": (
            "moss_p1_private_output is retained only because the frozen F3/F4 scripts "
            "look up this role; adapter_validation.stage is the authoritative source identity."
        ),
        "transcript_text_included": False,
        "status": binding_status,
    }
    write_json(private_derivation_path, private_derivation)
    private_derivation_sha256 = sha256_file(private_derivation_path)

    public_binding = {
        "schema_version": 1,
        "role": "MOSS_R3_PUBLIC_MONOLITHIC_SCORING_BINDING",
        "created_at": now,
        "status": binding_status,
        "source_manifest_sha256": source_manifest_sha256.upper(),
        "derived_manifest_sha256": derived_manifest_sha256.upper(),
        "private_derivation_sha256": private_derivation_sha256.upper(),
        "entry_count": len(source_entries),
        "source_manifest_rehash": {
            "checked_count": len(rehash_audit),
            "matched_count": sum(
                1 for result in rehash_audit.values() if result["matches"]
            ),
            "mismatch_count": len(recorded_source_drift),
            "recorded_non_scoring_drift_roles": recorded_source_drift,
            "all_f3_f4_scoring_inputs_match": True,
        },
        "derived_manifest_rehash": {
            "checked_count": len(derived_rehash_audit),
            "matched_count": sum(
                1 for result in derived_rehash_audit.values() if result["matches"]
            ),
            "mismatch_count": len(recorded_derived_drift),
            "recorded_non_scoring_drift_roles": recorded_derived_drift,
            "all_f3_f4_scoring_inputs_match": True,
        },
        "change_audit": {
            "changed_role_count": 1,
            "changed_role": MOSS_ROLE,
            "unchanged_role_count": unchanged_role_count,
            "all_non_moss_entries_logically_unchanged": True,
            "original_moss_sha256": str(original_moss_entry["sha256"]).upper(),
            "bound_moss_sha256": moss_actual_sha256.upper(),
            "bound_moss_bytes": moss_output_path.stat().st_size,
        },
        "adapter_validation": adapter_validation,
        "compatibility_role_note": (
            "The retained P1 role name is a frozen-scorer lookup alias only. "
            "The bound output is the R1 monolithic Vulkan 16K run recorded above."
        ),
        "transcript_text_included": False,
        "absolute_paths_included": False,
    }
    write_json(public_binding_path, public_binding)

    private_manifest_digest = write_sha256_sidecar(private_manifest_path)
    private_derivation_digest = write_sha256_sidecar(private_derivation_path)
    public_binding_digest = write_sha256_sidecar(public_binding_path)
    if private_manifest_digest != derived_manifest_sha256:
        raise RuntimeError("private_manifest_changed_after_derivation")
    if private_derivation_digest != private_derivation_sha256:
        raise RuntimeError("private_derivation_changed_after_public_binding")

    return {
        "status": binding_status,
        "entry_count": len(source_entries),
        "changed_role_count": 1,
        "unchanged_role_count": unchanged_role_count,
        "source_manifest_sha256": source_manifest_sha256,
        "derived_manifest_sha256": derived_manifest_sha256,
        "moss_output_sha256": moss_actual_sha256,
        "public_binding_sha256": public_binding_digest,
        "adapter_validation": adapter_validation,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Bind a frozen monolithic MOSS output into the frozen P0-R score manifest."
    )
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--moss-output", type=Path, required=True)
    parser.add_argument("--expected-moss-sha256", required=True)
    parser.add_argument("--private-manifest", type=Path, required=True)
    parser.add_argument("--private-derivation", type=Path, required=True)
    parser.add_argument("--public-binding", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        result = derive_binding(
            source_manifest_path=args.source_manifest,
            moss_output_path=args.moss_output,
            expected_moss_sha256=args.expected_moss_sha256,
            private_manifest_path=args.private_manifest,
            private_derivation_path=args.private_derivation,
            public_binding_path=args.public_binding,
        )
    except Exception as exc:
        print(json.dumps({"status": "FAIL", "error": str(exc)}, ensure_ascii=False))
        return 2
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
