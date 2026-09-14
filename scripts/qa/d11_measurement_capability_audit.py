#!/usr/bin/env python3
"""Create the read-only D-11 measurement-capability audit report."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
from typing import Any


REQUIRED_BASE = "bb5675838da9b0ba9d7f31b64ffdd6af625896e0"
PLAN_REL = Path("target/release/docs/方案/MOSS功能修复计划-20260902")


def digest(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    return {"path": path.as_posix(), "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest().upper()}


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-c", "core.quotepath=false", *args],
        cwd=repo,
        check=True,
        text=True,
        encoding="utf-8",
        stdout=subprocess.PIPE,
    ).stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    repo = args.repo.resolve(strict=True)
    plan_dir = repo / PLAN_REL
    manifest_path = plan_dir / "Q00-FROZEN-TRUTH-MANIFEST.json"
    negative_path = plan_dir / "Q00-NEGATIVE-TRUTH.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    negative = json.loads(negative_path.read_text(encoding="utf-8"))

    tracked = set(git(repo, "ls-files", "--", PLAN_REL.as_posix()).splitlines())
    artifacts = []
    missing_roles = []
    for role, declaration in manifest["files"].items():
        basename = Path(declaration["relative_path"]).name
        candidates = sorted(path for path in tracked if Path(path).name == basename)
        actual = []
        for relative in candidates:
            record = digest(repo / relative)
            record["matches_declaration"] = (
                record["bytes"] == declaration["bytes"]
                and record["sha256"] == declaration["sha256"]
            )
            actual.append(record)
        if not any(item["matches_declaration"] for item in actual):
            missing_roles.append(role)
        artifacts.append({"role": role, "declared": declaration, "controlled_tracked_candidates": actual})

    measurement = (repo / "frontend/src-tauri/src/audio/measurement.rs").read_text(encoding="utf-8")
    pipeline = (repo / "frontend/src-tauri/src/audio/pipeline.rs").read_text(encoding="utf-8")
    worker = (repo / "frontend/src-tauri/src/audio/transcription/worker.rs").read_text(encoding="utf-8")
    commands = (repo / "frontend/src-tauri/src/audio/recording_commands.rs").read_text(encoding="utf-8")
    saver = (repo / "frontend/src-tauri/src/audio/recording_saver.rs").read_text(encoding="utf-8")
    library = (repo / "frontend/src-tauri/src/lib.rs").read_text(encoding="utf-8")
    combined = "\n".join((measurement, pipeline, worker, commands, saver, library))
    fields = {
        "audio_input_path_id": ("RealtimeAudioInputContract", "audio_input_path_id", "start_controlled_realtime_input"),
        "capture_pcm_sha256": ("capture_pcm_sha256", "capture-pcm.f32le"),
        "chunk_id_and_source_window": ("source_chunk_ids", "source_sample_start", "source_sample_end"),
        "overlap": ("overlap_samples", "record_overlap"),
        "vad": ("vad_decision", "vad_exclusion_reason", "record_vad_window"),
        "queue": ("enqueued_at_monotonic_ns", "dequeued_at_monotonic_ns"),
        "inference": ("inference_started_at_monotonic_ns", "inference_finished_at_monotonic_ns", "inference_pcm_sha256"),
        "writeback": ("final_writeback_at_monotonic_ns", "record_final_writeback"),
        "dedup_text": ("text_before_dedup", "text_after_dedup", "dedup_trace_status"),
        "language_context_model": ("actual_language", "context_version_id", "context_sha256", "transcription_model"),
        "load_timeline": ("process_cpu_percent", "system_cpu_percent", "gpu_unavailable_reason", "load_timeline_sha256"),
        "stop_save_timeline": ("stop_requested", "queue_drained", "checkpoint_merge_started", "final_save_finished"),
    }
    field_results = [
        {"field": name, "source_contract_present": all(token in combined for token in tokens), "tokens": list(tokens)}
        for name, tokens in fields.items()
    ]

    protected_diff = git(
        repo,
        "diff",
        "--unified=0",
        REQUIRED_BASE,
        "--",
        "frontend/src-tauri/src/audio/pipeline.rs",
        "frontend/src-tauri/src/audio/transcription/worker.rs",
        "frontend/src-tauri/src/whisper_engine/whisper_engine.rs",
    )
    protected_mutations = []
    protected_tokens = (
        "MAX_WHISPER_BATCH_DURATION_SECONDS",
        "MAX_WHISPER_BATCH_GAP_SECONDS",
        "redemption_time",
        "set_temperature(",
        "set_no_speech_thold(",
        "set_audio_ctx(",
        "set_max_tokens(",
        "set_initial_prompt(",
    )
    for line in protected_diff.splitlines():
        if line.startswith(("+", "-")) and not line.startswith(("+++", "---")):
            if any(token in line for token in protected_tokens):
                protected_mutations.append(line)

    report = {
        "schema_version": 1,
        "stage": "D11_MEASUREMENT_CAPABILITY_CHECKPOINT",
        "status": "IMPLEMENTED_COMPILED_RUNTIME_BASELINE_BLOCKED",
        "d11_state": "IN_PROGRESS",
        "i11a_started": False,
        "git": {
            "branch": git(repo, "branch", "--show-current"),
            "head": git(repo, "rev-parse", "HEAD"),
            "required_base": REQUIRED_BASE,
            "head_matches_required_base_before_checkpoint_commit": git(repo, "rev-parse", "HEAD") == REQUIRED_BASE,
        },
        "scope": {
            "task": "D-11",
            "alias_only": "P0-RA-01",
            "formal_product_data_access_count": 0,
            "hardware_test_count": 0,
            "three_run_baseline_count": 0,
            "forbidden_untracked_draft_access_count": 0,
        },
        "frozen_truth_recheck": {
            "window_duration_seconds_registered": manifest["scope"]["window_duration_ms"] / 1000,
            "window_audio_sha256_registered": manifest["scope"]["window_audio_sha256"],
            "source_audio_sha256_registered": manifest["scope"]["source_audio_sha256"],
            "manifest": digest(manifest_path),
            "negative_truth": digest(negative_path),
            "negative_terms": [item["term"] for item in negative["terms"]],
            "artifacts": artifacts,
            "missing_controlled_truth_roles": missing_roles,
            "source_audio_bytes_rehashed": False,
            "window_audio_bytes_rehashed": False,
        },
        "measurement_fields": field_results,
        "required_field_count": len(field_results),
        "source_contract_present_count": sum(item["source_contract_present"] for item in field_results),
        "protected_parameter_diff_lines": protected_mutations,
        "protected_parameter_mutation_count": len(protected_mutations),
        "runtime": {
            "controlled_realtime_recordings_run": 0,
            "cold_runs": 0,
            "warm_runs": 0,
            "cer": None,
            "queue_backlog": None,
            "tail_drain_seconds": None,
            "save_duration_seconds": None,
        },
        "blockers": [
            "冻结源音频、226.440 秒窗口音频、人工逐字稿、说话人真值、人工复核、会前上下文和正例术语原件未出现在当前受控检出位置，不能启动正式冷热基线。",
            "Rust 测量模块测试二进制已编译，但本机启动时返回 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND，因此 4 个 Rust 定向单测实际执行数为 0。",
            "当前跨平台运行时没有 GPU 遥测提供方；结构化负载记录会写 null 和明确不可用原因，不能冒充 GPU 已测。",
            "没有实际完成三次冷/热正式实时运行，所以准确率、积压、延迟、排空和保存耗时均无 PASS。",
        ],
    }
    if report["source_contract_present_count"] != report["required_field_count"]:
        report["status"] = "FAIL_SOURCE_CONTRACT_INCOMPLETE"
    if protected_mutations:
        report["status"] = "FAIL_PROTECTED_PARAMETER_MUTATION"
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": report["status"],
        "source_contract_present": f'{report["source_contract_present_count"]}/{report["required_field_count"]}',
        "missing_truth_roles": len(missing_roles),
        "protected_parameter_mutation_count": len(protected_mutations),
    }, ensure_ascii=False))
    return 0 if report["status"] == "IMPLEMENTED_COMPILED_RUNTIME_BASELINE_BLOCKED" else 1


if __name__ == "__main__":
    raise SystemExit(main())
