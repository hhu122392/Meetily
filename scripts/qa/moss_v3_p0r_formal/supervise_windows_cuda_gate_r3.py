#!/usr/bin/env python3
"""Supervise the only admissible S8-M00-R3 CUDA worker execution.

The supervisor creates a fresh run directory, copies only manifest-approved
inputs to a read-only snapshot, captures child stdout/stderr at the OS process
boundary, applies a temporary machine-wide Windows outbound block, enforces
timeouts, and records the real child exit code and wall clock.  The private key
is never present while model code runs: post-run signing and final scoring are
separate commands executed only after the child is gone.
"""

from __future__ import annotations

import argparse
import csv
import ctypes
import hashlib
import io
import json
import math
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import threading
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import psutil

import score_s8_m00_r3 as scorer


SUPERVISOR_SOURCE = "SUPERVISED_WINDOWS_CUDA_RUNNER"
RUNNER_SOURCE = "AUTOMATIC_WINDOWS_CUDA_RUNNER"
CREATE_SUSPENDED = 0x00000004
MAX_CAPTURE_BYTES_PER_STREAM = 16 * 1024 * 1024


def now_iso() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat()


def write_new_json(path: Path, value: dict[str, Any]) -> None:
    if path.exists():
        raise scorer.ScoringError(f"拒绝覆盖已有证据: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        stream.write(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False))
        stream.write("\n")


def update_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(path.suffix + ".partial")
    partial.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n",
        encoding="utf-8",
    )
    os.replace(partial, path)


def path_is_inside(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=True).relative_to(root.resolve(strict=True))
        return True
    except ValueError:
        return False


def canonical_path_sha256(path: Path) -> str:
    canonical = os.path.normcase(str(path.resolve(strict=False)))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def require_external_run_ledger(ledger_dir: Path, workspace: Path) -> Path:
    resolved = ledger_dir.resolve(strict=True)
    if not resolved.is_dir():
        raise scorer.ScoringError("run ledger 必须是已存在目录")
    if path_is_inside(resolved, workspace):
        raise scorer.ScoringError("run ledger 必须放在工作区之外")
    return resolved


def require_signing_key_outside_workspace(private_key: Path, workspace: Path) -> None:
    if not private_key.is_file():
        raise scorer.ScoringError("签名私钥不存在")
    if path_is_inside(private_key, workspace):
        raise scorer.ScoringError("签名私钥必须放在工作区和证据目录之外")


def verify_private_public_pair(
    private_key: Path, public_key: Path, ssh_keygen_path: Path
) -> None:
    derived = subprocess.run(
        [str(ssh_keygen_path), "-y", "-f", str(private_key)],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
    )
    if derived.returncode != 0:
        raise scorer.ScoringError("无法读取签名私钥对应的公钥")
    expected = public_key.read_text(encoding="utf-8-sig").strip().split()
    actual = derived.stdout.strip().split()
    if len(expected) < 2 or len(actual) < 2 or expected[:2] != actual[:2]:
        raise scorer.ScoringError("签名私钥与冻结公钥不匹配")


def sign_file(
    payload: Path,
    signature_out: Path,
    private_key: Path,
    namespace: str,
    ssh_keygen_path: Path,
) -> None:
    default_signature = Path(str(payload) + ".sig")
    if signature_out.exists() or default_signature.exists():
        raise scorer.ScoringError("签名输出已存在，拒绝覆盖")
    completed = subprocess.run(
        [
            str(ssh_keygen_path),
            "-Y",
            "sign",
            "-f",
            str(private_key),
            "-n",
            namespace,
            str(payload),
        ],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=60,
    )
    if completed.returncode != 0 or not default_signature.is_file():
        raise scorer.ScoringError("证据签名失败")
    if default_signature.resolve() != signature_out.resolve(strict=False):
        signature_out.parent.mkdir(parents=True, exist_ok=True)
        os.replace(default_signature, signature_out)


def input_paths(args: argparse.Namespace) -> dict[str, Path]:
    return {
        "window_audio": args.window_audio,
        "full_audio": args.full_audio,
        "source_lock": args.source_lock,
        "verbatim": args.verbatim,
        "speaker_turns": args.turns,
        "human_review": args.review,
        "hotwords": args.hotwords,
        "pre_meeting_context": args.pre_meeting_context,
        "rules": args.rules,
        "whisper_full": args.whisper_full,
        "whisper_run_record": args.whisper_run_record,
        "whisper_model": args.whisper_model,
        "stable_exe": args.stable_exe,
        "prompt": args.prompt,
        "model_manifest": args.model_manifest,
        "runtime_lock": args.runtime_lock,
        "runtime_bootstrap": args.runtime_bootstrap,
        "scorer": args.scorer,
        "runner": args.runner,
        "supervisor": Path(__file__).resolve(),
        "pip_freeze": args.pip_freeze,
        "attestation_public_key": args.attestation_public_key,
    }


def validate_static_inputs(args: argparse.Namespace) -> tuple[dict[str, str], str]:
    paths = input_paths(args)
    for name, path in paths.items():
        if not path.is_file():
            raise scorer.ScoringError(f"输入不存在: {name}: {path}")
    if args.scorer.resolve() != Path(scorer.__file__).resolve():
        raise scorer.ScoringError("supervisor 实际导入的 scorer 与传入 scorer 不一致")
    rules = scorer.load_json(args.rules)
    scorer.require_frozen_policy(rules)
    runtime_lock = scorer.load_json(args.runtime_lock)
    scorer.verify_runtime_integrity(runtime_lock, require_isolated_python=False)
    locked_git = scorer.locked_external_tool(runtime_lock, "git")
    tool_lock = rules["tool_lock"]
    identity_binding = scorer.reviewer_signer_identity_binding(
        scorer.load_json(args.review),
        scorer.load_json(args.hotwords),
        scorer.load_json(args.pre_meeting_context),
        rules,
    )
    if not identity_binding["valid"]:
        raise scorer.ScoringError("人工真值、热词、会前上下文与签名身份不是同一复核人")
    hashes = {name: scorer.sha256_file(path) for name, path in paths.items()}
    if (
        hashes["scorer"].casefold() != str(tool_lock["scorer_sha256"]).casefold()
        or hashes["runner"].casefold() != str(tool_lock["runner_sha256"]).casefold()
        or hashes["supervisor"].casefold()
        != str(tool_lock["supervisor_sha256"]).casefold()
        or hashes["runtime_bootstrap"].casefold()
        != str(runtime_lock["entry_scripts"]["bootstrap"]["sha256"]).casefold()
        or hashes["runner"].casefold()
        != str(runtime_lock["entry_scripts"]["runner"]["sha256"]).casefold()
        or hashes["scorer"].casefold()
        != str(runtime_lock["entry_scripts"]["scorer"]["sha256"]).casefold()
        or hashes["supervisor"].casefold()
        != str(runtime_lock["entry_scripts"]["supervisor"]["sha256"]).casefold()
        or hashes["attestation_public_key"].casefold()
        != str(rules["attestation"]["public_key_sha256"]).casefold()
    ):
        raise scorer.ScoringError("工具或签名公钥与冻结规则不一致")
    source_head = scorer.git_head(args.source_code, locked_git)
    if source_head != rules["model_lock"]["official_code_commit"]:
        raise scorer.ScoringError("MOSS 源码 commit 与冻结规则不一致")
    if scorer.git_status_porcelain(args.source_code, locked_git):
        raise scorer.ScoringError("MOSS 源码工作树不干净")
    return hashes, source_head


def _json_file_bytes(value: dict[str, Any]) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    ).encode("utf-8")


def require_r3_source_draft(source_draft: dict[str, Any]) -> tuple[float, float]:
    """Validate the R3 provenance-aware source lock before final freezing."""

    if (
        source_draft.get("schema_version") != 3
        or source_draft.get("stage") != "S8-M00-R3"
        or source_draft.get("status")
        != "CANDIDATE_INPUTS_HASHED_CONTEXT_PROVENANCE_VALIDATED"
    ):
        raise scorer.ScoringError("source lock 不是 R3 待最终封存的来源审计候选状态")
    selected = source_draft.get("selected_annotation_audio")
    if not isinstance(selected, dict):
        raise scorer.ScoringError("R3 source lock 缺少人工窗口")
    try:
        start = float(selected["source_start_seconds"])
        end = float(selected["source_end_seconds"])
    except (KeyError, TypeError, ValueError) as exc:
        raise scorer.ScoringError("R3 人工窗口缺少合法的完整录音起止时间") from exc
    if not math.isfinite(start) or not math.isfinite(end) or start < 0 or end <= start:
        raise scorer.ScoringError("R3 人工窗口起止时间无效")
    return start, end


def freeze_policy(args: argparse.Namespace) -> int:
    """Create final source/rule locks without hand-editing hashes or statuses."""
    for output in (args.source_lock_out, args.rules_out, args.audit_out):
        if output.exists():
            raise scorer.ScoringError(f"拒绝覆盖已有冻结输出: {output}")
    required_files = {
        "source_lock_draft": args.source_lock_draft,
        "rules_draft": args.rules_draft,
        "window_audio": args.window_audio,
        "full_audio": args.full_audio,
        "verbatim": args.verbatim,
        "turns": args.turns,
        "review": args.review,
        "hotwords": args.hotwords,
        "pre_meeting_context": args.pre_meeting_context,
        "whisper_full": args.whisper_full,
        "whisper_run_record": args.whisper_run_record,
        "whisper_model": args.whisper_model,
        "stable_exe": args.stable_exe,
        "model_manifest": args.model_manifest,
        "runtime_lock": args.runtime_lock,
        "runtime_bootstrap": args.runtime_bootstrap,
        "scorer": args.scorer,
        "runner": args.runner,
        "whisper_supervisor": args.whisper_supervisor,
        "pip_freeze": args.pip_freeze,
        "attestation_public_key": args.attestation_public_key,
    }
    for name, path in required_files.items():
        if not path.is_file():
            raise scorer.ScoringError(f"冻结输入不存在: {name}: {path}")
    if not args.model_root.is_dir() or not args.source_code.is_dir():
        raise scorer.ScoringError("MOSS 模型目录或官方源码目录不存在")
    if args.scorer.resolve() != Path(scorer.__file__).resolve():
        raise scorer.ScoringError("freeze-policy 实际导入的 scorer 与传入文件不一致")

    source_draft = scorer.load_json(args.source_lock_draft)
    rules = scorer.load_json(args.rules_draft)
    window_start, window_end = require_r3_source_draft(source_draft)
    if rules.get("status") != "DRAFT_UNDER_AUDIT":
        raise scorer.ScoringError("评分规则不是待最终封存的草稿状态")

    runtime_lock = scorer.load_json(args.runtime_lock)
    scorer.require_runtime_lock_schema(runtime_lock)
    runtime_integrity = scorer.verify_runtime_integrity(
        runtime_lock, require_isolated_python=False
    )
    if runtime_integrity.get("valid") is not True:
        raise scorer.ScoringError("目标运行环境与 runtime lock 不一致")
    runtime_scripts = runtime_lock["entry_scripts"]
    runtime_script_paths = {
        "bootstrap": args.runtime_bootstrap,
        "runner": args.runner,
        "scorer": args.scorer,
        "supervisor": Path(__file__).resolve(),
    }
    for name, path in runtime_script_paths.items():
        if scorer.sha256_file(path).casefold() != str(
            runtime_scripts[name]["sha256"]
        ).casefold():
            raise scorer.ScoringError(f"{name} 与 runtime lock 不一致")
    if scorer.sha256_file(args.pip_freeze).casefold() != str(
        runtime_lock["pip_freeze_sha256"]
    ).casefold():
        raise scorer.ScoringError("pip freeze 与 runtime lock 不一致")

    model_audit = scorer.verify_model_manifest(args.model_root, args.model_manifest)
    if model_audit.get("matches") is not True:
        raise scorer.ScoringError("MOSS 模型目录与冻结清单不一致")
    locked_git = scorer.locked_external_tool(runtime_lock, "git")
    source_head = scorer.git_head(args.source_code, locked_git)
    if scorer.git_status_porcelain(args.source_code, locked_git):
        raise scorer.ScoringError("MOSS 官方源码工作树不干净")
    model_lock = rules.get("model_lock", {})
    if (
        source_head != model_lock.get("official_code_commit")
        or args.model_root.name != model_lock.get("model_revision")
        or scorer.sha256_file(args.model_manifest).casefold()
        != str(model_lock.get("model_manifest_file_sha256", "")).casefold()
        or str(model_audit.get("actual_digest", "")).casefold()
        != str(model_lock.get("model_directory_canonical_sha256", "")).casefold()
    ):
        raise scorer.ScoringError("MOSS 代码、revision 或模型清单与规则草稿不一致")

    full_audio_sha = scorer.sha256_file(args.full_audio)
    window_audio_sha = scorer.sha256_file(args.window_audio)
    full_duration = scorer.wav_duration_seconds(args.full_audio)
    window_duration = scorer.wav_duration_seconds(args.window_audio)
    pcm_slice = scorer.verify_window_pcm_slice(
        args.full_audio,
        args.window_audio,
        window_start,
        window_end,
    )
    if not pcm_slice.get("valid"):
        raise scorer.ScoringError("人工窗口不是完整录音的精确 PCM 切片")
    if (
        not math.isclose(
            full_duration,
            float(rules.get("full_audio_duration_seconds", math.nan)),
            abs_tol=0.001,
        )
        or not math.isclose(
            window_duration,
            float(rules.get("audio_window_duration_seconds", math.nan)),
            abs_tol=0.001,
        )
    ):
        raise scorer.ScoringError("音频实际时长与规则草稿不一致")

    whisper_full = scorer.load_json(args.whisper_full)
    whisper_run = scorer.load_json(args.whisper_run_record)
    scorer.require_whisper_baseline_schema(
        whisper_full, "FORMAL_CURRENT_WHISPER_FULL_OUTPUT"
    )
    scorer.require_whisper_run_record_schema(whisper_run)
    whisper_full_sha = scorer.sha256_file(args.whisper_full)
    whisper_run_sha = scorer.sha256_file(args.whisper_run_record)
    whisper_model_sha = scorer.sha256_file(args.whisper_model)
    stable_exe_sha = scorer.sha256_file(args.stable_exe)
    if (
        whisper_full.get("run_id") != whisper_run.get("run_id")
        or str(whisper_full.get("source_full_audio_sha256", "")).casefold()
        != full_audio_sha.casefold()
        or str(whisper_full.get("clip_audio_sha256", "")).casefold()
        != full_audio_sha.casefold()
        or str(whisper_full.get("model_file_sha256", "")).casefold()
        != whisper_model_sha.casefold()
        or str(whisper_full.get("stable_exe_sha256", "")).casefold()
        != stable_exe_sha.casefold()
        or str(whisper_run.get("source_full_audio_sha256", "")).casefold()
        != full_audio_sha.casefold()
        or str(whisper_run.get("window_audio_sha256", "")).casefold()
        != window_audio_sha.casefold()
        or str(whisper_run.get("model_file_sha256", "")).casefold()
        != whisper_model_sha.casefold()
        or str(whisper_run.get("stable_exe_sha256", "")).casefold()
        != stable_exe_sha.casefold()
        or str(whisper_run.get("output_full_sha256", "")).casefold()
        != whisper_full_sha.casefold()
        or str(whisper_run.get("raw_product_output_sha256", "")).casefold()
        != str(whisper_full.get("source_transcripts_sha256", "")).casefold()
        or str(whisper_run.get("command_sha256", "")).casefold()
        != str(whisper_full.get("source_producer_command_sha256", "")).casefold()
    ):
        raise scorer.ScoringError("正式 Whisper 输出、运行记录、音频、模型或 EXE 没有互相绑定")

    source_final = json.loads(json.dumps(source_draft, ensure_ascii=False))
    source_final_locked_at = now_iso()
    source_final.update(
        {
            "updated_at": source_final_locked_at,
            "final_locked_at": source_final_locked_at,
            "status": "FINAL_LOCKED_BEFORE_RULES_AND_CUDA_OUTPUT",
            "change_reason": "由 freeze-policy 机械核对真实音频、正式 Whisper、模型、工具和 runtime 后生成；禁止手工补哈希。",
        }
    )
    source_final["source_audio"].update(
        {
            "path": str(args.full_audio.resolve()),
            "duration_seconds": full_duration,
            "bytes": args.full_audio.stat().st_size,
            "sha256": full_audio_sha,
            "read_only_source": True,
            "modified_by_this_task": False,
        }
    )
    source_final["selected_annotation_audio"].update(
        {
            "path": str(args.window_audio.resolve()),
            "duration_seconds": window_duration,
            "bytes": args.window_audio.stat().st_size,
            "sha256": window_audio_sha,
            "gate_input": True,
            "role": "APPROVED_HUMAN_MULTI_SPEAKER_GATE_WINDOW",
        }
    )
    source_final["formal_current_whisper_baseline"] = {
        "run_id": whisper_run["run_id"],
        "full_output_path": str(args.whisper_full.resolve()),
        "full_output_sha256": whisper_full_sha,
        "run_record_path": str(args.whisper_run_record.resolve()),
        "run_record_sha256": whisper_run_sha,
        "model_path": str(args.whisper_model.resolve()),
        "model_sha256": whisper_model_sha,
        "stable_exe_path": str(args.stable_exe.resolve()),
        "stable_exe_sha256": stable_exe_sha,
    }
    source_final_bytes = _json_file_bytes(source_final)
    source_final_sha = hashlib.sha256(source_final_bytes).hexdigest()

    rules_final = json.loads(json.dumps(rules, ensure_ascii=False))
    rules_final["status"] = "FROZEN_BEFORE_CUDA_OUTPUT"
    rules_final["frozen_at"] = now_iso()
    rules_final["input_lock"].update(
        {
            "source_lock_file_sha256": source_final_sha,
            "full_audio_sha256": full_audio_sha,
            "window_audio_sha256": window_audio_sha,
            "whisper_full_baseline_sha256": whisper_full_sha,
            "whisper_run_record_sha256": whisper_run_sha,
            "whisper_model_sha256": whisper_model_sha,
            "stable_exe_sha256": stable_exe_sha,
            "pre_meeting_context_sha256": scorer.sha256_file(
                args.pre_meeting_context
            ),
            "status": "FROZEN_BEFORE_CUDA_OUTPUT",
        }
    )
    rules_final["runtime_lock"].update(
        {
            "status": "FROZEN_ON_TARGET_BEFORE_MODEL_OUTPUT",
            "file_sha256": scorer.sha256_file(args.runtime_lock),
        }
    )
    rules_final["tool_lock"].update(
        {
            "status": "FROZEN_BEFORE_CUDA_OUTPUT",
            "scorer_sha256": scorer.sha256_file(args.scorer),
            "runner_sha256": scorer.sha256_file(args.runner),
            "supervisor_sha256": scorer.sha256_file(Path(__file__).resolve()),
            "whisper_supervisor_sha256": scorer.sha256_file(
                args.whisper_supervisor
            ),
        }
    )
    rules_final["attestation"].update(
        {
            "status": "FROZEN_EXTERNAL_REVIEWER_KEY",
            "public_key_sha256": scorer.sha256_file(args.attestation_public_key),
            "signer_identity": args.signer_identity.strip(),
        }
    )
    scorer.require_frozen_policy(rules_final)
    scorer.require_final_source_lock(source_final, rules_final)

    review = scorer.load_json(args.review)
    hotwords = scorer.load_json(args.hotwords)
    context = scorer.load_json(args.pre_meeting_context)
    identity = scorer.reviewer_signer_identity_binding(
        review, hotwords, context, rules_final
    )
    if not identity["valid"]:
        raise scorer.ScoringError("人工真值、热词、会前上下文和签名身份不一致")
    truth_segments = scorer.truth_text_segments_from_tsv(args.verbatim)
    hotword_audit = scorer.validate_hotwords(
        hotwords,
        truth_segments,
        rules_final,
        [
            segment["text"]
            for segment in scorer.parse_hypothesis_segments(whisper_full)
        ],
    )
    if not hotword_audit["valid"]:
        raise scorer.ScoringError("正负热词未通过最终冻结校验")
    context_audit = scorer.validate_pre_meeting_context(
        context,
        scorer.sha256_file(args.pre_meeting_context),
        hotwords,
        rules_final,
        whisper_run,
    )
    if not context_audit["valid"]:
        raise scorer.ScoringError("会前名单/术语表未通过防答案泄漏校验")

    with tempfile.TemporaryDirectory(prefix="meetily-r3-freeze-policy-") as temp_dir:
        temp_rules = Path(temp_dir) / "rules.json"
        temp_rules.write_bytes(_json_file_bytes(rules_final))
        annotation = scorer.validate_annotation_kit(
            args.window_audio,
            args.verbatim,
            args.turns,
            args.review,
            temp_rules,
        )
    if annotation.get("validation_result") != "STRUCTURE_PASS_SELF_ATTESTED":
        raise scorer.ScoringError("人工基准未通过最终冻结校验")

    audit = {
        "schema_version": 1,
        "role": "R3_POLICY_FREEZE_AUDIT",
        "created_at": now_iso(),
        "go_allowed": False,
        "source_lock_sha256": source_final_sha,
        "rules_sha256": hashlib.sha256(_json_file_bytes(rules_final)).hexdigest(),
        "runtime_lock_sha256": scorer.sha256_file(args.runtime_lock),
        "whisper_full_sha256": whisper_full_sha,
        "whisper_run_record_sha256": whisper_run_sha,
        "stable_exe_sha256": stable_exe_sha,
        "whisper_model_sha256": whisper_model_sha,
        "pcm_window_is_exact_slice": True,
        "annotation_validation_result": annotation["validation_result"],
        "hotwords_valid": True,
        "pre_meeting_context_valid": True,
        "identity_binding": identity,
        "runtime_integrity": runtime_integrity,
        "model_manifest": model_audit,
        "truth_boundary": "程序只能验证文件、时序、哈希和声明一致；真人听写语义由同一签名身份负责。",
    }
    write_new_json(args.source_lock_out, source_final)
    if scorer.sha256_file(args.source_lock_out) != source_final_sha:
        raise scorer.ScoringError("最终 source lock 落盘哈希不一致")
    write_new_json(args.rules_out, rules_final)
    write_new_json(args.audit_out, audit)
    print(
        json.dumps(
            {
                "decision": "POLICY_FROZEN_BEFORE_CUDA_OUTPUT",
                "go_allowed": False,
                "source_lock": str(args.source_lock_out.resolve()),
                "source_lock_sha256": scorer.sha256_file(args.source_lock_out),
                "rules": str(args.rules_out.resolve()),
                "rules_sha256": scorer.sha256_file(args.rules_out),
                "audit": str(args.audit_out.resolve()),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


def seal_inputs(args: argparse.Namespace) -> int:
    if not re.fullmatch(r"[0-9a-f]{32}", args.run_id):
        raise scorer.ScoringError("run_id 必须是 32 位小写十六进制")
    require_signing_key_outside_workspace(args.private_key, args.workspace_root)
    runtime_lock = scorer.load_json(args.runtime_lock)
    ssh_keygen_path = scorer.locked_external_tool(runtime_lock, "ssh_keygen")
    verify_private_public_pair(
        args.private_key, args.attestation_public_key, ssh_keygen_path
    )
    hashes, source_head = validate_static_inputs(args)
    if args.run_dir.exists():
        raise scorer.ScoringError("seal-inputs 绑定的 run-dir 必须尚不存在")
    ledger_dir = require_external_run_ledger(args.run_ledger_dir, args.workspace_root)
    bundle = {
        "schema_version": 1,
        "role": "SIGNED_PRE_RUN_INPUT_BUNDLE",
        "run_id": args.run_id,
        "created_at": now_iso(),
        "source_code_commit": source_head,
        "source_code_clean": True,
        "expected_run_dir_sha256": canonical_path_sha256(args.run_dir),
        "run_ledger_dir_sha256": canonical_path_sha256(ledger_dir),
        "single_execution_policy": "RUN_ID_AND_RUN_DIR_BOUND_ONE_ATTEMPT_ONLY",
        "artifact_hashes": hashes,
        "truth_boundary": "签名证明签名者批准这些字节；不能由程序证明听写内容与音频语义一致。",
    }
    write_new_json(args.bundle_out, bundle)
    rules = scorer.load_json(args.rules)
    sign_file(
        args.bundle_out,
        args.signature_out,
        args.private_key,
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    verified = scorer.verify_ssh_signature(
        args.bundle_out,
        args.signature_out,
        args.attestation_public_key,
        str(rules["attestation"]["signer_identity"]),
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    if verified.get("valid") is not True:
        raise scorer.ScoringError("预运行签名写出后复核失败")
    print(
        json.dumps(
            {
                "decision": "PRE_RUN_INPUTS_SIGNED",
                "run_id": args.run_id,
                "bundle_sha256": scorer.sha256_file(args.bundle_out),
                "signature_sha256": scorer.sha256_file(args.signature_out),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


def make_read_only_tree(root: Path) -> None:
    for path in sorted(root.rglob("*"), reverse=True):
        try:
            if path.is_file():
                path.chmod(stat.S_IREAD)
        except OSError as exc:
            raise scorer.ScoringError(f"无法将快照设为只读: {path}: {exc}") from exc


def exact_directory_manifest(root: Path) -> dict[str, Any]:
    if not root.is_dir():
        raise scorer.ScoringError(f"快照目录不存在: {root}")
    entries: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*"), key=lambda item: item.relative_to(root).as_posix()):
        if path.is_symlink():
            raise scorer.ScoringError(f"快照目录不允许符号链接: {path}")
        if path.is_file():
            entries.append(
                {
                    "relative_path": path.relative_to(root).as_posix(),
                    "bytes": path.stat().st_size,
                    "sha256": scorer.sha256_file(path),
                }
            )
    canonical = json.dumps(
        entries, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return {
        "file_count": len(entries),
        "total_bytes": sum(item["bytes"] for item in entries),
        "canonical_sha256": hashlib.sha256(canonical).hexdigest(),
        "files": entries,
    }


def copy_manifest_model(
    model_root: Path, manifest_path: Path, destination: Path
) -> dict[str, Any]:
    manifest = scorer.load_json(manifest_path)
    entries = manifest.get("files")
    if not isinstance(entries, list) or not entries:
        raise scorer.ScoringError("模型清单为空，拒绝建立运行快照")
    destination.mkdir(parents=True, exist_ok=False)
    for entry in entries:
        if not isinstance(entry, dict):
            raise scorer.ScoringError("模型清单项目格式错误")
        relative = Path(str(entry.get("relative_path", "")))
        if relative.is_absolute() or ".." in relative.parts or not relative.parts:
            raise scorer.ScoringError("模型清单包含越界路径")
        source = model_root / relative
        target = destination / relative
        if not source.is_file() or source.is_symlink():
            raise scorer.ScoringError(f"模型清单文件不存在或为链接: {relative}")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    verification = scorer.verify_model_manifest(destination, manifest_path)
    if not verification["matches"]:
        raise scorer.ScoringError("按清单建立的模型快照复核失败")
    return {
        "method": "COPY_ONLY_FILES_LISTED_IN_FROZEN_MODEL_MANIFEST",
        "file_count": verification["actual_file_count"],
        "total_bytes": verification["actual_total_bytes"],
        "canonical_sha256": verification["actual_digest"],
    }


def export_git_commit(
    source_code: Path, commit: str, destination: Path, git_executable: Path
) -> dict[str, Any]:
    completed = subprocess.run(
        [str(git_executable), "-C", str(source_code), "archive", "--format=tar", commit],
        check=False,
        capture_output=True,
        timeout=120,
    )
    if completed.returncode != 0:
        raise scorer.ScoringError("git archive 导出冻结 commit 失败")
    destination.mkdir(parents=True, exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(completed.stdout), mode="r:") as archive:
        members = archive.getmembers()
        for member in members:
            member_path = Path(member.name)
            if (
                member_path.is_absolute()
                or ".." in member_path.parts
                or member.issym()
                or member.islnk()
                or not (member.isfile() or member.isdir())
            ):
                raise scorer.ScoringError("git archive 包含不安全成员")
            target = destination / member_path
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            extracted = archive.extractfile(member)
            if extracted is None:
                raise scorer.ScoringError("git archive 文件读取失败")
            with target.open("xb") as stream:
                shutil.copyfileobj(extracted, stream)
    entries = []
    for path in sorted(
        (item for item in destination.rglob("*") if item.is_file()),
        key=lambda item: item.relative_to(destination).as_posix(),
    ):
        entries.append(
            {
                "relative_path": path.relative_to(destination).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": scorer.sha256_file(path),
            }
        )
    return {
        "method": "GIT_ARCHIVE_LOCKED_COMMIT_TRACKED_FILES_ONLY",
        "commit": commit,
        "file_count": len(entries),
        "total_bytes": sum(item["bytes"] for item in entries),
        "canonical_sha256": hashlib.sha256(
            json.dumps(
                entries, ensure_ascii=False, sort_keys=True, separators=(",", ":")
            ).encode("utf-8")
        ).hexdigest(),
        "ignored_and_untracked_files_copied": False,
    }


def snapshot_inputs(
    args: argparse.Namespace, run_dir: Path
) -> tuple[dict[str, Path], Path, Path, dict[str, Any]]:
    snapshot_root = run_dir / "input-snapshot"
    snapshot_root.mkdir(parents=True, exist_ok=False)
    originals = input_paths(args)
    copied: dict[str, Path] = {}
    # Whisper model and stable EXE are baseline provenance only, not read by MOSS.
    # Keep them on their frozen paths and verify pre/post hashes instead of copying
    # several extra GiB into every run.
    provenance_only = {"whisper_model", "stable_exe"}
    for name, source in originals.items():
        if name in provenance_only:
            copied[name] = source.resolve(strict=True)
            continue
        suffix = "".join(source.suffixes) or ".bin"
        destination = snapshot_root / f"{name}{suffix}"
        shutil.copy2(source, destination)
        if scorer.sha256_file(destination) != scorer.sha256_file(source):
            raise scorer.ScoringError(f"输入快照复制校验失败: {name}")
        copied[name] = destination

    model_snapshot = snapshot_root / "model" / args.model_root.name
    model_export = copy_manifest_model(
        args.model_root, args.model_manifest, model_snapshot
    )
    source_snapshot = snapshot_root / "source-code"
    rules = scorer.load_json(args.rules)
    source_export = export_git_commit(
        args.source_code,
        str(rules["model_lock"]["official_code_commit"]),
        source_snapshot,
        scorer.locked_external_tool(
            scorer.load_json(args.runtime_lock), "git"
        ),
    )
    make_read_only_tree(snapshot_root)
    return copied, model_snapshot, source_snapshot, {
        "source": source_export,
        "model": model_export,
    }


def _firewall_snapshot(powershell_path: Path, rule_names: list[str]) -> dict[str, Any]:
    quoted_names = ",".join("'" + name.replace("'", "''") + "'" for name in rule_names)
    command = (
        "$ErrorActionPreference='Stop'; "
        f"$names=@({quoted_names}); $items=@(); "
        "foreach($name in $names){$r=Get-NetFirewallRule -DisplayName $name "
        "-ErrorAction SilentlyContinue; if($null -ne $r){foreach($one in @($r)){"
        "$app=@($one | Get-NetFirewallApplicationFilter "
        "-ErrorAction SilentlyContinue); "
        "$program=if($app.Count -gt 0){[string]$app[0].Program}else{$null}; "
        "$programScope=if([string]::IsNullOrWhiteSpace($program)){'Unknown'} "
        "elseif($program -eq 'Any' -or $program -eq '*'){'Any'}else{'ExactPath'}; "
        "$programPathSha256=$null; if($programScope -eq 'ExactPath'){"
        "$normalized=[IO.Path]::GetFullPath($program).ToLowerInvariant(); "
        "$hasher=[Security.Cryptography.SHA256]::Create(); try{"
        "$bytes=[Text.Encoding]::UTF8.GetBytes($normalized); "
        "$programPathSha256=-join ($hasher.ComputeHash($bytes) | "
        "ForEach-Object {$_.ToString('x2')})}finally{$hasher.Dispose()}}; "
        "$items += [pscustomobject]@{DisplayName=[string]$one.DisplayName;"
        "Enabled=$one.Enabled.ToString();Direction=$one.Direction.ToString();"
        "Action=$one.Action.ToString();Profile=$one.Profile.ToString();"
        "ProgramScope=$programScope;ProgramPathSha256=$programPathSha256}}}}; "
        "$services=@(Get-Service -Name BFE,MpsSvc | ForEach-Object {"
        "[pscustomobject]@{Name=[string]$_.Name;Status=$_.Status.ToString()}}); "
        "$profiles=@(Get-NetFirewallProfile | ForEach-Object {"
        "[pscustomobject]@{Name=[string]$_.Name;Enabled=[bool]$_.Enabled}}); "
        "[pscustomobject]@{rules=$items;services=$services;profiles=$profiles} "
        "| ConvertTo-Json -Compress -Depth 4"
    )
    completed = subprocess.run(
        [str(powershell_path), "-NoProfile", "-NonInteractive", "-Command", command],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=20,
    )
    if completed.returncode != 0:
        raise scorer.ScoringError("无法读取临时防火墙规则状态")
    text = completed.stdout.strip()
    if not text:
        raise scorer.ScoringError("防火墙状态命令没有返回 JSON")
    parsed = json.loads(text)
    if not isinstance(parsed, dict):
        raise scorer.ScoringError("防火墙状态 JSON 结构错误")
    raw_records = parsed.get("rules", [])
    records = raw_records if isinstance(raw_records, list) else [raw_records]
    raw_services = parsed.get("services", [])
    services = raw_services if isinstance(raw_services, list) else [raw_services]
    raw_profiles = parsed.get("profiles", [])
    profiles = raw_profiles if isinstance(raw_profiles, list) else [raw_profiles]
    records.sort(key=lambda item: str(item.get("DisplayName", "")))
    services.sort(key=lambda item: str(item.get("Name", "")))
    profiles.sort(key=lambda item: str(item.get("Name", "")))
    return {
        "rule_names": sorted(rule_names),
        "records": records,
        "services": services,
        "profiles": profiles,
    }


def firewall_platform_is_effective(snapshot: dict[str, Any]) -> bool:
    services = snapshot.get("services", [])
    profiles = snapshot.get("profiles", [])
    return (
        {str(item.get("Name")) for item in services} == {"BFE", "MpsSvc"}
        and all(str(item.get("Status")) == "Running" for item in services)
        and {str(item.get("Name")) for item in profiles}
        == {"Domain", "Private", "Public"}
        and all(item.get("Enabled") is True for item in profiles)
    )


def outbound_block_probe(python_path: Path) -> dict[str, Any]:
    probe = (
        "import socket,sys; s=socket.socket(); s.settimeout(5); "
        "\ntry: s.connect(('1.1.1.1',443)); sys.exit(10)"
        "\nexcept OSError as e: sys.exit(0 if getattr(e,'winerror',None)==10013 else 11)"
        "\nfinally: s.close()"
    )
    completed = subprocess.run(
        [str(python_path), "-I", "-S", "-B", "-c", probe],
        check=False,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        timeout=15,
        env={
            "SYSTEMROOT": os.environ.get("SYSTEMROOT", r"C:\Windows"),
            "WINDIR": os.environ.get("WINDIR", r"C:\Windows"),
        },
    )
    return {
        "target": "1.1.1.1:443",
        "expected_windows_socket_error": 10013,
        "exit_code": completed.returncode,
        "blocked_by_firewall_policy": completed.returncode == 0,
        "stdout_sha256": hashlib.sha256(completed.stdout).hexdigest(),
        "stderr_sha256": hashlib.sha256(completed.stderr).hexdigest(),
    }


def firewall_add(
    run_id: str,
    powershell_path: Path,
    locked_python_path: Path,
    protected_programs: list[Path] | None = None,
) -> dict[str, Any]:
    if sys.platform != "win32":
        raise scorer.ScoringError("正式 supervisor 只接受 Windows")
    program_paths: list[Path] = []
    seen_programs: set[str] = set()
    if protected_programs is not None:
        for source in protected_programs:
            resolved = source.resolve(strict=True)
            identity = os.path.normcase(str(resolved))
            if identity in seen_programs:
                continue
            seen_programs.add(identity)
            program_paths.append(resolved)
        locked_python_identity = os.path.normcase(
            str(locked_python_path.resolve(strict=True))
        )
        if not program_paths or locked_python_identity not in seen_programs:
            raise scorer.ScoringError("按程序隔离必须包含受控联网探针")

    rule_specs: list[dict[str, str]] = []
    if program_paths:
        for index, program in enumerate(program_paths, start=1):
            for direction in ("Inbound", "Outbound"):
                short_direction = "IN" if direction == "Inbound" else "OUT"
                rule_specs.append(
                    {
                        "name": f"Meetily-R3-{run_id}-P{index:02d}-{short_direction}",
                        "direction": direction,
                        "program": str(program),
                    }
                )
        scope = "PROGRAM_SCOPED_ALL_PROFILES_BOTH_DIRECTIONS"
    else:
        rule_specs = [
            {
                "name": f"Meetily-R3-{run_id}-IN",
                "direction": "Inbound",
                "program": "Any",
            },
            {
                "name": f"Meetily-R3-{run_id}-OUT",
                "direction": "Outbound",
                "program": "Any",
            },
        ]
        scope = "MACHINE_WIDE_ALL_PROGRAMS_ALL_PROFILES_BOTH_DIRECTIONS"
    rule_names = [item["name"] for item in rule_specs]
    before = _firewall_snapshot(powershell_path, rule_names)
    if before["records"] or not firewall_platform_is_effective(before):
        raise scorer.ScoringError("防火墙服务/配置未生效或同名规则已存在，拒绝运行")
    commands = ["$ErrorActionPreference='Stop'"]
    for item in rule_specs:
        escaped_name = item["name"].replace("'", "''")
        program_option = ""
        if item["program"] != "Any":
            escaped_program = item["program"].replace("'", "''")
            program_option = f" -Program '{escaped_program}'"
        commands.append(
            f"New-NetFirewallRule -DisplayName '{escaped_name}' "
            f"-Direction {item['direction']} -Action Block -Profile Any "
            f"-Enabled True{program_option} | Out-Null"
        )
    command = "; ".join(commands)
    completed = subprocess.run(
        [str(powershell_path), "-NoProfile", "-NonInteractive", "-Command", command],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=60,
    )
    if completed.returncode != 0:
        firewall_remove(
            {"rule_names": rule_names, "before": before}, powershell_path
        )
        raise scorer.ScoringError("无法建立并验证临时出站阻断规则；拒绝运行")
    installed = _firewall_snapshot(powershell_path, rule_names)
    expected_by_name = {item["name"]: item for item in rule_specs}
    installed_by_name = {
        str(item.get("DisplayName")): item for item in installed["records"]
    }

    def program_matches(record: dict[str, Any], expected: str) -> bool:
        if expected == "Any":
            return (
                record.get("ProgramScope") == "Any"
                and record.get("ProgramPathSha256") is None
            )
        return (
            record.get("ProgramScope") == "ExactPath"
            and record.get("ProgramPathSha256")
            == canonical_path_sha256(Path(expected))
        )

    valid = (
        len(installed["records"]) == len(rule_specs)
        and set(installed_by_name) == set(expected_by_name)
        and all(str(item.get("Enabled")) == "True" for item in installed["records"])
        and all(str(item.get("Action")) == "Block" for item in installed["records"])
        and all(str(item.get("Profile")) == "Any" for item in installed["records"])
        and all(
            str(installed_by_name[name].get("Direction")) == expected["direction"]
            and program_matches(
                installed_by_name[name], expected["program"]
            )
            for name, expected in expected_by_name.items()
        )
        and firewall_platform_is_effective(installed)
    )
    if not valid:
        firewall_remove(
            {"rule_names": rule_names, "before": before, "installed": installed},
            powershell_path,
        )
        raise scorer.ScoringError("临时按程序双向阻断规则安装后复核失败")
    connection_probe = outbound_block_probe(locked_python_path)
    if connection_probe["blocked_by_firewall_policy"] is not True:
        firewall_remove(
            {"rule_names": rule_names, "before": before, "installed": installed},
            powershell_path,
        )
        raise scorer.ScoringError("临时防火墙规则未产生可验证的出站拒绝错误")
    return {
        "scope": scope,
        "protected_program_count": len(program_paths),
        "protected_program_path_sha256": [
            canonical_path_sha256(program) for program in program_paths
        ],
        "rule_names": rule_names,
        "before": before,
        "installed": installed,
        "outbound_block_probe": connection_probe,
    }


def firewall_remove(
    firewall_evidence: dict[str, Any], powershell_path: Path
) -> dict[str, Any]:
    rule_names = [str(item) for item in firewall_evidence.get("rule_names", [])]
    if not rule_names:
        return {"removed": False, "after": {"rule_names": [], "records": []}}
    quoted_names = ",".join("'" + name.replace("'", "''") + "'" for name in rule_names)
    command = (
        f"$ErrorActionPreference='Stop'; "
        f"$names=@({quoted_names}); foreach($name in $names){{"
        "$rules=@(Get-NetFirewallRule -DisplayName $name "
        "-ErrorAction SilentlyContinue); if($rules.Count -gt 0){"
        "$rules | Remove-NetFirewallRule -ErrorAction Stop}}"
    )
    attempts: list[dict[str, Any]] = []
    after: dict[str, Any] = {
        "rule_names": sorted(rule_names),
        "records": ["REMOVAL_NOT_ATTEMPTED"],
    }
    deadline = time.monotonic() + 20.0
    while True:
        completed = subprocess.run(
            [
                str(powershell_path),
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command,
            ],
            check=False,
            capture_output=True,
            timeout=60,
        )
        snapshot_error_type: str | None = None
        try:
            after = _firewall_snapshot(powershell_path, rule_names)
        except Exception as exc:
            snapshot_error_type = type(exc).__name__
            after = {
                "rule_names": sorted(rule_names),
                "records": ["QUERY_FAILED"],
            }
        platform_effective = (
            firewall_platform_is_effective(after)
            if snapshot_error_type is None
            else False
        )
        attempts.append(
            {
                "remove_exit_code": completed.returncode,
                "stdout_bytes": len(completed.stdout),
                "stderr_bytes": len(completed.stderr),
                "stdout_prefix_hex": completed.stdout[:512].hex(),
                "stderr_prefix_hex": completed.stderr[:512].hex(),
                "stdout_sha256": hashlib.sha256(completed.stdout).hexdigest(),
                "stderr_sha256": hashlib.sha256(completed.stderr).hexdigest(),
                "remaining_rule_count": len(after["records"]),
                "snapshot_error_type": snapshot_error_type,
                "firewall_platform_effective": platform_effective,
            }
        )
        if (
            completed.returncode == 0
            and not after["records"]
            and platform_effective
        ):
            return {
                "removed": True,
                "remove_exit_code": completed.returncode,
                "attempt_count": len(attempts),
                "attempts": attempts,
                "after": after,
            }
        if time.monotonic() >= deadline:
            return {
                "removed": False,
                "remove_exit_code": completed.returncode,
                "attempt_count": len(attempts),
                "attempts": attempts,
                "after": after,
            }
        time.sleep(0.5)


def terminate_tree(pid: int) -> list[int]:
    terminated: list[int] = []
    try:
        parent = psutil.Process(pid)
        children = parent.children(recursive=True)
        for process in children:
            try:
                process.terminate()
                terminated.append(process.pid)
            except psutil.Error:
                pass
        _, alive = psutil.wait_procs(children, timeout=5)
        for process in alive:
            try:
                process.kill()
            except psutil.Error:
                pass
        try:
            parent.terminate()
            parent.wait(timeout=5)
        except psutil.TimeoutExpired:
            parent.kill()
    except psutil.Error:
        pass
    return terminated


def process_connections(pid: int) -> list[str]:
    result: set[str] = set()
    try:
        processes = [psutil.Process(pid)] + psutil.Process(pid).children(recursive=True)
    except psutil.Error:
        return []
    for process in processes:
        try:
            for connection in process.net_connections(kind="inet"):
                if connection.raddr:
                    result.add(f"{connection.raddr.ip}:{connection.raddr.port}")
        except (psutil.AccessDenied, psutil.NoSuchProcess):
            continue
    return sorted(result)


def observe_process_tree(pid: int) -> dict[str, Any]:
    """Observe descendants, RSS, remote sockets and listeners from the parent."""

    try:
        root = psutil.Process(pid)
        processes = [root] + root.children(recursive=True)
    except psutil.Error:
        processes = []
    identities: dict[int, float] = {}
    remote: set[str] = set()
    listeners: set[str] = set()
    total_rss = 0
    errors: list[str] = []
    for process in processes:
        try:
            identities[process.pid] = float(process.create_time())
            total_rss += int(process.memory_info().rss)
            for connection in process.net_connections(kind="inet"):
                local = (
                    f"{connection.laddr.ip}:{connection.laddr.port}"
                    if connection.laddr
                    else ""
                )
                if connection.status == psutil.CONN_LISTEN:
                    listeners.add(f"pid={process.pid}:{local}")
                if connection.raddr:
                    remote.add(
                        f"pid={process.pid}:{connection.raddr.ip}:{connection.raddr.port}"
                    )
        except (psutil.AccessDenied, psutil.NoSuchProcess, OSError) as exc:
            errors.append(type(exc).__name__)
    return {
        "process_identities": identities,
        "process_tree_rss_bytes": total_rss,
        "system_available_bytes": int(psutil.virtual_memory().available),
        "remote_connections": sorted(remote),
        "listening_sockets": sorted(listeners),
        "errors": errors,
    }


def nvidia_memory_sample(nvidia_smi_path: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [
            str(nvidia_smi_path),
            "--query-gpu=uuid,memory.used,memory.free",
            "--format=csv,noheader,nounits",
        ],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
    )
    rows = [line.strip() for line in completed.stdout.splitlines() if line.strip()]
    fields = [item.strip() for item in rows[0].split(",", 2)] if len(rows) == 1 else []
    if completed.returncode != 0 or len(fields) != 3:
        raise scorer.ScoringError("父进程无法取得唯一 GPU 显存样本")
    return {
        "gpu_uuid": fields[0],
        "memory_used_bytes": int(fields[1]) * 1024 * 1024,
        "memory_free_bytes": int(fields[2]) * 1024 * 1024,
    }


class WindowsKillOnCloseJob:
    """Put the untrusted worker tree in a Windows kill-on-close Job Object."""

    def __init__(self) -> None:
        if sys.platform != "win32":
            raise scorer.ScoringError("Windows Job Object 只能在正式 Windows 主机使用")
        from ctypes import wintypes

        class IO_COUNTERS(ctypes.Structure):
            _fields_ = [
                ("ReadOperationCount", ctypes.c_ulonglong),
                ("WriteOperationCount", ctypes.c_ulonglong),
                ("OtherOperationCount", ctypes.c_ulonglong),
                ("ReadTransferCount", ctypes.c_ulonglong),
                ("WriteTransferCount", ctypes.c_ulonglong),
                ("OtherTransferCount", ctypes.c_ulonglong),
            ]

        class BASIC_LIMIT_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("PerProcessUserTimeLimit", ctypes.c_longlong),
                ("PerJobUserTimeLimit", ctypes.c_longlong),
                ("LimitFlags", wintypes.DWORD),
                ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t),
                ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.c_size_t),
                ("PriorityClass", wintypes.DWORD),
                ("SchedulingClass", wintypes.DWORD),
            ]

        class EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("BasicLimitInformation", BASIC_LIMIT_INFORMATION),
                ("IoInfo", IO_COUNTERS),
                ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t),
            ]

        class BASIC_ACCOUNTING_INFORMATION(ctypes.Structure):
            _fields_ = [
                ("TotalUserTime", ctypes.c_longlong),
                ("TotalKernelTime", ctypes.c_longlong),
                ("ThisPeriodTotalUserTime", ctypes.c_longlong),
                ("ThisPeriodTotalKernelTime", ctypes.c_longlong),
                ("TotalPageFaultCount", wintypes.DWORD),
                ("TotalProcesses", wintypes.DWORD),
                ("ActiveProcesses", wintypes.DWORD),
                ("TotalTerminatedProcesses", wintypes.DWORD),
            ]

        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        kernel32.SetInformationJobObject.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            wintypes.DWORD,
        ]
        kernel32.SetInformationJobObject.restype = wintypes.BOOL
        kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
        kernel32.QueryInformationJobObject.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            wintypes.DWORD,
            ctypes.POINTER(wintypes.DWORD),
        ]
        kernel32.QueryInformationJobObject.restype = wintypes.BOOL
        kernel32.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
        kernel32.TerminateJobObject.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        self._kernel32 = kernel32
        self._handle = kernel32.CreateJobObjectW(None, None)
        if not self._handle:
            raise ctypes.WinError(ctypes.get_last_error())
        information = EXTENDED_LIMIT_INFORMATION()
        information.BasicLimitInformation.LimitFlags = 0x00002000
        ok = kernel32.SetInformationJobObject(
            self._handle, 9, ctypes.byref(information), ctypes.sizeof(information)
        )
        if not ok:
            kernel32.CloseHandle(self._handle)
            raise ctypes.WinError(ctypes.get_last_error())
        self._basic_accounting_type = BASIC_ACCOUNTING_INFORMATION
        self._dword_type = wintypes.DWORD
        self.assigned = False
        self.kill_on_close_configured = True

    def assign(self, process: subprocess.Popen[bytes]) -> None:
        if not self._kernel32.AssignProcessToJobObject(
            self._handle, process._handle
        ):
            raise ctypes.WinError(ctypes.get_last_error())
        self.assigned = True

    def active_processes(self) -> int:
        if not self._handle:
            return 0
        information = self._basic_accounting_type()
        returned_length = self._dword_type()
        ok = self._kernel32.QueryInformationJobObject(
            self._handle,
            1,
            ctypes.byref(information),
            ctypes.sizeof(information),
            ctypes.byref(returned_length),
        )
        if not ok:
            raise ctypes.WinError(ctypes.get_last_error())
        return int(information.ActiveProcesses)

    def close(self) -> dict[str, Any]:
        evidence: dict[str, Any] = {
            "close_handle_attempted": False,
            "close_handle_succeeded": False,
            "active_processes_before_close": None,
            "termination_fallback_used": False,
            "termination_fallback_succeeded": None,
        }
        if not self._handle:
            return evidence
        try:
            evidence["active_processes_before_close"] = self.active_processes()
        except BaseException as query_exc:
            evidence["active_process_query_error"] = type(query_exc).__name__
        evidence["close_handle_attempted"] = True
        close_ok = bool(self._kernel32.CloseHandle(self._handle))
        if close_ok:
            evidence["close_handle_succeeded"] = True
            self._handle = None
            return evidence
        evidence["close_error_code"] = int(ctypes.get_last_error())
        evidence["termination_fallback_used"] = True
        terminate_ok = bool(self._kernel32.TerminateJobObject(self._handle, 2))
        evidence["termination_fallback_succeeded"] = terminate_ok
        second_close_ok = bool(self._kernel32.CloseHandle(self._handle))
        evidence["second_close_handle_succeeded"] = second_close_ok
        evidence["close_handle_succeeded"] = second_close_ok
        if second_close_ok:
            self._handle = None
        return evidence


def resume_suspended_process(process: subprocess.Popen[bytes]) -> None:
    """Resume the only thread after the suspended process is inside the Job."""

    if sys.platform != "win32":
        raise scorer.ScoringError("只能在 Windows 恢复冻结启动的 worker")
    from ctypes import wintypes

    threads = psutil.Process(process.pid).threads()
    if len(threads) != 1:
        raise scorer.ScoringError("冻结启动阶段未找到唯一主线程")
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenThread.restype = wintypes.HANDLE
    kernel32.ResumeThread.argtypes = [wintypes.HANDLE]
    kernel32.ResumeThread.restype = wintypes.DWORD
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel32.CloseHandle.restype = wintypes.BOOL
    thread_handle = kernel32.OpenThread(0x0002, False, int(threads[0].id))
    if not thread_handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        previous_suspend_count = kernel32.ResumeThread(thread_handle)
        if previous_suspend_count == 0xFFFFFFFF or previous_suspend_count != 1:
            raise scorer.ScoringError("worker 主线程恢复失败或冻结计数异常")
    finally:
        kernel32.CloseHandle(thread_handle)


class ParentOwnedPipeCapture:
    """Drain one child pipe into parent-owned memory.

    A child process can seek/truncate a redirected file handle.  It cannot
    rewrite bytes that the parent has already read from an anonymous pipe.
    The hard size limit fails closed instead of silently discarding a tail
    that could contain protected text.
    """

    def __init__(
        self,
        stream: Any,
        label: str,
        max_bytes: int = MAX_CAPTURE_BYTES_PER_STREAM,
    ) -> None:
        self.stream = stream
        self.label = label
        self.max_bytes = max_bytes
        self.data = bytearray()
        self.total_bytes = 0
        self.digest = hashlib.sha256()
        self.overflow = False
        self.error_type: str | None = None
        self.thread = threading.Thread(
            target=self._drain,
            name=f"meetily-r3-{label}-capture",
            daemon=True,
        )

    def _drain(self) -> None:
        try:
            while True:
                chunk = self.stream.read(64 * 1024)
                if not chunk:
                    break
                self.total_bytes += len(chunk)
                self.digest.update(chunk)
                remaining = self.max_bytes - len(self.data)
                if remaining > 0:
                    self.data.extend(chunk[:remaining])
                if len(chunk) > remaining:
                    self.overflow = True
        except BaseException as exc:  # recorded and rejected by the parent gate
            self.error_type = type(exc).__name__
        finally:
            try:
                self.stream.close()
            except BaseException as exc:
                self.error_type = self.error_type or type(exc).__name__

    def start(self) -> None:
        self.thread.start()

    def finish(self, timeout_seconds: float = 30.0) -> None:
        self.thread.join(timeout_seconds)
        if self.thread.is_alive():
            self.error_type = self.error_type or "PipeReaderJoinTimeout"

    @property
    def complete(self) -> bool:
        return not self.overflow and self.error_type is None and not self.thread.is_alive()

    def captured_bytes(self) -> bytes:
        return bytes(self.data)


class IndependentResourceSampler:
    """Sample RAM/process RSS and GPU memory on parent-owned threads."""

    def __init__(self, pid: int, nvidia_smi_path: Path, expected_gpu_uuid: str) -> None:
        self.pid = pid
        self.nvidia_smi_path = nvidia_smi_path
        self.expected_gpu_uuid = expected_gpu_uuid
        self.started_monotonic = time.monotonic()
        self.finished_monotonic: float | None = None
        self.system_samples: list[dict[str, Any]] = []
        self.gpu_samples: list[dict[str, Any]] = []
        self.errors: list[str] = []
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._threads = [
            threading.Thread(
                target=self._system_loop,
                name="meetily-r3-system-resource-sampler",
                daemon=True,
            ),
            threading.Thread(
                target=self._gpu_loop,
                name="meetily-r3-gpu-resource-sampler",
                daemon=True,
            ),
        ]

    def _system_loop(self) -> None:
        while not self._stop.is_set():
            sampled_at = time.monotonic()
            try:
                root = psutil.Process(self.pid)
                processes = [root] + root.children(recursive=True)
                rss = 0
                for process in processes:
                    try:
                        rss += int(process.memory_info().rss)
                    except psutil.Error:
                        continue
                sample = {
                    "sampled_at_monotonic_seconds": sampled_at,
                    "process_tree_rss_bytes": rss,
                    "system_available_bytes": int(psutil.virtual_memory().available),
                    "observed_process_count": len(processes),
                }
                with self._lock:
                    self.system_samples.append(sample)
            except psutil.NoSuchProcess:
                if self._stop.is_set():
                    break
            except BaseException as exc:
                with self._lock:
                    self.errors.append(f"system:{type(exc).__name__}")
            self._stop.wait(0.25)

    def _gpu_loop(self) -> None:
        while not self._stop.is_set():
            sampled_at = time.monotonic()
            try:
                value = nvidia_memory_sample(self.nvidia_smi_path)
                if value["gpu_uuid"] != self.expected_gpu_uuid:
                    raise scorer.ScoringError("独立采样器检测到 GPU UUID 改变")
                with self._lock:
                    self.gpu_samples.append(
                        {
                            "sampled_at_monotonic_seconds": sampled_at,
                            "gpu_uuid": value["gpu_uuid"],
                            "gpu_memory_used_bytes": int(value["memory_used_bytes"]),
                            "gpu_memory_free_bytes": int(value["memory_free_bytes"]),
                        }
                    )
            except BaseException as exc:
                with self._lock:
                    self.errors.append(f"gpu:{type(exc).__name__}")
            self._stop.wait(0.5)

    def start(self) -> None:
        for thread in self._threads:
            thread.start()

    def wait_for_initial_samples(self, timeout_seconds: float = 35.0) -> None:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            with self._lock:
                if self.system_samples and self.gpu_samples:
                    return
                errors = list(self.errors)
            if errors:
                raise scorer.ScoringError("独立资源采样器启动失败")
            time.sleep(0.05)
        raise scorer.ScoringError("独立资源采样器没有按时取得首个 RAM/GPU 样本")

    def finish(self, timeout_seconds: float = 35.0) -> None:
        self._stop.set()
        for thread in self._threads:
            thread.join(timeout_seconds)
            if thread.is_alive():
                with self._lock:
                    self.errors.append(f"join:{thread.name}")
        self.finished_monotonic = time.monotonic()

    @staticmethod
    def maximum_interval(samples: list[dict[str, Any]]) -> float | None:
        stamps = sorted(float(item["sampled_at_monotonic_seconds"]) for item in samples)
        if len(stamps) < 2:
            return None
        return max(second - first for first, second in zip(stamps, stamps[1:]))


def scan_captured_logs(
    run_id: str,
    stdout_path: Path,
    stderr_path: Path,
    moss_output: Path,
    hotwords_path: Path,
    sensitive_paths: list[Path],
    protected_text_paths: list[Path] | None = None,
) -> dict[str, Any]:
    return scan_captured_log_bytes(
        run_id,
        stdout_path.read_bytes(),
        stderr_path.read_bytes(),
        moss_output,
        hotwords_path,
        sensitive_paths,
        protected_text_paths,
    )


def scan_captured_log_bytes(
    run_id: str,
    stdout_bytes: bytes,
    stderr_bytes: bytes,
    moss_output: Path,
    hotwords_path: Path,
    sensitive_paths: list[Path],
    protected_text_paths: list[Path] | None = None,
    *,
    capture_complete: bool = True,
    capture_errors: list[str] | None = None,
) -> dict[str, Any]:
    combined = (stdout_bytes + b"\n" + stderr_bytes).decode("utf-8", errors="replace")
    normalized_log = scorer.normalize_text(combined)
    probes: list[tuple[str, str, str]] = []
    transcript_ngrams: set[str] = set()
    exact_sensitive_terms: set[str] = set()
    protected_source_hashes: dict[str, str] = {}
    transcript_ngram_characters = 8

    def add_transcript_ngrams(value: str) -> None:
        normalized = scorer.normalize_text(value)
        if len(normalized) < transcript_ngram_characters:
            return
        for index in range(len(normalized) - transcript_ngram_characters + 1):
            transcript_ngrams.add(
                normalized[index : index + transcript_ngram_characters]
            )

    processed_text_paths: set[Path] = set()

    def add_protected_text_path(protected_path: Path) -> None:
        resolved = protected_path.resolve(strict=True)
        if resolved in processed_text_paths:
            return
        processed_text_paths.add(resolved)
        protected_source_hashes[str(resolved)] = scorer.sha256_file(resolved)
        suffix = resolved.suffix.casefold()
        if suffix == ".tsv":
            with resolved.open("r", encoding="utf-8-sig", newline="") as stream:
                reader = csv.reader(stream, delimiter="\t", strict=True)
                for row in reader:
                    for cell in row:
                        add_transcript_ngrams(cell)
            return
        if suffix in {".txt", ".md", ".prompt"}:
            add_transcript_ngrams(resolved.read_text(encoding="utf-8-sig"))
            return
        if suffix != ".json":
            return
        protected_payload = scorer.load_json(resolved)

        def walk_strings(value: Any, key: str = "") -> None:
            if isinstance(value, str):
                add_transcript_ngrams(value)
                normalized_value = scorer.normalize_text(value)
                if key in {"term", "name", "alias", "aliases"} and len(normalized_value) >= 2:
                    exact_sensitive_terms.add(normalized_value)
                return
            if isinstance(value, list):
                for item in value:
                    walk_strings(item, key)
                return
            if isinstance(value, dict):
                for nested_key, nested_value in value.items():
                    walk_strings(nested_value, str(nested_key))

        walk_strings(protected_payload)

    if moss_output.is_file():
        payload = scorer.load_json(moss_output)
        raw = str(payload.get("raw_model_text", ""))
        add_transcript_ngrams(raw)
        for segment in payload.get("segments", []):
            add_transcript_ngrams(str(segment.get("text", "")))
        protected_source_hashes[str(moss_output.resolve(strict=True))] = scorer.sha256_file(
            moss_output
        )
    for protected_path in protected_text_paths or []:
        add_protected_text_path(protected_path)
    hotwords = scorer.load_json(hotwords_path)
    for item in hotwords.get("positive_spoken_terms", []) + hotwords.get(
        "negative_unspoken_terms", []
    ):
        value = str(item.get("term", ""))
        probes.append(("hotword", scorer.normalize_text(value), value))
        normalized_value = scorer.normalize_text(value)
        if len(normalized_value) >= 2:
            exact_sensitive_terms.add(normalized_value)
    for path in sensitive_paths:
        value = str(path.resolve(strict=False))
        probes.append(("absolute_path", scorer.normalize_text(value), value))
        if path.is_file() and path.suffix.casefold() in {
            ".json",
            ".tsv",
            ".txt",
            ".md",
            ".prompt",
        }:
            add_protected_text_path(path)
    for ngram in sorted(transcript_ngrams):
        probes.append(("protected_text_ngram", ngram, ngram))
    for value in sorted(exact_sensitive_terms):
        probes.append(("protected_short_term", value, value))

    hits: list[dict[str, Any]] = []
    for category, normalized, digest_source in probes:
        if len(normalized) < 2:
            continue
        count = normalized_log.count(normalized)
        if count:
            hits.append(
                {
                    "category": category,
                    "probe_sha256": hashlib.sha256(
                        digest_source.encode("utf-8")
                    ).hexdigest(),
                    "count": count,
                }
            )
    return {
        "schema_version": 1,
        "role": "SUPERVISED_STREAM_LOG_AUDIT",
        "run_id": run_id,
        "captured_at": now_iso(),
        "capture_method": "PARENT_OWNED_ANONYMOUS_PIPE_MEMORY_CAPTURE",
        "capture_complete": capture_complete,
        "capture_errors": sorted(capture_errors or []),
        "raw_logs_written_to_disk": False,
        "stdout_bytes": len(stdout_bytes),
        "stdout_sha256": hashlib.sha256(stdout_bytes).hexdigest(),
        "stderr_bytes": len(stderr_bytes),
        "stderr_sha256": hashlib.sha256(stderr_bytes).hexdigest(),
        "probe_count": len(probes),
        "protected_text_source_hashes": protected_source_hashes,
        "transcript_ngram_characters": transcript_ngram_characters,
        "sensitive_hit_count": sum(item["count"] for item in hits),
        "hit_records_without_plaintext": hits,
        "transcript_printed_to_console": any(
            item["category"] in {"protected_text_ngram", "protected_short_term"}
            for item in hits
        ),
        "raw_logs_deleted_after_hash_and_scan": True,
    }


def worker_command(
    args: argparse.Namespace,
    copied: dict[str, Path],
    model_root: Path,
    source_code: Path,
    run_dir: Path,
) -> list[str]:
    runtime_lock = scorer.load_json(copied["runtime_lock"])
    locked_python = scorer.locked_external_tool(runtime_lock, "python")
    command = [
        str(locked_python),
        "-I",
        "-S",
        "-B",
        str(copied["runtime_bootstrap"]),
        "--runtime-lock",
        str(copied["runtime_lock"]),
        "--runner",
        str(copied["runner"]),
        "--",
        "run",
        "--window-audio",
        str(copied["window_audio"]),
        "--full-audio",
        str(copied["full_audio"]),
        "--source-lock",
        str(copied["source_lock"]),
        "--verbatim",
        str(copied["verbatim"]),
        "--turns",
        str(copied["speaker_turns"]),
        "--review",
        str(copied["human_review"]),
        "--hotwords",
        str(copied["hotwords"]),
        "--pre-meeting-context",
        str(copied["pre_meeting_context"]),
        "--rules",
        str(copied["rules"]),
        "--whisper-full",
        str(copied["whisper_full"]),
        "--whisper-run-record",
        str(copied["whisper_run_record"]),
        "--whisper-model",
        str(copied["whisper_model"]),
        "--stable-exe",
        str(copied["stable_exe"]),
        "--prompt",
        str(copied["prompt"]),
        "--model-root",
        str(model_root),
        "--model-manifest",
        str(copied["model_manifest"]),
        "--runtime-lock",
        str(copied["runtime_lock"]),
        "--runtime-bootstrap",
        str(copied["runtime_bootstrap"]),
        "--source-code",
        str(source_code),
        "--scorer",
        str(copied["scorer"]),
        "--supervisor",
        str(copied["supervisor"]),
        "--pip-freeze",
        str(copied["pip_freeze"]),
        "--pre-run-bundle",
        str(copied["pre_run_bundle"]),
        "--pre-run-signature",
        str(copied["pre_run_signature"]),
        "--attestation-public-key",
        str(copied["attestation_public_key"]),
        "--run-id",
        args.run_id,
        "--moss-full-out",
        str(run_dir / "20-moss-full.json"),
        "--run-out",
        str(run_dir / "21-worker-run.json"),
        "--heartbeat-out",
        str(run_dir / "10-worker-heartbeat.json"),
    ]
    return command


def _execute_impl(args: argparse.Namespace) -> int:
    if not re.fullmatch(r"[0-9a-f]{32}", args.run_id):
        raise scorer.ScoringError("run_id 必须是 32 位小写十六进制")
    if args.run_dir.exists():
        raise scorer.ScoringError("run-dir 必须是全新目录")
    for field, maximum in (
        ("model_load_timeout_seconds", 600.0),
        ("inference_timeout_seconds", 1200.0),
        ("total_timeout_seconds", 1800.0),
    ):
        value = float(getattr(args, field))
        if not math.isfinite(value) or value <= 0 or value > maximum:
            raise scorer.ScoringError(f"{field} 必须为有限正数且不得超过 {maximum}")
    static_hashes, source_head = validate_static_inputs(args)
    pre_bundle = scorer.load_json(args.pre_run_bundle)
    rules = scorer.load_json(args.rules)
    runtime_lock = scorer.load_json(args.runtime_lock)
    ssh_keygen_path = scorer.locked_external_tool(runtime_lock, "ssh_keygen")
    scorer.require_signed_bundle_schema(
        pre_bundle, "SIGNED_PRE_RUN_INPUT_BUNDLE", args.run_id
    )
    verification = scorer.verify_ssh_signature(
        args.pre_run_bundle,
        args.pre_run_signature,
        args.attestation_public_key,
        str(rules["attestation"]["signer_identity"]),
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    if verification.get("valid") is not True or pre_bundle.get(
        "artifact_hashes"
    ) != static_hashes:
        raise scorer.ScoringError("预运行签名或输入哈希不匹配")
    ledger_dir = require_external_run_ledger(
        args.run_ledger_dir, args.workspace_root
    )
    if (
        pre_bundle.get("expected_run_dir_sha256")
        != canonical_path_sha256(args.run_dir)
        or pre_bundle.get("run_ledger_dir_sha256")
        != canonical_path_sha256(ledger_dir)
        or pre_bundle.get("single_execution_policy")
        != "RUN_ID_AND_RUN_DIR_BOUND_ONE_ATTEMPT_ONLY"
    ):
        raise scorer.ScoringError("预运行签名未绑定本次 run-dir/run ledger")
    consumption_marker_path = ledger_dir / f"{args.run_id}.consumed.json"
    consumption_marker = {
        "schema_version": 1,
        "role": "ONE_TIME_RUN_ID_CONSUMPTION",
        "run_id": args.run_id,
        "consumed_at": now_iso(),
        "pre_run_bundle_sha256": scorer.sha256_file(args.pre_run_bundle),
        "run_dir_sha256": canonical_path_sha256(args.run_dir),
        "policy": "NO_RETRY_WITH_SAME_RUN_ID_OR_SIGNED_PREBUNDLE",
    }
    write_new_json(consumption_marker_path, consumption_marker)
    consumption_marker_sha256 = scorer.sha256_file(consumption_marker_path)

    args.run_dir.mkdir(parents=True, exist_ok=False)
    status_path = args.run_dir / "00-supervisor-status.json"
    status = {
        "schema_version": 1,
        "role": "CUDA_SUPERVISOR_STATUS",
        "run_id": args.run_id,
        "status": "PREPARING_SNAPSHOT",
        "started_at": now_iso(),
        "updated_at": now_iso(),
        "supervisor_pid": os.getpid(),
    }
    write_new_json(status_path, status)
    copied, model_snapshot, source_snapshot, snapshot_evidence = snapshot_inputs(
        args, args.run_dir
    )
    # The signed bundle itself and signature are copied only after the static map.
    for name, source in {
        "pre_run_bundle": args.pre_run_bundle,
        "pre_run_signature": args.pre_run_signature,
    }.items():
        destination = args.run_dir / "input-snapshot" / f"{name}{''.join(source.suffixes)}"
        shutil.copy2(source, destination)
        destination.chmod(stat.S_IREAD)
        copied[name] = destination
    input_snapshot_root = args.run_dir / "input-snapshot"
    snapshot_evidence["input_snapshot_pre"] = exact_directory_manifest(
        input_snapshot_root
    )

    status.update({"status": "INSTALLING_NETWORK_BLOCK", "updated_at": now_iso()})
    update_json(status_path, status)
    powershell_path = scorer.locked_external_tool(runtime_lock, "powershell")
    stdout_capture: ParentOwnedPipeCapture | None = None
    stderr_capture: ParentOwnedPipeCapture | None = None
    firewall_removal = {"removed": False, "after": {"records": ["NOT_REMOVED"]}}
    worker_started: str | None = None
    worker_finished: str | None = None
    parent_start_monotonic: float | None = None
    parent_elapsed_seconds: float | None = None
    timeout_reason = None
    observed_connections: set[str] = set()
    observed_listeners: set[str] = set()
    observed_process_identities: dict[int, float] = {}
    firewall_runtime_checks: list[dict[str, Any]] = []
    parent_monitor_errors: list[str] = []
    parent_system_available_before = int(psutil.virtual_memory().available)
    terminated_ids: list[int] = []
    command = worker_command(args, copied, model_snapshot, source_snapshot, args.run_dir)
    environment = os.environ.copy()
    for unsafe_name in ("PYTHONPATH", "PYTHONHOME", "PYTHONUSERBASE"):
        environment.pop(unsafe_name, None)
    environment.update(
        {
            "HF_HUB_OFFLINE": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "HF_DATASETS_OFFLINE": "1",
            "NO_PROXY": "*",
            "GIT_OPTIONAL_LOCKS": "0",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONNOUSERSITE": "1",
        }
    )
    process: subprocess.Popen[bytes] | None = None
    job: WindowsKillOnCloseJob | None = None
    resource_sampler: IndependentResourceSampler | None = None
    worker_resume_monotonic: float | None = None
    worker_exit_monotonic: float | None = None
    job_close_evidence: dict[str, Any] = {
        "close_handle_attempted": False,
        "close_handle_succeeded": False,
        "active_processes_before_close": None,
        "termination_fallback_used": False,
    }
    process_cleanup_evidence: dict[str, Any] = {
        "process_existed": False,
        "process_exited_after_job_close": None,
        "explicit_tree_termination_used": False,
    }
    firewall_evidence = firewall_add(
        args.run_id,
        powershell_path,
        scorer.locked_external_tool(runtime_lock, "python"),
    )
    try:
        if parent_system_available_before < 7 * 1024**3:
            raise scorer.ScoringError("父监督器实测启动前可用系统内存低于 7 GiB")
        pre_gpu_memory = nvidia_memory_sample(
            scorer.locked_external_tool(runtime_lock, "nvidia_smi")
        )
        if pre_gpu_memory["gpu_uuid"] != runtime_lock["environment"].get("gpu_uuid"):
            raise scorer.ScoringError("启动前显存样本来自另一张 GPU")
        status.update({"status": "WORKER_RUNNING", "updated_at": now_iso()})
        update_json(status_path, status)
        job = WindowsKillOnCloseJob()
        worker_started = now_iso()
        parent_start_monotonic = time.monotonic()
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            creationflags=(
                (subprocess.CREATE_NO_WINDOW | CREATE_SUSPENDED)
                if sys.platform == "win32"
                else 0
            ),
        )
        # The child is born suspended.  Assign it to the kill-on-close Job
        # before touching any fallible pipe/capture setup so no suspended
        # process can escape if setup fails.
        job.assign(process)
        if process.stdout is None or process.stderr is None:
            raise scorer.ScoringError("父进程无法建立匿名日志管道")
        stdout_capture = ParentOwnedPipeCapture(process.stdout, "stdout")
        stderr_capture = ParentOwnedPipeCapture(process.stderr, "stderr")
        stdout_capture.start()
        stderr_capture.start()
        resource_sampler = IndependentResourceSampler(
            process.pid,
            scorer.locked_external_tool(runtime_lock, "nvidia_smi"),
            str(runtime_lock["environment"].get("gpu_uuid")),
        )
        resource_sampler.start()
        resource_sampler.wait_for_initial_samples()
        try:
            worker_resume_monotonic = time.monotonic()
            resume_suspended_process(process)
        except Exception:
            terminate_tree(process.pid)
            raise
        start_monotonic = parent_start_monotonic
        stage_started = start_monotonic
        next_firewall_check = start_monotonic
        last_stage = None
        last_stage_rank = -1
        stage_ranks = {
            None: 0,
            "PRECHECK_VALIDATING": 1,
            "MODEL_LOADING": 2,
            "INFERENCE_STARTED": 3,
            "OUTPUT_PARSING": 4,
            "CLEANUP": 5,
        }
        while process.poll() is None:
            now = time.monotonic()
            if now >= next_firewall_check:
                try:
                    firewall_snapshot = _firewall_snapshot(
                        powershell_path, firewall_evidence["rule_names"]
                    )
                    directions = {
                        str(item.get("Direction"))
                        for item in firewall_snapshot.get("records", [])
                    }
                    firewall_valid = (
                        len(firewall_snapshot.get("records", [])) == 2
                        and directions == {"Inbound", "Outbound"}
                        and all(
                            str(item.get("Enabled")) == "True"
                            and str(item.get("Action")) == "Block"
                            and str(item.get("Profile")) == "Any"
                            for item in firewall_snapshot.get("records", [])
                        )
                        and firewall_platform_is_effective(firewall_snapshot)
                    )
                    firewall_runtime_checks.append(
                        {
                            "checked_at": now_iso(),
                            "valid": firewall_valid,
                            "snapshot": firewall_snapshot,
                        }
                    )
                    if not firewall_valid:
                        timeout_reason = "NETWORK_BLOCK_TAMPERED_OR_DISABLED"
                except BaseException as firewall_check_exc:
                    firewall_runtime_checks.append(
                        {
                            "checked_at": now_iso(),
                            "valid": False,
                            "error_type": type(firewall_check_exc).__name__,
                        }
                    )
                    timeout_reason = "NETWORK_BLOCK_CHECK_FAILED"
                next_firewall_check = now + 5.0
            observation = observe_process_tree(process.pid)
            observed_process_identities.update(observation["process_identities"])
            observed_connections.update(observation["remote_connections"])
            observed_listeners.update(observation["listening_sockets"])
            parent_monitor_errors.extend(observation["errors"])
            heartbeat_path = args.run_dir / "10-worker-heartbeat.json"
            stage = None
            if heartbeat_path.is_file():
                try:
                    stage = scorer.load_json(heartbeat_path).get("stage")
                except scorer.ScoringError:
                    stage = "INVALID_HEARTBEAT"
            if stage not in stage_ranks or stage_ranks[stage] < last_stage_rank:
                timeout_reason = "INVALID_OR_REGRESSING_HEARTBEAT_STAGE"
            elif stage != last_stage:
                last_stage = stage
                last_stage_rank = stage_ranks[stage]
                stage_started = now
            if timeout_reason:
                pass
            elif now - start_monotonic > args.total_timeout_seconds:
                timeout_reason = "TOTAL_TIMEOUT"
            elif stage == "MODEL_LOADING" and now - stage_started > args.model_load_timeout_seconds:
                timeout_reason = "MODEL_LOAD_TIMEOUT"
            elif stage == "INFERENCE_STARTED" and now - stage_started > args.inference_timeout_seconds:
                timeout_reason = "INFERENCE_TIMEOUT"
            if timeout_reason:
                terminated_ids = terminate_tree(process.pid)
                break
            status.update(
                {
                    "status": "WORKER_RUNNING",
                    "worker_pid": process.pid,
                    "worker_stage": stage,
                    "updated_at": now_iso(),
                }
            )
            update_json(status_path, status)
            time.sleep(1.0)
        try:
            return_code = process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            terminated_ids.extend(terminate_tree(process.pid))
            return_code = process.wait(timeout=10)
        worker_finished = now_iso()
        worker_exit_monotonic = time.monotonic()
        parent_elapsed_seconds = time.monotonic() - parent_start_monotonic
    finally:
        if job is not None:
            try:
                job_close_evidence = job.close()
            except BaseException as close_exc:
                job_close_evidence = {
                    "close_handle_attempted": True,
                    "close_handle_succeeded": False,
                    "active_processes_before_close": None,
                    "termination_fallback_used": False,
                    "error_type": type(close_exc).__name__,
                }
        if process is not None:
            process_cleanup_evidence["process_existed"] = True
            try:
                process.wait(timeout=10)
                process_cleanup_evidence["process_exited_after_job_close"] = True
                process_cleanup_evidence["final_return_code"] = process.returncode
            except subprocess.TimeoutExpired:
                process_cleanup_evidence["explicit_tree_termination_used"] = True
                process_cleanup_evidence["explicitly_terminated_process_ids"] = (
                    terminate_tree(process.pid)
                )
                try:
                    process.wait(timeout=10)
                    process_cleanup_evidence["process_exited_after_job_close"] = True
                    process_cleanup_evidence["final_return_code"] = process.returncode
                except subprocess.TimeoutExpired:
                    process_cleanup_evidence["process_exited_after_job_close"] = False
        if resource_sampler is not None:
            resource_sampler.finish()
        if stdout_capture is not None:
            stdout_capture.finish()
        if stderr_capture is not None:
            stderr_capture.finish()
        try:
            firewall_removal = firewall_remove(firewall_evidence, powershell_path)
        except BaseException as firewall_exc:
            firewall_removal = {
                "removed": False,
                "after": {"records": ["REMOVAL_CHECK_FAILED"]},
                "error_type": type(firewall_exc).__name__,
            }

    if stdout_capture is None or stderr_capture is None:
        raise scorer.ScoringError("监督日志管道未建立")

    if worker_started is None or worker_finished is None or parent_elapsed_seconds is None:
        raise scorer.ScoringError("父进程墙钟没有完整记录")
    snapshot_evidence["input_snapshot_post"] = exact_directory_manifest(
        input_snapshot_root
    )
    snapshot_evidence["input_snapshot_unchanged"] = (
        snapshot_evidence["input_snapshot_pre"]
        == snapshot_evidence["input_snapshot_post"]
    )
    worker_record_path = args.run_dir / "21-worker-run.json"
    moss_output_path = args.run_dir / "20-moss-full.json"
    capture_errors = [
        f"stdout:{stdout_capture.error_type}"
        for _ in [0]
        if stdout_capture.error_type is not None
    ] + [
        f"stderr:{stderr_capture.error_type}"
        for _ in [0]
        if stderr_capture.error_type is not None
    ]
    if stdout_capture.overflow:
        capture_errors.append("stdout:CaptureLimitExceeded")
    if stderr_capture.overflow:
        capture_errors.append("stderr:CaptureLimitExceeded")
    log_audit = scan_captured_log_bytes(
        args.run_id,
        stdout_capture.captured_bytes(),
        stderr_capture.captured_bytes(),
        moss_output_path,
        copied["hotwords"],
        [
            copied["window_audio"],
            copied["full_audio"],
            copied["verbatim"],
            copied["speaker_turns"],
            copied["human_review"],
            copied["hotwords"],
            copied["pre_meeting_context"],
            copied["prompt"],
        ],
        [
            copied["verbatim"],
            copied["whisper_full"],
            copied["pre_meeting_context"],
            copied["hotwords"],
        ],
        capture_complete=stdout_capture.complete and stderr_capture.complete,
        capture_errors=capture_errors,
    )
    log_audit_path = args.run_dir / "22-log-audit.json"
    write_new_json(log_audit_path, log_audit)

    if not worker_record_path.is_file():
        worker_record: dict[str, Any] = {
            "execution_status": "FAILED",
            "finished": False,
            "worker_reported_exit_code": None,
            "error_type": "MissingWorkerRecord",
        }
    else:
        worker_record = scorer.load_json(worker_record_path)
    residual_ids: list[int] = []
    root_pid = process.pid if process is not None else None
    for pid, created_at in observed_process_identities.items():
        if pid == root_pid:
            continue
        try:
            candidate = psutil.Process(pid)
            if candidate.is_running() and math.isclose(
                float(candidate.create_time()), float(created_at), abs_tol=0.001
            ):
                residual_ids.append(pid)
        except psutil.Error:
            continue
    worker_claimed_timing = {
        "started_at": worker_record.get("started_at"),
        "finished_at": worker_record.get("finished_at"),
        "elapsed_seconds": worker_record.get("elapsed_seconds"),
    }
    worker_claimed_resource_usage = worker_record.get("resource_usage")
    if (
        resource_sampler is None
        or worker_resume_monotonic is None
        or worker_exit_monotonic is None
    ):
        raise scorer.ScoringError("独立资源监督器没有形成完整覆盖")
    system_samples = list(resource_sampler.system_samples)
    gpu_samples = list(resource_sampler.gpu_samples)
    sampler_errors = list(resource_sampler.errors)
    all_monitor_errors = parent_monitor_errors + sampler_errors
    peak_gpu_used = max(
        (item["gpu_memory_used_bytes"] for item in gpu_samples), default=0
    )
    baseline_gpu_used = int(pre_gpu_memory["memory_used_bytes"])
    system_max_interval = resource_sampler.maximum_interval(system_samples)
    gpu_max_interval = resource_sampler.maximum_interval(gpu_samples)
    first_system_stamp = min(
        (float(item["sampled_at_monotonic_seconds"]) for item in system_samples),
        default=math.inf,
    )
    last_system_stamp = max(
        (float(item["sampled_at_monotonic_seconds"]) for item in system_samples),
        default=-math.inf,
    )
    first_gpu_stamp = min(
        (float(item["sampled_at_monotonic_seconds"]) for item in gpu_samples),
        default=math.inf,
    )
    last_gpu_stamp = max(
        (float(item["sampled_at_monotonic_seconds"]) for item in gpu_samples),
        default=-math.inf,
    )
    parent_resource_usage = {
        "source": "SUPERVISOR_INDEPENDENT_THREADED_PSUTIL_AND_LOCKED_NVIDIA_SMI_SAMPLING",
        "sample_count": min(len(system_samples), len(gpu_samples)),
        "system_sample_count": len(system_samples),
        "gpu_sample_count": len(gpu_samples),
        "monitor_failed": bool(all_monitor_errors),
        "monitor_error_count": len(all_monitor_errors),
        "monitor_error_types": sorted(set(all_monitor_errors)),
        "system_available_before_start_bytes": parent_system_available_before,
        "system_observed_sampled_min_available_bytes": min(
            (item["system_available_bytes"] for item in system_samples),
            default=0,
        ),
        "process_tree_observed_sampled_peak_rss_bytes": max(
            (item["process_tree_rss_bytes"] for item in system_samples),
            default=0,
        ),
        "gpu_uuid": runtime_lock["environment"].get("gpu_uuid"),
        "gpu_memory_used_before_start_bytes": baseline_gpu_used,
        "gpu_memory_observed_sampled_peak_used_bytes": peak_gpu_used,
        "gpu_memory_observed_sampled_growth_bytes": max(
            0, peak_gpu_used - baseline_gpu_used
        ),
        "gpu_memory_observed_sampled_min_free_bytes": min(
            (item["gpu_memory_free_bytes"] for item in gpu_samples),
            default=0,
        ),
        "system_maximum_sample_interval_seconds": system_max_interval,
        "gpu_maximum_sample_interval_seconds": gpu_max_interval,
        "system_first_sample_before_resume": first_system_stamp
        <= worker_resume_monotonic,
        "gpu_first_sample_before_resume": first_gpu_stamp <= worker_resume_monotonic,
        "system_end_coverage_lag_seconds": max(
            0.0, worker_exit_monotonic - last_system_stamp
        ),
        "gpu_end_coverage_lag_seconds": max(
            0.0, worker_exit_monotonic - last_gpu_stamp
        ),
        "worker_resume_monotonic_seconds": worker_resume_monotonic,
        "worker_exit_monotonic_seconds": worker_exit_monotonic,
        "system_samples": system_samples,
        "gpu_samples": gpu_samples,
    }
    full_run = dict(worker_record)
    full_run.update(
        {
            "schema_version": 3,
            "stage": "S8-M00-R3",
            "evidence_class": "SUPERVISED_LIVE_CUDA_RUN",
            "runner_source": SUPERVISOR_SOURCE,
            "run_id": args.run_id,
            "process_exit_code": return_code,
            "started_at": worker_started,
            "finished_at": worker_finished,
            "elapsed_seconds": parent_elapsed_seconds,
            "worker_claimed_timing": worker_claimed_timing,
            "resource_usage": parent_resource_usage,
            "worker_claimed_resource_usage": worker_claimed_resource_usage,
            "residual_process_count": len(residual_ids),
            "residual_process_ids": residual_ids,
            "supervision": {
                "worker_runner_source": RUNNER_SOURCE,
                "worker_started_at": worker_started,
                "worker_finished_at": worker_finished,
                "parent_elapsed_seconds": parent_elapsed_seconds,
                "worker_reported_exit_code": worker_record.get(
                    "worker_reported_exit_code"
                ),
                "child_exit_code": return_code,
                "timeout": timeout_reason is not None,
                "timeout_reason": timeout_reason,
                "terminated_process_ids": sorted(set(terminated_ids)),
                "job_object": {
                    "assigned": bool(job and job.assigned),
                    "worker_started_suspended_before_assignment": True,
                    "kill_on_close_configured": bool(
                        job and job.kill_on_close_configured
                    ),
                    **job_close_evidence,
                    "process_cleanup": process_cleanup_evidence,
                    "observed_process_identities": {
                        str(pid): created
                        for pid, created in sorted(observed_process_identities.items())
                    },
                    "residual_observed_process_ids_after_close": sorted(residual_ids),
                },
                "stdout_stderr_capture_method": "PARENT_OWNED_ANONYMOUS_PIPE_MEMORY_CAPTURE",
                "log_audit_sha256": scorer.sha256_file(log_audit_path),
                "network_isolation": {
                    "status": (
                        "MACHINE_WIDE_BLOCK_INSTALLED_AND_REMOVAL_VERIFIED"
                        if firewall_removal["removed"]
                        else "MACHINE_WIDE_BLOCK_REMOVAL_NOT_VERIFIED"
                    ),
                    "method": "WINDOWS_FIREWALL_MACHINE_WIDE_ALL_PROGRAMS_BOTH_DIRECTIONS",
                    "scope": firewall_evidence["scope"],
                    "before": firewall_evidence["before"],
                    "installed": firewall_evidence["installed"],
                    "outbound_block_probe": firewall_evidence[
                        "outbound_block_probe"
                    ],
                    "runtime_checks": firewall_runtime_checks,
                    "removal": firewall_removal,
                    "temporary_firewall_rule_removed": firewall_removal["removed"],
                    "observed_remote_connections": sorted(observed_connections),
                    "observed_listening_sockets": sorted(observed_listeners),
                },
                "snapshot_export": snapshot_evidence,
                "run_consumption": {
                    "policy": "RUN_ID_AND_RUN_DIR_BOUND_ONE_ATTEMPT_ONLY",
                    "marker_sha256": consumption_marker_sha256,
                    "ledger_dir_sha256": canonical_path_sha256(ledger_dir),
                    "run_dir_sha256": canonical_path_sha256(args.run_dir),
                },
            },
        }
    )
    full_run.pop("worker_reported_exit_code", None)
    full_run_path = args.run_dir / "23-full-run.json"
    write_new_json(full_run_path, full_run)

    success = (
        return_code == 0
        and timeout_reason is None
        and worker_record.get("execution_status") == "COMPLETED"
        and worker_record.get("finished") is True
        and worker_record.get("worker_reported_exit_code") == 0
        and not residual_ids
        and log_audit["sensitive_hit_count"] == 0
        and log_audit["capture_complete"] is True
        and not observed_connections
        and not observed_listeners
        and bool(firewall_runtime_checks)
        and all(item.get("valid") is True for item in firewall_runtime_checks)
        and not parent_monitor_errors
        and parent_resource_usage["system_observed_sampled_min_available_bytes"]
        >= 3 * 1024**3
        and parent_resource_usage["gpu_memory_observed_sampled_growth_bytes"] > 0
        and parent_resource_usage["system_sample_count"] >= 4
        and parent_resource_usage["gpu_sample_count"] >= 2
        and parent_resource_usage["monitor_failed"] is False
        and parent_resource_usage["system_maximum_sample_interval_seconds"]
        is not None
        and parent_resource_usage["system_maximum_sample_interval_seconds"] <= 1.0
        and parent_resource_usage["gpu_maximum_sample_interval_seconds"] is not None
        and parent_resource_usage["gpu_maximum_sample_interval_seconds"] <= 5.0
        and parent_resource_usage["system_first_sample_before_resume"] is True
        and parent_resource_usage["gpu_first_sample_before_resume"] is True
        and parent_resource_usage["system_end_coverage_lag_seconds"] <= 1.0
        and parent_resource_usage["gpu_end_coverage_lag_seconds"] <= 5.0
        and bool(job and job.assigned)
        and job_close_evidence.get("close_handle_succeeded") is True
        and process_cleanup_evidence.get("process_exited_after_job_close") is True
        and snapshot_evidence.get("input_snapshot_unchanged") is True
        and firewall_removal["removed"]
        and moss_output_path.is_file()
    )
    if not success:
        status.update(
            {
                "status": "FAILED",
                "updated_at": now_iso(),
                "timeout_reason": timeout_reason,
                "child_exit_code": return_code,
                "log_sensitive_hit_count": log_audit["sensitive_hit_count"],
                "observed_remote_connections": sorted(observed_connections),
            }
        )
        update_json(status_path, status)
        return 2

    post_bundle_path = args.run_dir / "24-post-run-bundle.json"
    post_bundle = {
        "schema_version": 1,
        "role": "SIGNED_POST_RUN_EVIDENCE_BUNDLE",
        "run_id": args.run_id,
        "created_at": now_iso(),
        "pre_run_bundle_sha256": scorer.sha256_file(copied["pre_run_bundle"]),
        "artifact_hashes": {
            "pre_run_bundle": scorer.sha256_file(copied["pre_run_bundle"]),
            "pre_run_signature": scorer.sha256_file(copied["pre_run_signature"]),
            "full_run": scorer.sha256_file(full_run_path),
            "moss_full": scorer.sha256_file(moss_output_path),
            "log_audit": scorer.sha256_file(log_audit_path),
        },
    }
    write_new_json(post_bundle_path, post_bundle)
    status.update(
        {
            "status": "AWAITING_EXTERNAL_POST_SIGNATURE",
            "updated_at": now_iso(),
            "post_run_bundle_sha256": scorer.sha256_file(post_bundle_path),
            "decision": "NOT_SCORED",
            "go_allowed": False,
            "next_required_action": "模型进程结束后运行 sign-post，再运行 finalize-score。",
        }
    )
    update_json(status_path, status)
    print(
        json.dumps(
            {
                "run_id": args.run_id,
                "decision": "NOT_SCORED",
                "go_allowed": False,
                "post_run_bundle": str(post_bundle_path),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


def execute(args: argparse.Namespace) -> int:
    try:
        return _execute_impl(args)
    except BaseException as exc:
        if args.run_dir.exists() and args.run_dir.is_dir():
            status_path = args.run_dir / "00-supervisor-status.json"
            try:
                status = scorer.load_json(status_path) if status_path.is_file() else {
                    "schema_version": 1,
                    "role": "CUDA_SUPERVISOR_STATUS",
                    "run_id": args.run_id,
                    "started_at": now_iso(),
                }
            except BaseException:
                status = {
                    "schema_version": 1,
                    "role": "CUDA_SUPERVISOR_STATUS",
                    "run_id": args.run_id,
                    "started_at": now_iso(),
                }
            failure_stage = str(status.get("status", "UNKNOWN"))
            cleanup: dict[str, Any] = {
                "network_rule_removal_verified": False,
                "known_worker_terminated": False,
            }
            try:
                runtime_lock = scorer.load_json(args.runtime_lock)
                powershell_path = scorer.locked_external_tool(
                    runtime_lock, "powershell"
                )
                removal = firewall_remove(
                    {
                        "rule_names": [
                            f"Meetily-R3-{args.run_id}-IN",
                            f"Meetily-R3-{args.run_id}-OUT",
                        ]
                    },
                    powershell_path,
                )
                cleanup["network_rule_removal_verified"] = removal.get("removed") is True
                cleanup["network_removal"] = removal
            except BaseException as cleanup_exc:
                cleanup["network_removal_error_type"] = type(cleanup_exc).__name__
            worker_pid = status.get("worker_pid")
            if isinstance(worker_pid, int) and not isinstance(worker_pid, bool):
                terminate_tree(worker_pid)
                try:
                    cleanup["known_worker_terminated"] = not psutil.pid_exists(worker_pid)
                except BaseException:
                    cleanup["known_worker_terminated"] = False
            else:
                cleanup["known_worker_terminated"] = True
            failure_record = {
                "schema_version": 1,
                "role": "SUPERVISOR_FAIL_CLOSED_RECORD",
                "run_id": args.run_id,
                "failed_at": now_iso(),
                "failure_stage": failure_stage,
                "error_type": type(exc).__name__,
                "error_message_sha256": hashlib.sha256(
                    str(exc).encode("utf-8")
                ).hexdigest(),
                "cleanup": cleanup,
                "decision": "NO-GO",
                "go_allowed": False,
            }
            failure_path = args.run_dir / "99-supervisor-failure.json"
            try:
                write_new_json(failure_path, failure_record)
            except BaseException:
                pass
            status.update(
                {
                    "status": "FAILED_CLOSED",
                    "updated_at": now_iso(),
                    "failure_stage": failure_stage,
                    "failure_record_sha256": (
                        scorer.sha256_file(failure_path)
                        if failure_path.is_file()
                        else None
                    ),
                    "cleanup": cleanup,
                    "decision": "NO-GO",
                    "go_allowed": False,
                    "next_required_action": "该 run_id 已消费；修复后必须重新 seal 并使用新 run_id。",
                }
            )
            try:
                update_json(status_path, status)
            except BaseException:
                pass
        raise


def sign_post(args: argparse.Namespace) -> int:
    require_signing_key_outside_workspace(args.private_key, args.workspace_root)
    runtime_lock = scorer.load_json(args.runtime_lock)
    scorer.verify_runtime_integrity(runtime_lock, require_isolated_python=False)
    ssh_keygen_path = scorer.locked_external_tool(runtime_lock, "ssh_keygen")
    verify_private_public_pair(
        args.private_key, args.attestation_public_key, ssh_keygen_path
    )
    bundle = scorer.load_json(args.post_run_bundle)
    run_id = str(bundle.get("run_id", ""))
    scorer.require_signed_bundle_schema(
        bundle, "SIGNED_POST_RUN_EVIDENCE_BUNDLE", run_id
    )
    rules = scorer.load_json(args.rules)
    scorer.require_frozen_policy(rules)
    if args.post_run_signature_out.exists():
        raise scorer.ScoringError("拒绝覆盖已有运行后签名")
    status_path = args.post_run_bundle.parent / "00-supervisor-status.json"
    status = scorer.load_json(status_path)
    if (
        status.get("run_id") != run_id
        or status.get("status") != "AWAITING_EXTERNAL_POST_SIGNATURE"
        or status.get("post_run_bundle_sha256")
        != scorer.sha256_file(args.post_run_bundle)
    ):
        raise scorer.ScoringError("supervisor 状态与待签运行后证据不一致")
    sign_file(
        args.post_run_bundle,
        args.post_run_signature_out,
        args.private_key,
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    verification = scorer.verify_ssh_signature(
        args.post_run_bundle,
        args.post_run_signature_out,
        args.attestation_public_key,
        str(rules["attestation"]["signer_identity"]),
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    if verification.get("valid") is not True:
        raise scorer.ScoringError("运行后证据包签名复核失败")
    status.update(
        {
            "status": "POST_RUN_SIGNED",
            "updated_at": now_iso(),
            "post_run_signature_sha256": scorer.sha256_file(
                args.post_run_signature_out
            ),
            "decision": "NOT_SCORED",
            "go_allowed": False,
            "next_required_action": "运行 finalize-score；评分文件生成后仍须运行 sign-score。",
        }
    )
    update_json(status_path, status)
    result = {
        "decision": "POST_RUN_BUNDLE_SIGNED",
        "go_allowed": False,
        "run_id": run_id,
        "bundle_sha256": scorer.sha256_file(args.post_run_bundle),
        "signature_sha256": scorer.sha256_file(args.post_run_signature_out),
        "truth_boundary": "该步骤仅在模型进程结束后签字；尚未完成最终评分。",
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


def finalize_score(args: argparse.Namespace) -> int:
    if not re.fullmatch(r"[0-9a-f]{32}", args.run_id):
        raise scorer.ScoringError("run_id 必须是 32 位小写十六进制")
    validate_static_inputs(args)
    run_dir = args.run_dir.resolve(strict=True)
    full_run_path = run_dir / "23-full-run.json"
    moss_output_path = run_dir / "20-moss-full.json"
    log_audit_path = run_dir / "22-log-audit.json"
    post_bundle_path = run_dir / "24-post-run-bundle.json"
    score_path = run_dir / "25-score.json"
    status_path = run_dir / "00-supervisor-status.json"
    for path in (
        full_run_path,
        moss_output_path,
        log_audit_path,
        post_bundle_path,
        args.post_run_signature,
    ):
        if not path.is_file():
            raise scorer.ScoringError(f"最终评分缺少证据: {path}")
    if score_path.exists():
        raise scorer.ScoringError("拒绝覆盖已有最终评分")
    pre_bundle = scorer.load_json(args.pre_run_bundle)
    full_run = scorer.load_json(full_run_path)
    moss_output = scorer.load_json(moss_output_path)
    log_audit = scorer.load_json(log_audit_path)
    post_bundle = scorer.load_json(post_bundle_path)
    status = scorer.load_json(status_path)
    run_id_values = {
        "cli": args.run_id,
        "pre_bundle": pre_bundle.get("run_id"),
        "full_run": full_run.get("run_id"),
        "moss_output": moss_output.get("run_id"),
        "log_audit": log_audit.get("run_id"),
        "post_bundle": post_bundle.get("run_id"),
        "status": status.get("run_id"),
    }
    if set(run_id_values.values()) != {args.run_id}:
        raise scorer.ScoringError("最终评分的 CLI/预签/运行/输出/日志/后签 run_id 不一致")
    if status.get("status") != "POST_RUN_SIGNED":
        raise scorer.ScoringError("运行后证据尚未完成外部签名，拒绝评分")
    if status.get("post_run_signature_sha256") != scorer.sha256_file(
        args.post_run_signature
    ):
        raise scorer.ScoringError("状态记录的运行后签名哈希不一致")
    score_args = argparse.Namespace(
        audio=args.window_audio,
        full_audio=args.full_audio,
        verbatim=args.verbatim,
        turns=args.turns,
        review=args.review,
        hotwords=args.hotwords,
        pre_meeting_context=args.pre_meeting_context,
        rules=args.rules,
        source_lock=args.source_lock,
        model_root=args.model_root,
        model_manifest=args.model_manifest,
        runtime_lock=args.runtime_lock,
        runtime_bootstrap=args.runtime_bootstrap,
        source_code=args.source_code,
        whisper_full=args.whisper_full,
        whisper_run_record=args.whisper_run_record,
        whisper_model=args.whisper_model,
        stable_exe=args.stable_exe,
        prompt=args.prompt,
        runner=args.runner,
        supervisor=Path(__file__).resolve(),
        pip_freeze=args.pip_freeze,
        pre_run_bundle=args.pre_run_bundle,
        pre_run_signature=args.pre_run_signature,
        post_run_bundle=post_bundle_path,
        post_run_signature=args.post_run_signature,
        attestation_public_key=args.attestation_public_key,
        log_audit=log_audit_path,
        moss_full=moss_output_path,
        full_run=full_run_path,
    )
    result = scorer.score_gate(score_args, live_execution_context=True)
    write_new_json(score_path, result)
    score_sha256 = scorer.sha256_file(score_path)
    status.update(
        {
            "status": "AWAITING_EXTERNAL_SCORE_SIGNATURE",
            "updated_at": now_iso(),
            "score_sha256": score_sha256,
            "decision": result.get("decision"),
            "go_allowed": False,
            "unsigned_score_go_allowed": result.get("go_allowed"),
            "next_required_action": "在模型进程已结束的外部签名环境运行 sign-score。",
        }
    )
    update_json(status_path, status)
    print(
        json.dumps(
            {
                "run_id": args.run_id,
                "decision": result.get("decision"),
                "go_allowed": result.get("go_allowed"),
                "score": str(score_path),
                "score_sha256": score_sha256,
                "final_evidence_signed": False,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0 if result.get("go_allowed") is True else 2


def sign_score(args: argparse.Namespace) -> int:
    require_signing_key_outside_workspace(args.private_key, args.workspace_root)
    runtime_lock = scorer.load_json(args.runtime_lock)
    scorer.verify_runtime_integrity(runtime_lock, require_isolated_python=False)
    ssh_keygen_path = scorer.locked_external_tool(runtime_lock, "ssh_keygen")
    verify_private_public_pair(
        args.private_key, args.attestation_public_key, ssh_keygen_path
    )
    score = scorer.load_json(args.score)
    run_id = str(score.get("run_id", ""))
    if (
        score.get("schema_version") != 4
        or score.get("stage") != "S8-M00-R3"
        or not re.fullmatch(r"[0-9a-f]{32}", run_id)
        or not isinstance(score.get("go_allowed"), bool)
    ):
        raise scorer.ScoringError("最终评分文件 schema/run_id 无效")
    status_path = args.score.parent / "00-supervisor-status.json"
    status = scorer.load_json(status_path)
    score_sha256 = scorer.sha256_file(args.score)
    if (
        status.get("run_id") != run_id
        or status.get("status") != "AWAITING_EXTERNAL_SCORE_SIGNATURE"
        or status.get("score_sha256") != score_sha256
        or status.get("unsigned_score_go_allowed") is not score.get("go_allowed")
    ):
        raise scorer.ScoringError("supervisor 状态与待签最终评分不一致")
    if args.score_signature_out.exists():
        raise scorer.ScoringError("拒绝覆盖已有最终评分签名")
    rules = scorer.load_json(args.rules)
    scorer.require_frozen_policy(rules)
    sign_file(
        args.score,
        args.score_signature_out,
        args.private_key,
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    verification = scorer.verify_ssh_signature(
        args.score,
        args.score_signature_out,
        args.attestation_public_key,
        str(rules["attestation"]["signer_identity"]),
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    if verification.get("valid") is not True:
        raise scorer.ScoringError("最终评分签名复核失败")
    final_status = (
        "FINAL_GO_SIGNED" if score.get("go_allowed") is True else "FINAL_NO_GO_SIGNED"
    )
    status.update(
        {
            "status": final_status,
            "updated_at": now_iso(),
            "finalized_at": now_iso(),
            "decision": score.get("decision"),
            "go_allowed": score.get("go_allowed"),
            "score_sha256": score_sha256,
            "score_signature_sha256": scorer.sha256_file(
                args.score_signature_out
            ),
            "final_evidence_signed": True,
            "next_required_action": "运行 verify-final 复核最终签名；不得手工修改评分或状态。",
        }
    )
    update_json(status_path, status)
    print(
        json.dumps(
            {
                "run_id": run_id,
                "decision": score.get("decision"),
                "go_allowed": score.get("go_allowed"),
                "status": final_status,
                "score_sha256": score_sha256,
                "score_signature_sha256": scorer.sha256_file(
                    args.score_signature_out
                ),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0 if score.get("go_allowed") is True else 2


def verify_final(args: argparse.Namespace) -> int:
    runtime_lock = scorer.load_json(args.runtime_lock)
    scorer.verify_runtime_integrity(runtime_lock, require_isolated_python=False)
    ssh_keygen_path = scorer.locked_external_tool(runtime_lock, "ssh_keygen")
    rules = scorer.load_json(args.rules)
    scorer.require_frozen_policy(rules)
    score = scorer.load_json(args.score)
    status = scorer.load_json(args.score.parent / "00-supervisor-status.json")
    verification = scorer.verify_ssh_signature(
        args.score,
        args.score_signature,
        args.attestation_public_key,
        str(rules["attestation"]["signer_identity"]),
        str(rules["attestation"]["namespace"]),
        ssh_keygen_path,
    )
    expected_status = (
        "FINAL_GO_SIGNED" if score.get("go_allowed") is True else "FINAL_NO_GO_SIGNED"
    )
    valid = (
        score.get("schema_version") == 4
        and status.get("run_id") == score.get("run_id")
        and status.get("status") == expected_status
        and status.get("decision") == score.get("decision")
        and status.get("go_allowed") is score.get("go_allowed")
        and status.get("score_sha256") == scorer.sha256_file(args.score)
        and status.get("score_signature_sha256")
        == scorer.sha256_file(args.score_signature)
        and status.get("final_evidence_signed") is True
        and verification.get("valid") is True
    )
    print(
        json.dumps(
            {
                "run_id": score.get("run_id"),
                "decision": score.get("decision") if valid else "NO-GO",
                "go_allowed": score.get("go_allowed") is True and valid,
                "final_signature_valid": verification.get("valid") is True,
                "status_consistent": valid,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0 if valid and score.get("go_allowed") is True else 2


def add_common(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--workspace-root", type=Path, required=True)
    parser.add_argument("--run-id", default=None)
    parser.add_argument("--window-audio", type=Path, required=True)
    parser.add_argument("--full-audio", type=Path, required=True)
    parser.add_argument("--source-lock", type=Path, required=True)
    parser.add_argument("--verbatim", type=Path, required=True)
    parser.add_argument("--turns", type=Path, required=True)
    parser.add_argument("--review", type=Path, required=True)
    parser.add_argument("--hotwords", type=Path, required=True)
    parser.add_argument("--pre-meeting-context", type=Path, required=True)
    parser.add_argument("--rules", type=Path, required=True)
    parser.add_argument("--whisper-full", type=Path, required=True)
    parser.add_argument("--whisper-run-record", type=Path, required=True)
    parser.add_argument("--whisper-model", type=Path, required=True)
    parser.add_argument("--stable-exe", type=Path, required=True)
    parser.add_argument("--prompt", type=Path, required=True)
    parser.add_argument("--model-root", type=Path, required=True)
    parser.add_argument("--model-manifest", type=Path, required=True)
    parser.add_argument("--runtime-lock", type=Path, required=True)
    parser.add_argument("--runtime-bootstrap", type=Path, required=True)
    parser.add_argument("--source-code", type=Path, required=True)
    parser.add_argument("--scorer", type=Path, required=True)
    parser.add_argument("--runner", type=Path, required=True)
    parser.add_argument("--pip-freeze", type=Path, required=True)
    parser.add_argument("--attestation-public-key", type=Path, required=True)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="S8-M00-R3 严格 CUDA supervisor")
    commands = parser.add_subparsers(dest="command", required=True)
    freeze = commands.add_parser("freeze-policy")
    freeze.add_argument("--source-lock-draft", type=Path, required=True)
    freeze.add_argument("--rules-draft", type=Path, required=True)
    freeze.add_argument("--source-lock-out", type=Path, required=True)
    freeze.add_argument("--rules-out", type=Path, required=True)
    freeze.add_argument("--audit-out", type=Path, required=True)
    freeze.add_argument("--window-audio", type=Path, required=True)
    freeze.add_argument("--full-audio", type=Path, required=True)
    freeze.add_argument("--verbatim", type=Path, required=True)
    freeze.add_argument("--turns", type=Path, required=True)
    freeze.add_argument("--review", type=Path, required=True)
    freeze.add_argument("--hotwords", type=Path, required=True)
    freeze.add_argument("--pre-meeting-context", type=Path, required=True)
    freeze.add_argument("--whisper-full", type=Path, required=True)
    freeze.add_argument("--whisper-run-record", type=Path, required=True)
    freeze.add_argument("--whisper-model", type=Path, required=True)
    freeze.add_argument("--stable-exe", type=Path, required=True)
    freeze.add_argument("--model-root", type=Path, required=True)
    freeze.add_argument("--model-manifest", type=Path, required=True)
    freeze.add_argument("--runtime-lock", type=Path, required=True)
    freeze.add_argument("--runtime-bootstrap", type=Path, required=True)
    freeze.add_argument("--source-code", type=Path, required=True)
    freeze.add_argument("--scorer", type=Path, required=True)
    freeze.add_argument("--runner", type=Path, required=True)
    freeze.add_argument("--whisper-supervisor", type=Path, required=True)
    freeze.add_argument("--pip-freeze", type=Path, required=True)
    freeze.add_argument("--attestation-public-key", type=Path, required=True)
    freeze.add_argument("--signer-identity", required=True)

    seal = commands.add_parser("seal-inputs")
    add_common(seal)
    seal.add_argument("--private-key", type=Path, required=True)
    seal.add_argument("--run-dir", type=Path, required=True)
    seal.add_argument("--run-ledger-dir", type=Path, required=True)
    seal.add_argument("--bundle-out", type=Path, required=True)
    seal.add_argument("--signature-out", type=Path, required=True)

    execute_parser = commands.add_parser("execute")
    add_common(execute_parser)
    execute_parser.add_argument("--pre-run-bundle", type=Path, required=True)
    execute_parser.add_argument("--pre-run-signature", type=Path, required=True)
    execute_parser.add_argument("--run-dir", type=Path, required=True)
    execute_parser.add_argument("--run-ledger-dir", type=Path, required=True)
    execute_parser.add_argument("--model-load-timeout-seconds", type=float, default=600.0)
    execute_parser.add_argument("--inference-timeout-seconds", type=float, default=1200.0)
    execute_parser.add_argument("--total-timeout-seconds", type=float, default=1800.0)

    sign_parser = commands.add_parser("sign-post")
    sign_parser.add_argument("--workspace-root", type=Path, required=True)
    sign_parser.add_argument("--rules", type=Path, required=True)
    sign_parser.add_argument("--runtime-lock", type=Path, required=True)
    sign_parser.add_argument("--post-run-bundle", type=Path, required=True)
    sign_parser.add_argument("--post-run-signature-out", type=Path, required=True)
    sign_parser.add_argument("--attestation-public-key", type=Path, required=True)
    sign_parser.add_argument("--private-key", type=Path, required=True)

    finalize_parser = commands.add_parser("finalize-score")
    add_common(finalize_parser)
    finalize_parser.add_argument("--pre-run-bundle", type=Path, required=True)
    finalize_parser.add_argument("--pre-run-signature", type=Path, required=True)
    finalize_parser.add_argument("--post-run-signature", type=Path, required=True)
    finalize_parser.add_argument("--run-dir", type=Path, required=True)

    sign_score_parser = commands.add_parser("sign-score")
    sign_score_parser.add_argument("--workspace-root", type=Path, required=True)
    sign_score_parser.add_argument("--rules", type=Path, required=True)
    sign_score_parser.add_argument("--runtime-lock", type=Path, required=True)
    sign_score_parser.add_argument("--score", type=Path, required=True)
    sign_score_parser.add_argument("--score-signature-out", type=Path, required=True)
    sign_score_parser.add_argument("--attestation-public-key", type=Path, required=True)
    sign_score_parser.add_argument("--private-key", type=Path, required=True)

    verify_parser = commands.add_parser("verify-final")
    verify_parser.add_argument("--rules", type=Path, required=True)
    verify_parser.add_argument("--runtime-lock", type=Path, required=True)
    verify_parser.add_argument("--score", type=Path, required=True)
    verify_parser.add_argument("--score-signature", type=Path, required=True)
    verify_parser.add_argument("--attestation-public-key", type=Path, required=True)
    return parser


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    args = build_parser().parse_args()
    if hasattr(args, "run_id") and not args.run_id:
        args.run_id = uuid.uuid4().hex
    try:
        if args.command == "freeze-policy":
            return freeze_policy(args)
        if args.command == "seal-inputs":
            return seal_inputs(args)
        if args.command == "execute":
            return execute(args)
        if args.command == "sign-post":
            return sign_post(args)
        if args.command == "finalize-score":
            return finalize_score(args)
        if args.command == "sign-score":
            return sign_score(args)
        return verify_final(args)
    except (Exception, KeyboardInterrupt) as exc:
        print(
            json.dumps(
                {
                    "decision": "NO-GO",
                    "go_allowed": False,
                    "error": type(exc).__name__,
                    "message": str(exc),
                },
                ensure_ascii=False,
                indent=2,
            )
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
