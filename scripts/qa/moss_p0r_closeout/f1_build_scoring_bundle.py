from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"JSON root must be an object: {path}")
    return value


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def find_p1_business_case(manifest: dict[str, Any]) -> dict[str, Any] | None:
    cases = manifest.get("positive_cases")
    if not isinstance(cases, list):
        return None
    for item in cases:
        if isinstance(item, dict) and item.get("sample") == "business_737s":
            return item
    return None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Freeze P0-R local scoring inputs as reference-only hashes."
    )
    parser.add_argument("--spec", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    spec = load_json(args.spec)
    raw_entries = spec.get("entries")
    if not isinstance(raw_entries, list) or not raw_entries:
        raise SystemExit("spec.entries must be a non-empty list")

    roles: list[str] = []
    paths: dict[str, Path] = {}
    sensitivity: dict[str, bool] = {}
    spec_errors: list[str] = []
    for index, entry in enumerate(raw_entries):
        if not isinstance(entry, dict):
            spec_errors.append(f"entry_{index}_not_object")
            continue
        role = entry.get("role")
        path_value = entry.get("path")
        sensitive = entry.get("sensitive")
        if not isinstance(role, str) or not role:
            spec_errors.append(f"entry_{index}_invalid_role")
            continue
        if not isinstance(path_value, str) or not Path(path_value).is_absolute():
            spec_errors.append(f"entry_{index}_invalid_absolute_path")
            continue
        if not isinstance(sensitive, bool):
            spec_errors.append(f"entry_{index}_invalid_sensitive_flag")
            continue
        roles.append(role)
        paths[role] = Path(path_value)
        sensitivity[role] = sensitive

    duplicate_roles = sorted({role for role in roles if roles.count(role) > 1})
    missing_roles = sorted(role for role, path in paths.items() if not path.is_file())
    spec_valid = not spec_errors and not duplicate_roles and not missing_roles

    args.out_dir.mkdir(parents=True, exist_ok=True)
    if not spec_valid:
        failure = {
            "schema_version": 1,
            "role": "P0R_F1_SCORING_BUNDLE_AUDIT",
            "created_at": datetime.now(timezone.utc).isoformat(),
            "status": "FAIL",
            "spec_errors": spec_errors,
            "duplicate_roles": duplicate_roles,
            "missing_roles": missing_roles,
            "decision": "F1_FAIL_STOP_BEFORE_TERM_FREEZE",
        }
        write_json(args.out_dir / "02-F1-audit.json", failure)
        print(json.dumps({"status": "FAIL", "missing_role_count": len(missing_roles)}))
        return 2

    records = []
    for role in sorted(paths):
        path = paths[role]
        records.append(
            {
                "role": role,
                "path": str(path.resolve()),
                "bytes": path.stat().st_size,
                "sha256": sha256_file(path),
                "sensitive": sensitivity[role],
                "storage": "EXTERNAL_REFERENCE_NOT_COPIED",
            }
        )
    by_role = {item["role"]: item for item in records}

    f0 = load_json(paths["f0_formal_whisper_audit"])
    human_review = load_json(paths["human_review"])
    human_independent = load_json(paths["human_independent_audit"])
    formal_record = load_json(paths["formal_whisper_run_record"])
    formal_status = load_json(paths["formal_whisper_status"])
    formal_full = load_json(paths["formal_whisper_full_output"])
    moss_private = load_json(paths["moss_p1_private_output"])
    moss_public = load_json(paths["moss_p1_public_output"])
    moss_supervisor = load_json(paths["moss_p1_supervisor_record"])
    moss_manifest = load_json(paths["moss_p1_evidence_manifest"])
    p1_case = find_p1_business_case(moss_manifest)
    if p1_case is None:
        p1_case = {}

    global_turns = moss_private.get("global_turns")
    if not isinstance(global_turns, list):
        global_turns = []
    global_gate = moss_private.get("global_gate")
    if not isinstance(global_gate, dict):
        global_gate = {}
    source_sample = moss_private.get("source_sample")
    if not isinstance(source_sample, dict):
        source_sample = {}
    post_integrity = moss_private.get("post_integrity")
    if not isinstance(post_integrity, dict):
        post_integrity = {}
    model_integrity = post_integrity.get("model")
    if not isinstance(model_integrity, dict):
        model_integrity = {}
    toolchain = post_integrity.get("test_toolchain")
    if not isinstance(toolchain, dict):
        toolchain = {}

    tool_bindings = {
        "moss_p1_runner": "runner",
        "moss_p1_supervisor": "supervisor",
        "moss_p1_chunked_runner": "chunked_runner",
        "moss_p1_lock": "lock",
    }
    tool_matches: dict[str, bool] = {}
    for manifest_role, p1_name in tool_bindings.items():
        recorded = toolchain.get(p1_name)
        tool_matches[manifest_role] = (
            isinstance(recorded, dict)
            and str(recorded.get("sha256", "")).casefold()
            == by_role[manifest_role]["sha256"]
        )

    strict_required_moss_fields = {
        "schema_version",
        "role",
        "run_id",
        "input_audio_sha256",
        "prompt_sha256",
        "model_manifest_sha256",
        "generated_tokens",
        "prompt_tokens",
        "generation_termination",
        "raw_model_text",
        "parser_audit",
        "segments",
    }
    strict_schema_compatible = (
        strict_required_moss_fields.issubset(moss_private)
        and moss_private.get("schema_version") == 2
        and moss_private.get("role") == "LOCKED_FULL_MOSS_OUTPUT"
    )

    checks: dict[str, bool] = {
        "spec_valid": spec_valid,
        "roles_unique": len(set(roles)) == len(roles),
        "all_referenced_files_exist": not missing_roles,
        "f0_pass": f0.get("status") == "PASS",
        "f0_has_no_failed_checks": f0.get("summary", {}).get("failed_count") == 0,
        "human_review_approved": human_review.get("approved_as_ground_truth") is True,
        "human_independent_audit_pass": (
            human_independent.get("structural_status") == "PASS"
            and isinstance(human_independent.get("checks"), dict)
            and all(value is True for value in human_independent["checks"].values())
        ),
        "formal_whisper_completed": formal_record.get("execution_status") == "COMPLETED",
        "formal_whisper_exit_zero": formal_record.get("process_exit_code") == 0,
        "formal_whisper_status_not_falsely_go": formal_status.get("go_allowed") is False,
        "formal_whisper_full_hash_bound": str(
            formal_record.get("output_full_sha256", "")
        ).casefold()
        == by_role["formal_whisper_full_output"]["sha256"],
        "formal_whisper_run_record_hash_bound": str(
            formal_status.get("run_record_sha256", "")
        ).casefold()
        == by_role["formal_whisper_run_record"]["sha256"],
        "formal_whisper_audio_matches_source": str(
            formal_record.get("source_full_audio_sha256", "")
        ).casefold()
        == by_role["source_full_audio"]["sha256"],
        "formal_whisper_window_matches_reference": str(
            formal_record.get("window_audio_sha256", "")
        ).casefold()
        == by_role["reference_window_audio"]["sha256"],
        "formal_whisper_model_bound": str(
            formal_record.get("model_file_sha256", "")
        ).casefold()
        == by_role["whisper_model"]["sha256"],
        "formal_whisper_exe_bound": str(
            formal_record.get("stable_exe_sha256", "")
        ).casefold()
        == by_role["stable_meetily_exe"]["sha256"],
        "formal_whisper_transformation_exact_copy": (
            isinstance(formal_full.get("transformation"), dict)
            and formal_full["transformation"].get("text_changed") is False
            and formal_full["transformation"].get("timestamps_changed") is False
            and formal_full["transformation"].get("speaker_labels_changed") is False
        ),
        "moss_manifest_commit_is_canonical": moss_manifest.get("git_commit")
        == "985b40f14d843e1a8e14fb944440cc64de0c9412",
        "moss_business_case_found": bool(p1_case),
        "moss_public_hash_matches_p1_manifest": str(
            p1_case.get("public_sha256", "")
        ).casefold()
        == by_role["moss_p1_public_output"]["sha256"],
        "moss_private_hash_matches_p1_manifest": str(
            p1_case.get("private_sha256", "")
        ).casefold()
        == by_role["moss_p1_private_output"]["sha256"],
        "moss_supervisor_hash_matches_p1_manifest": str(
            p1_case.get("supervisor_sha256", "")
        ).casefold()
        == by_role["moss_p1_supervisor_record"]["sha256"],
        "moss_attestation_hash_matches_p1_manifest": str(
            p1_case.get("attestation_sha256", "")
        ).casefold()
        == by_role["moss_p1_public_attestation"]["sha256"],
        "moss_private_stage_is_p1_chunked": moss_private.get("stage")
        == "MOSS_V3_P1_CHUNKED",
        "moss_private_structural_runtime_pass": moss_private.get("run_state")
        == "STRUCTURAL_RUNTIME_PASS",
        "moss_accuracy_was_not_prejudged": moss_private.get("p1_verdict")
        == "NOT_EVALUATED_P0_R_BLOCKED",
        "moss_release_was_not_falsely_allowed": moss_private.get("release_go") is False,
        "moss_source_audio_matches_frozen_full_audio": str(
            source_sample.get("sha256", "")
        ).casefold()
        == by_role["source_full_audio"]["sha256"],
        "moss_model_hash_matches_current_file": str(
            model_integrity.get("sha256", "")
        ).casefold()
        == by_role["moss_q8_model"]["sha256"],
        "moss_global_gate_pass": global_gate.get("state") == "PASS",
        "moss_has_76_global_turns": len(global_turns) == 76,
        "moss_full_audio_duration_is_737_728": abs(
            float(global_gate.get("total_audio_seconds", -1)) - 737.728
        )
        <= 0.001,
        "moss_rtf_within_gate": float(global_gate.get("total_rtf", 999)) <= 1.0,
        "moss_supervisor_completed": moss_supervisor.get("status")
        == "SUPERVISED_CHILD_COMPLETED",
        "moss_public_is_non_transcript_attestation": "global_turns" not in moss_public,
        "moss_private_is_bound_but_not_copied": by_role["moss_p1_private_output"][
            "storage"
        ]
        == "EXTERNAL_REFERENCE_NOT_COPIED",
        "all_p1_tool_hashes_still_match": all(tool_matches.values()),
        "strict_cuda_schema_is_not_falsely_claimed": not strict_schema_compatible,
    }

    failed_checks = [name for name, passed in checks.items() if not passed]
    now = datetime.now(timezone.utc).isoformat()
    manifest = {
        "schema_version": 1,
        "role": "P0R_F1_REFERENCE_ONLY_SCORING_BUNDLE_MANIFEST",
        "bundle_id": spec.get("bundle_id"),
        "created_at": now,
        "storage_policy": "REFERENCE_ONLY_NO_SOURCE_COPY",
        "transcript_text_included": False,
        "entry_count": len(records),
        "sensitive_entry_count": sum(1 for item in records if item["sensitive"]),
        "spec": {
            "path": str(args.spec.resolve()),
            "bytes": args.spec.stat().st_size,
            "sha256": sha256_file(args.spec),
        },
        "entries": records,
    }
    manifest_path = args.out_dir / "MANIFEST.json"
    write_json(manifest_path, manifest)
    manifest_hash = sha256_file(manifest_path)
    (args.out_dir / "MANIFEST.json.sha256").write_text(
        f"{manifest_hash}  MANIFEST.json\n", encoding="ascii"
    )

    local_ready = not failed_checks
    lock = {
        "schema_version": 1,
        "role": "P0R_F1_SCORING_BUNDLE_LOCK",
        "bundle_id": spec.get("bundle_id"),
        "created_at": now,
        "manifest_sha256": manifest_hash,
        "status": (
            "LOCAL_P0R_BASE_INPUTS_FROZEN_F2_TERMS_PENDING"
            if local_ready
            else "F1_INPUT_INTEGRITY_FAIL"
        ),
        "local_p0r_scoring_lane": {
            "allowed_after_f2_terms": local_ready,
            "moss_input_schema": "MOSS_V3_P1_CHUNKED_SCHEMA_1_GLOBAL_TURNS",
            "source_audio_sha256": by_role["source_full_audio"]["sha256"],
            "window_audio_sha256": by_role["reference_window_audio"]["sha256"],
            "human_truth_sha256": {
                "verbatim": by_role["human_verbatim"]["sha256"],
                "turns": by_role["human_speaker_turns"]["sha256"],
                "review": by_role["human_review"]["sha256"],
            },
            "formal_whisper_run_id": formal_record.get("run_id"),
            "moss_git_commit": moss_manifest.get("git_commit"),
        },
        "strict_s8_cuda_lane": {
            "allowed": False,
            "status": "BLOCKED_MISSING_LOCKED_FULL_MOSS_OUTPUT_AND_SUPERVISED_LIVE_CUDA_RUN",
            "required_moss_schema": "schema_version=2 role=LOCKED_FULL_MOSS_OUTPUT",
            "actual_moss_schema": f"schema_version={moss_private.get('schema_version')} stage={moss_private.get('stage')}",
            "conversion_or_rename_allowed": False,
        },
        "next_gate": "F2_TERM_TRUTH",
    }
    lock_path = args.out_dir / "01-bundle-lock.json"
    write_json(lock_path, lock)

    audit = {
        "schema_version": 1,
        "role": "P0R_F1_SCORING_BUNDLE_AUDIT",
        "created_at": now,
        "status": "PASS" if local_ready else "FAIL",
        "transcript_text_included": False,
        "summary": {
            "check_count": len(checks),
            "passed_count": len(checks) - len(failed_checks),
            "failed_count": len(failed_checks),
            "failed_checks": failed_checks,
            "entry_count": len(records),
            "sensitive_entry_count": manifest["sensitive_entry_count"],
            "moss_global_turn_count": len(global_turns),
            "moss_total_rtf": global_gate.get("total_rtf"),
            "strict_cuda_schema_compatible": strict_schema_compatible,
        },
        "checks": checks,
        "tool_hash_matches": tool_matches,
        "outputs": {
            "manifest": {
                "path": str(manifest_path.resolve()),
                "bytes": manifest_path.stat().st_size,
                "sha256": manifest_hash,
            },
            "bundle_lock": {
                "path": str(lock_path.resolve()),
                "bytes": lock_path.stat().st_size,
                "sha256": sha256_file(lock_path),
            },
        },
        "decision": (
            "F1_PASS_ALLOW_F2_LOCAL_SCORING_TERMS"
            if local_ready
            else "F1_FAIL_STOP_BEFORE_TERMS"
        ),
    }
    audit_path = args.out_dir / "02-F1-audit.json"
    write_json(audit_path, audit)
    print(
        json.dumps(
            {
                "status": audit["status"],
                "check_count": len(checks),
                "failed_count": len(failed_checks),
                "entry_count": len(records),
                "decision": audit["decision"],
                "manifest_sha256": manifest_hash,
                "audit_sha256": sha256_file(audit_path),
            },
            ensure_ascii=False,
        )
    )
    return 0 if local_ready else 2


if __name__ == "__main__":
    sys.exit(main())
