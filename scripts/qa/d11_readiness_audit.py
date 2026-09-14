#!/usr/bin/env python3
"""Read-only D-11 readiness audit for the real-time transcription baseline.

The audit only reads tracked source and the two checked-in Q-00 truth records.
It never opens the production ``com.meetily.ai`` data root, never runs audio
capture, and never imports the untracked historical draft excluded by TASKS.md.
"""

from __future__ import annotations

import argparse
import ast
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
from typing import Any, Iterable


BASE_COMMIT = "4f4737575fff220ed92c2ed8f4ac56994749733e"
BASE_BRANCH = "codex/d10a-evidence-20260906"
EVIDENCE_DIRECTORY = (
    Path("target")
    / "release"
    / "docs"
    / "方案"
    / "MOSS功能修复计划-20260902"
    / "D11-DIAGNOSTIC-CHECKPOINT-20260906"
)
PLAN_DIRECTORY = EVIDENCE_DIRECTORY.parent
Q00_SCORER = Path("scripts/qa/moss_functional_fix_quality_gate.py")
Q00_FORMAL_RUNNER = Path("scripts/qa/moss_functional_fix_q00_formal.py")
Q00_TRUTH_MANIFEST = PLAN_DIRECTORY / "Q00-FROZEN-TRUTH-MANIFEST.json"
Q00_NEGATIVE_TRUTH = PLAN_DIRECTORY / "Q00-NEGATIVE-TRUTH.json"

SOURCE_SCAN_ROOTS = (
    Path("frontend/src-tauri/src"),
    Path("frontend/src"),
    Path("backend"),
    Path("moss-helper/src"),
    Path("scripts/qa"),
)
SOURCE_SUFFIXES = {
    ".c",
    ".cc",
    ".cpp",
    ".h",
    ".hpp",
    ".js",
    ".json",
    ".mjs",
    ".ps1",
    ".py",
    ".rs",
    ".ts",
    ".tsx",
}
SELF_FILES = {
    "d11_readiness_audit.py",
    "run_d11_diagnostic_checkpoint.py",
    "test_d11_readiness_audit.py",
}


class AuditError(RuntimeError):
    """The audit itself could not establish a trustworthy result."""


def _sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest().upper()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest().upper()


def _file_record(repo: Path, relative: Path) -> dict[str, Any]:
    path = repo / relative
    if not path.is_file():
        raise AuditError(f"required checked-in file is missing: {relative.as_posix()}")
    if path.is_symlink():
        raise AuditError(f"required checked-in file must not be a symlink: {relative.as_posix()}")
    return {
        "path": relative.as_posix(),
        "bytes": path.stat().st_size,
        "sha256": _sha256_file(path),
    }


def _read_text(repo: Path, relative: Path) -> str:
    return (repo / relative).read_text(encoding="utf-8")


def _read_json(repo: Path, relative: Path) -> dict[str, Any]:
    document = json.loads(_read_text(repo, relative))
    if not isinstance(document, dict):
        raise AuditError(f"JSON root must be an object: {relative.as_posix()}")
    return document


def _git(repo: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repo,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    if completed.returncode != 0:
        raise AuditError(
            f"git {' '.join(arguments)} failed with {completed.returncode}: "
            f"{completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def _tracked_paths(repo: Path) -> list[Path]:
    completed = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=repo,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        raise AuditError(f"git ls-files failed with {completed.returncode}")
    return [
        Path(item.decode("utf-8"))
        for item in completed.stdout.split(b"\0")
        if item
    ]


def _literal_constants(source: str) -> dict[str, Any]:
    tree = ast.parse(source)
    result: dict[str, Any] = {}
    for node in tree.body:
        if not isinstance(node, (ast.Assign, ast.AnnAssign)):
            continue
        targets = node.targets if isinstance(node, ast.Assign) else [node.target]
        value = node.value
        try:
            literal = ast.literal_eval(value)
        except (ValueError, TypeError):
            continue
        for target in targets:
            if isinstance(target, ast.Name):
                result[target.id] = literal
    return result


def _function_source(source: str, name: str) -> str:
    tree = ast.parse(source)
    lines = source.replace("\r\n", "\n").splitlines(keepends=True)
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name:
            if node.end_lineno is None:
                raise AuditError(f"Python AST lacks end line for {name}")
            return "".join(lines[node.lineno - 1 : node.end_lineno]).rstrip("\n") + "\n"
    raise AuditError(f"function not found in Q-00 scorer: {name}")


def _line_matches(source: str, needle: str) -> list[int]:
    return [
        index
        for index, line in enumerate(source.replace("\r\n", "\n").splitlines(), 1)
        if needle in line
    ]


def _extract_struct(source: str, name: str) -> str:
    match = re.search(
        rf"(?:pub\s+)?struct\s+{re.escape(name)}\s*\{{(?P<body>.*?)\n\}}",
        source,
        flags=re.DOTALL,
    )
    if not match:
        raise AuditError(f"Rust struct not found: {name}")
    return match.group(0)


def _scan_source_files(repo: Path) -> list[tuple[Path, str]]:
    rows: list[tuple[Path, str]] = []
    for root in SOURCE_SCAN_ROOTS:
        absolute_root = repo / root
        if not absolute_root.exists():
            continue
        for path in absolute_root.rglob("*"):
            if not path.is_file() or path.suffix.lower() not in SOURCE_SUFFIXES:
                continue
            if path.name in SELF_FILES or "node_modules" in path.parts:
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                continue
            rows.append((path.relative_to(repo), text))
    rows.sort(key=lambda row: row[0].as_posix())
    return rows


def _find_occurrences(
    source_files: Iterable[tuple[Path, str]], token: str, *, limit: int = 20
) -> list[dict[str, Any]]:
    matches: list[dict[str, Any]] = []
    for path, source in source_files:
        for line_number, line in enumerate(source.replace("\r\n", "\n").splitlines(), 1):
            if token in line:
                matches.append(
                    {
                        "path": path.as_posix(),
                        "line": line_number,
                        "text": line.strip()[:240],
                    }
                )
                if len(matches) >= limit:
                    return matches
    return matches


def _q00_actions(source: str) -> list[str]:
    tree = ast.parse(source)
    actions: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        for keyword in node.keywords:
            if keyword.arg != "action":
                continue
            try:
                value = ast.literal_eval(keyword.value)
            except (ValueError, TypeError):
                continue
            if isinstance(value, str):
                actions.add(value)
    return sorted(actions)


def _artifact_checkout_inventory(
    repo: Path, tracked: list[Path], truth_manifest: dict[str, Any]
) -> list[dict[str, Any]]:
    tracked_by_name: dict[str, list[Path]] = {}
    for path in tracked:
        tracked_by_name.setdefault(path.name, []).append(path)

    files = truth_manifest.get("files")
    if not isinstance(files, dict):
        raise AuditError("Q-00 truth manifest files must be an object")
    rows: list[dict[str, Any]] = []
    for role, raw_record in files.items():
        if not isinstance(raw_record, dict):
            raise AuditError(f"Q-00 truth manifest role must be an object: {role}")
        relative_name = raw_record.get("relative_path")
        if not isinstance(relative_name, str):
            raise AuditError(f"Q-00 truth manifest relative path missing: {role}")
        candidates = tracked_by_name.get(Path(relative_name).name, [])
        exact_plan_path = PLAN_DIRECTORY / relative_name
        exact_exists = exact_plan_path in tracked and (repo / exact_plan_path).is_file()
        actual_record = _file_record(repo, exact_plan_path) if exact_exists else None
        rows.append(
            {
                "role": role,
                "declared": {
                    "relative_path": relative_name,
                    "bytes": raw_record.get("bytes"),
                    "sha256": str(raw_record.get("sha256", "")).upper(),
                },
                "checked_in_at_manifest_relative_path": exact_exists,
                "checked_in_same_basename_paths": [item.as_posix() for item in candidates],
                "actual": actual_record,
                "actual_matches_declaration": bool(
                    actual_record
                    and actual_record["bytes"] == raw_record.get("bytes")
                    and actual_record["sha256"] == str(raw_record.get("sha256", "")).upper()
                ),
            }
        )
    return rows


def _canonical_json_sha256(document: dict[str, Any]) -> str:
    payload = json.dumps(
        document,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return _sha256_bytes(payload)


def _require_literal_string_list(source: str, expected: list[str], label: str) -> list[str]:
    tree = ast.parse(source)
    for node in ast.walk(tree):
        if not isinstance(node, (ast.List, ast.Tuple)):
            continue
        try:
            value = ast.literal_eval(node)
        except (ValueError, TypeError):
            continue
        if value == expected:
            return list(value)
    raise AuditError(f"{label} is no longer enforced by the Q-00 scorer")


def _validated_output_path(repo: Path, output: Path) -> Path:
    resolved = output.resolve()
    allowed_root = (repo.resolve() / EVIDENCE_DIRECTORY).resolve()
    try:
        resolved.relative_to(allowed_root)
    except ValueError as exc:
        raise AuditError(f"output must stay inside {allowed_root}") from exc
    return resolved


def collect_audit(repo: Path) -> dict[str, Any]:
    repo = repo.resolve(strict=True)
    head = _git(repo, "rev-parse", "HEAD")
    branch = _git(repo, "branch", "--show-current")
    base_tip = _git(repo, "rev-parse", BASE_BRANCH)
    merge_base = _git(repo, "merge-base", "HEAD", BASE_BRANCH)
    tracked = _tracked_paths(repo)

    scorer_source = _read_text(repo, Q00_SCORER)
    formal_source = _read_text(repo, Q00_FORMAL_RUNNER)
    worker_path = Path("frontend/src-tauri/src/audio/transcription/worker.rs")
    pipeline_path = Path("frontend/src-tauri/src/audio/pipeline.rs")
    recording_state_path = Path("frontend/src-tauri/src/audio/recording_state.rs")
    recording_commands_path = Path("frontend/src-tauri/src/audio/recording_commands.rs")
    saver_path = Path("frontend/src-tauri/src/audio/recording_saver.rs")
    worker_source = _read_text(repo, worker_path)
    pipeline_source = _read_text(repo, pipeline_path)
    recording_state_source = _read_text(repo, recording_state_path)
    recording_commands_source = _read_text(repo, recording_commands_path)
    saver_source = _read_text(repo, saver_path)
    source_files = _scan_source_files(repo)

    constants = _literal_constants(scorer_source)
    truth_manifest = _read_json(repo, Q00_TRUTH_MANIFEST)
    negative_truth = _read_json(repo, Q00_NEGATIVE_TRUTH)
    manifest_record = _file_record(repo, Q00_TRUTH_MANIFEST)
    negative_record = _file_record(repo, Q00_NEGATIVE_TRUTH)
    checkout_artifacts = _artifact_checkout_inventory(repo, tracked, truth_manifest)

    required_constants = {
        "SOURCE_SHA256",
        "SOURCE_BYTES",
        "SOURCE_DURATION_SECONDS",
        "WINDOW_START_SECONDS",
        "WINDOW_END_SECONDS",
        "WINDOW_DURATION_SECONDS",
        "SAMPLE_RATE_HZ",
        "CHANNELS",
        "SAMPLE_WIDTH_BYTES",
        "WINDOW_FRAME_COUNT",
        "WINDOW_BYTES",
        "WINDOW_SHA256",
        "FROZEN_TRUTH_MANIFEST_BYTES",
        "FROZEN_TRUTH_MANIFEST_SHA256",
        "FROZEN_HUMAN_VERBATIM_BYTES",
        "FROZEN_HUMAN_VERBATIM_SHA256",
        "FROZEN_POSITIVE_TRUTH_BYTES",
        "FROZEN_POSITIVE_TRUTH_SHA256",
        "FROZEN_NEGATIVE_TRUTH_BYTES",
        "FROZEN_NEGATIVE_TRUTH_SHA256",
    }
    missing_constants = sorted(required_constants - set(constants))
    if missing_constants:
        raise AuditError(f"Q-00 scorer constants missing: {missing_constants}")

    manifest_scope = truth_manifest.get("scope")
    expected_scope = {
        "source_audio_sha256": constants["SOURCE_SHA256"],
        "window_audio_sha256": constants["WINDOW_SHA256"],
        "window_start_ms": round(constants["WINDOW_START_SECONDS"] * 1000),
        "window_end_ms": round(constants["WINDOW_END_SECONDS"] * 1000),
        "window_duration_ms": round(constants["WINDOW_DURATION_SECONDS"] * 1000),
    }
    frozen_checks = {
        "manifest_file_matches_q00_constants": (
            manifest_record["bytes"] == constants["FROZEN_TRUTH_MANIFEST_BYTES"]
            and manifest_record["sha256"] == constants["FROZEN_TRUTH_MANIFEST_SHA256"]
        ),
        "negative_file_matches_q00_constants": (
            negative_record["bytes"] == constants["FROZEN_NEGATIVE_TRUTH_BYTES"]
            and negative_record["sha256"] == constants["FROZEN_NEGATIVE_TRUTH_SHA256"]
        ),
        "manifest_scope_matches_q00_constants": manifest_scope == expected_scope,
        "negative_window_matches_manifest": (
            str(negative_truth.get("window_audio_sha256", "")).upper()
            == constants["WINDOW_SHA256"]
        ),
        "negative_human_truth_matches_manifest": (
            str(negative_truth.get("human_verbatim_sha256", "")).upper()
            == constants["FROZEN_HUMAN_VERBATIM_SHA256"]
        ),
    }

    functions = {}
    bundle_parts = []
    for name in ("_normalize_cer_text", "_levenshtein_distance", "_cer"):
        function_source = _function_source(scorer_source, name)
        functions[name] = {
            "sha256_lf_utf8": _sha256_bytes(function_source.encode("utf-8")),
            "start_lines": _line_matches(scorer_source, f"def {name}"),
        }
        bundle_parts.append(f"{name}\n{function_source}")
    cer_bundle = "\n".join(bundle_parts).encode("utf-8")
    cer_rule = {
        "unicode_normalization": "NFKC",
        "case": "UNICODE_CASEFOLD",
        "retained_categories": ["LETTER", "NUMBER"],
        "unit": "ONE_RETAINED_UNICODE_CODE_POINT",
        "formula": "LEVENSHTEIN_DISTANCE / REFERENCE_CODE_POINT_COUNT",
    }
    positive_terms = _require_literal_string_list(
        scorer_source,
        ["YouTube", "PWA", "Google"],
        "frozen positive term set",
    )

    transcript_update = _extract_struct(worker_source, "TranscriptUpdate")
    audio_chunk = _extract_struct(recording_state_source, "AudioChunk")
    q00_actions = _q00_actions(formal_source)

    audio_input_occurrences = _find_occurrences(source_files, "audio_input_path_id")
    capture_pcm_occurrences = _find_occurrences(source_files, "capture_pcm_sha256")
    load_timeline_occurrences = _find_occurrences(source_files, "load_timeline_sha256")
    baseline_snapshot_occurrences = _find_occurrences(source_files, "baseline_snapshot_id")

    trace_matrix = [
        {
            "field": "chunk_id",
            "availability": "TRANSIENT_ONLY_FAIL",
            "facts": [
                "AudioChunk 在内存中携带 chunk_id。",
                "TranscriptUpdate 没有 chunk_id，最终落盘文字无法回连原始分段。",
                "积压合并会让多个源分段共用一个保留下来的 AudioChunk 身份。",
            ],
            "anchors": [
                {"path": recording_state_path.as_posix(), "lines": _line_matches(recording_state_source, "pub chunk_id")},
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "pub struct TranscriptUpdate")},
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "represented_chunks")[:6]},
            ],
            "structured_contract_present": "chunk_id" in transcript_update,
        },
        {
            "field": "sample_window_start_end",
            "availability": "PARTIAL_NOT_RECOMPUTABLE_FAIL",
            "facts": [
                "实时 VAD 分段临时持有起止毫秒，TranscriptUpdate 只保留派生后的音频起止秒数。",
                "没有落盘的原始采样索引窗口、VAD 排除窗口清单或覆盖 [0, 226.440] 的完整记录。",
            ],
            "anchors": [
                {"path": pipeline_path.as_posix(), "lines": _line_matches(pipeline_source, "segment.start_timestamp_ms")[:8]},
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "pub audio_start_time")},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "overlap",
            "availability": "TRANSIENT_ONLY_FAIL",
            "facts": [
                "积压合并只在局部变量中计算 overlap_samples。",
                "重叠量和源分段成员关系都没有结构化发出或落盘。",
            ],
            "anchors": [
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "overlap_samples")},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "vad_decision_and_exclusions",
            "availability": "TRANSIENT_ONLY_FAIL",
            "facts": [
                "ContinuousVadProcessor 只返回带临时边界的语音段。",
                "没有逐窗口 VAD 判定或明确排除区间，无法重算全程覆盖。",
            ],
            "anchors": [
                {"path": pipeline_path.as_posix(), "lines": _line_matches(pipeline_source, "process_audio(&mixed_with_gain)")},
                {"path": pipeline_path.as_posix(), "lines": _line_matches(pipeline_source, "Dropping short VAD segment")},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "enqueue_dequeue_timestamps",
            "availability": "AGGREGATE_COUNTER_ONLY_FAIL",
            "facts": [
                "工作线程只保留汇总的已排队/已完成计数并发送汇总进度。",
                "没有逐 chunk 的单调时钟入队和出队时间。",
            ],
            "anchors": [
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "CURRENT_CHUNKS_QUEUED")[:8]},
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, '"chunks_queued"')},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "inference_start_end_timestamps",
            "availability": "MISSING_FAIL",
            "facts": [
                "生产调用等待转写提供方返回，但没有记录逐 chunk 单调时钟推理起止事件。",
                "测试代码中的模拟 inference_seconds 不是产品运行证据。",
            ],
            "anchors": [
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "transcribe_chunk_with_provider(")[:4]},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "final_writeback_timestamp",
            "availability": "EVENT_WITHOUT_REQUIRED_JOIN_KEYS_FAIL",
            "facts": [
                "TranscriptUpdate 会发出，transcripts.json 也会增量写入。",
                "更新不含 chunk_id 和单调时钟最终写回时间，无法重算最大分段延迟。",
            ],
            "anchors": [
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, 'emit("transcript-update"')},
                {"path": saver_path.as_posix(), "lines": _line_matches(saver_source, "write_transcripts_json(folder)")},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "text_before_after_dedup",
            "availability": "MISSING_STRUCTURED_PAIR_FAIL",
            "facts": [
                "提供方文字可能在创建 TranscriptUpdate 前被 RecognitionContext 规范化。",
                "只有处理后的文字进入结构化记录，处理前后文字和规则版本没有一起保存。",
            ],
            "anchors": [
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "normalize_transcript(&transcript)")},
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "text: transcript")},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "language_and_context_version_per_chunk",
            "availability": "RECORDING_LEVEL_OR_RUNTIME_ONLY_FAIL",
            "facts": [
                "语言在推理时从全局状态读取，没有复制到 TranscriptUpdate。",
                "录音级上下文可以有 SHA，但没有逐 chunk 保存语言和上下文版本的关联。",
            ],
            "anchors": [
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "get_language_preference_internal")},
                {"path": worker_path.as_posix(), "lines": _line_matches(worker_source, "Recognition context enabled")},
            ],
            "structured_contract_present": False,
        },
        {
            "field": "capture_pcm_sha256",
            "availability": "MISSING_FOR_REALTIME_ENGINE_INPUT_FAIL",
            "facts": [
                "计划文档之外的生产和 QA 源码没有定义 capture_pcm_sha256。",
                "Q-00 正式执行器散列的是导入窗口，不是实时转写链路实际收到的 PCM。",
            ],
            "anchors": capture_pcm_occurrences,
            "structured_contract_present": bool(capture_pcm_occurrences),
        },
        {
            "field": "load_timeline",
            "availability": "MISSING_FAIL",
            "facts": [
                "计划文档之外的生产和 QA 源码没有定义 load_timeline_sha256，也没有连续 CPU/GPU/内存时间线合同。",
            ],
            "anchors": load_timeline_occurrences,
            "structured_contract_present": bool(load_timeline_occurrences),
        },
        {
            "field": "stop_queue_and_save_stage_timeline",
            "availability": "COARSE_PROGRESS_ONLY_FAIL",
            "facts": [
                "停止流程等待转写时只发送粗粒度阶段和汇总耗时秒数。",
                "200 毫秒收尾、检查点合并/最终化、文字写入、元数据完成和最终事件没有绑定的逐阶段单调时间线。",
            ],
            "anchors": [
                {"path": recording_commands_path.as_posix(), "lines": _line_matches(recording_commands_source, '"processing_transcripts"')},
                {"path": saver_path.as_posix(), "lines": _line_matches(saver_source, "from_millis(200)")},
                {"path": saver_path.as_posix(), "lines": _line_matches(saver_source, "saver.finalize().await")},
                {"path": saver_path.as_posix(), "lines": _line_matches(saver_source, "complete_metadata(recording_duration)")},
            ],
            "structured_contract_present": False,
        },
    ]

    blockers = [
        "不存在可复现的正式实时 audio_input_path_id；Q-00 走的是 import-audio 再 moss-complete。",
        "冻结源音频、窗口音频、人工逐字稿、说话人真值和正例术语原件没有跟随当前检出；没有另行提供批准原件时，本检查点只能核对登记散列，不能重算原件。",
        "实时引擎入口的 capture_pcm_sha256 取不到。",
        "逐 chunk 痕迹没有端到端落盘；全部必需字段仍是失败或部分失败。",
        "不存在连续负载时间线合同和 load_timeline_sha256。",
        "输入和痕迹合同未就绪，因此没有启动三次冷热基线；本检查点不声称得到 CER、积压、延迟、排空或保存耗时结果。",
    ]

    source_audio_candidates = [
        path.as_posix()
        for path in tracked
        if path.suffix.lower() in {".wav", ".wave"}
    ]
    missing_checked_in_truth_roles = [
        row["role"]
        for row in checkout_artifacts
        if not row["checked_in_at_manifest_relative_path"]
    ]
    exact_tokens = {
        "audio_input_path_id": audio_input_occurrences,
        "capture_pcm_sha256": capture_pcm_occurrences,
        "load_timeline_sha256": load_timeline_occurrences,
        "baseline_snapshot_id": baseline_snapshot_occurrences,
    }

    result: dict[str, Any] = {
        "schema_version": 1,
        "stage": "D11_REALTIME_BASELINE_READINESS_AUDIT",
        "status": "DIAGNOSTIC_CHECKPOINT_ONLY",
        "d11_state": "IN_PROGRESS",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "scope": {
            "task": "D-11",
            "production_behavior_modified": False,
            "hardware_test_waited_for": False,
            "i11a_started": False,
            "formal_product_data_accessed": False,
            "untracked_historical_draft_accessed": False,
        },
        "git_origin": {
            "head": head,
            "current_branch": branch,
            "required_base_branch": BASE_BRANCH,
            "required_base_commit": BASE_COMMIT,
            "required_base_branch_tip": base_tip,
            "merge_base_with_required_branch": merge_base,
            "started_from_required_commit": base_tip == BASE_COMMIT and merge_base == BASE_COMMIT,
        },
        "frozen_q00_registration": {
            "status": "PARTIAL_BYTES_AVAILABLE_IN_CHECKOUT",
            "source": {
                "sha256": constants["SOURCE_SHA256"],
                "bytes": constants["SOURCE_BYTES"],
                "duration_seconds": constants["SOURCE_DURATION_SECONDS"],
                "checked_in_audio_candidates": source_audio_candidates,
                "actual_bytes_rehashed_in_this_checkpoint": False,
            },
            "window": {
                "start_seconds": constants["WINDOW_START_SECONDS"],
                "end_seconds": constants["WINDOW_END_SECONDS"],
                "duration_seconds": constants["WINDOW_DURATION_SECONDS"],
                "sample_rate_hz": constants["SAMPLE_RATE_HZ"],
                "channels": constants["CHANNELS"],
                "sample_width_bytes": constants["SAMPLE_WIDTH_BYTES"],
                "frame_count": constants["WINDOW_FRAME_COUNT"],
                "bytes": constants["WINDOW_BYTES"],
                "sha256": constants["WINDOW_SHA256"],
                "actual_bytes_rehashed_in_this_checkpoint": False,
            },
            "truth_manifest": manifest_record,
            "negative_truth": negative_record,
            "positive_terms_declared_by_q00_code": positive_terms,
            "positive_truth_actual_bytes_rehashed_in_this_checkpoint": False,
            "negative_terms_rehashed_from_checked_in_truth": [
                row.get("term") for row in negative_truth.get("terms", []) if isinstance(row, dict)
            ],
            "checks": frozen_checks,
            "artifact_checkout_inventory": checkout_artifacts,
            "missing_checked_in_truth_roles": missing_checked_in_truth_roles,
        },
        "q00_cer_implementation": {
            "status": "IMPLEMENTATION_HASHED_BUT_RULE_HASH_NOT_PUBLISHED_BY_Q00",
            "scorer": _file_record(repo, Q00_SCORER),
            "functions": functions,
            "function_bundle_sha256_lf_utf8": _sha256_bytes(cer_bundle),
            "normalization_rule": cer_rule,
            "derived_cer_normalization_rule_sha256": _canonical_json_sha256(cer_rule),
            "derived_hash_method": "SHA-256 of UTF-8 canonical JSON with sorted keys and compact separators",
            "q00_public_report_contains_cer_rule_object": '"cer_rule"' in scorer_source,
            "q00_publishes_cer_normalization_rule_sha256": "cer_normalization_rule_sha256" in scorer_source,
        },
        "formal_realtime_input_contract": {
            "status": "FAIL_MISSING",
            "q00_formal_runner": _file_record(repo, Q00_FORMAL_RUNNER),
            "q00_cdp_actions": q00_actions,
            "q00_uses_import_audio": "import-audio" in q00_actions,
            "q00_uses_live_recording_input": any(
                action in q00_actions
                for action in ("start-recording", "recording-start", "inject-live-audio")
            ),
            "audio_input_path_id_occurrences_outside_plans": audio_input_occurrences,
            "conclusion": (
                "Q-00 proves a prepared-window import and MOSS batch path. It does not define "
                "a reproducible injection contract through the formal real-time recording path."
            ),
        },
        "traceability": {
            "status": "FAIL_INCOMPLETE",
            "source_file_count_scanned": len(source_files),
            "source_roots": [root.as_posix() for root in SOURCE_SCAN_ROOTS],
            "audio_chunk_has_chunk_id": "chunk_id" in audio_chunk,
            "transcript_update_has_chunk_id": "chunk_id" in transcript_update,
            "exact_contract_token_occurrences": exact_tokens,
            "fields": trace_matrix,
            "required_field_count": len(trace_matrix),
            "fully_available_field_count": sum(
                1 for row in trace_matrix if row["availability"] == "AVAILABLE"
            ),
        },
        "baseline_execution": {
            "status": "NOT_RUN_CONTRACT_BLOCKED",
            "baseline_snapshot_id": None,
            "comparison_id": None,
            "cold_run_count": 0,
            "warm_run_count": 0,
            "cer": None,
            "terminology_accuracy": None,
            "negative_term_insertions": None,
            "streaming_compute_rtf": None,
            "max_chunk_lag_seconds": None,
            "tail_drain_seconds": None,
            "maximum_queue_length": None,
            "remaining_queue_at_stop": None,
            "save_stage_timeline": None,
            "attribution_claims_made": 0,
        },
        "blockers": blockers,
        "blocker_count": len(blockers),
        "ready_for_three_run_baseline": False,
    }
    return result


def _write_json(path: Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(document, ensure_ascii=False, indent=2) + "\n"
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(payload, encoding="utf-8", newline="\n")
    temporary.replace(path)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output", type=Path)
    return parser


def main() -> int:
    arguments = build_parser().parse_args()
    result = collect_audit(arguments.repo)
    if arguments.output is not None:
        output = _validated_output_path(arguments.repo, arguments.output)
        _write_json(output, result)
    print(
        json.dumps(
            {
                "status": result["status"],
                "d11_state": result["d11_state"],
                "blocker_count": result["blocker_count"],
                "ready_for_three_run_baseline": result["ready_for_three_run_baseline"],
            },
            ensure_ascii=False,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
