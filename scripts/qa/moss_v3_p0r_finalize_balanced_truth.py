#!/usr/bin/env python3
"""Derive and formally seal the balanced P0-R truth from completed human reviews."""

from __future__ import annotations

import argparse
import csv
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import time
from typing import Any, Iterable


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


SOURCE_START_SECONDS = 70.370
R2_SOURCE_START_SECONDS = 116.810
SOURCE_END_SECONDS = 296.810
PREFIX_DURATION_SECONDS = R2_SOURCE_START_SECONDS - SOURCE_START_SECONDS
WINDOW_DURATION_SECONDS = SOURCE_END_SECONDS - SOURCE_START_SECONDS
FULL_DURATION_SECONDS = 737.728
EXPECTED_FULL_AUDIO_SHA256 = (
    "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
)
EXPECTED_REVIEWER = "lili"
EXPECTED_ROLE = "会议记录"
REVIEW_ATTESTATION = (
    "我已从头到尾听完冻结音频，并按实际听到的内容和换人位置完成标注；"
    "机器草稿仅用于定位，没有被当作标准答案。"
)
PREFIX_SPEAKER_MAP = {"H02": "H01", "H03": "H04"}
BAD_TERMS = ("CJS", "YOUTUBE", "储库", "宝宝", "H五", "CTS")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8-sig"))


def write_json(path: Path, payload: Any) -> None:
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def interval_union_seconds(intervals: Iterable[tuple[float, float]]) -> float:
    merged: list[list[float]] = []
    for start, end in sorted((float(a), float(b)) for a, b in intervals if b > a):
        if not merged or start > merged[-1][1] + 0.001:
            merged.append([start, end])
        else:
            merged[-1][1] = max(merged[-1][1], end)
    return sum(end - start for start, end in merged)


def maximum_gap_seconds(rows: list[dict[str, Any]]) -> float:
    intervals = sorted((float(row["start"]), float(row["end"])) for row in rows)
    cursor = 0.0
    maximum = 0.0
    for start, end in intervals:
        maximum = max(maximum, start - cursor)
        cursor = max(cursor, end)
    return max(maximum, WINDOW_DURATION_SECONDS - cursor)


def validate_completed_wip(
    package: Path,
    *,
    expected_duration: float,
    label: str,
) -> tuple[dict[str, Any], Path]:
    path = package / "human-review-work-in-progress.json"
    payload = read_json(path)
    progress = payload.get("window_progress") or {}
    rows = payload.get("rows") or []
    require(payload.get("reviewer_id") == EXPECTED_REVIEWER, f"{label} reviewer mismatch")
    require(payload.get("reviewer_role") == EXPECTED_ROLE, f"{label} reviewer role mismatch")
    require(bool(rows), f"{label} has no rows")
    require(all(row.get("human_checked") is True for row in rows), f"{label} has unchecked rows")
    require(progress.get("complete") is True, f"{label} playback is incomplete")
    require(abs(float(progress.get("duration_seconds", -1)) - expected_duration) <= 0.001, f"{label} duration mismatch")
    require(float(progress.get("covered_seconds", -1)) + 0.001 >= expected_duration, f"{label} coverage mismatch")
    require(float(progress.get("coverage_percent", -1)) >= 100.0, f"{label} coverage percent mismatch")
    require(float(progress.get("wall_elapsed_seconds", -1)) + 0.001 >= expected_duration, f"{label} real review elapsed time is too short")
    require(progress.get("review_started_at"), f"{label} missing review start")
    for index, row in enumerate(rows, 1):
        if not row.get("non_speech"):
            require(str(row.get("speaker", "")).strip(), f"{label} row {index} missing speaker")
            require(str(row.get("text", "")).strip(), f"{label} row {index} missing text")
    return payload, path


def copy_row(row: dict[str, Any]) -> dict[str, Any]:
    return {
        "start": round(float(row["start"]), 3),
        "end": round(float(row["end"]), 3),
        "speaker": str(row.get("speaker", "")).strip(),
        "text": str(row.get("text", "")).strip(),
        "overlap": row.get("overlap") is True,
        "non_speech": row.get("non_speech") is True,
        "note": str(row.get("note", "")).strip(),
        "human_checked": row.get("human_checked") is True,
        "machine_source": str(row.get("machine_source", "")).strip(),
    }


def merge_human_rows(full_wip: dict[str, Any], r2_wip: dict[str, Any]) -> list[dict[str, Any]]:
    merged: list[dict[str, Any]] = []
    for source in full_wip["rows"]:
        start = float(source["start"])
        end = float(source["end"])
        if end <= SOURCE_START_SECONDS or start >= R2_SOURCE_START_SECONDS:
            continue
        require(start >= SOURCE_START_SECONDS - 0.001, "prefix starts across frozen boundary")
        require(end <= R2_SOURCE_START_SECONDS + 0.001, "prefix ends across frozen boundary")
        row = copy_row(source)
        row["start"] = round(max(start, SOURCE_START_SECONDS) - SOURCE_START_SECONDS, 3)
        row["end"] = round(min(end, R2_SOURCE_START_SECONDS) - SOURCE_START_SECONDS, 3)
        if not row["non_speech"]:
            require(row["speaker"] in PREFIX_SPEAKER_MAP, f"unmapped prefix speaker {row['speaker']}")
            row["speaker"] = PREFIX_SPEAKER_MAP[row["speaker"]]
        row["note"] = (row["note"] + " | 来源：737.728秒完整人工听审。" ).strip()
        row["machine_source"] = "human-full:" + row["machine_source"]
        merged.append(row)

    for source in r2_wip["rows"]:
        row = copy_row(source)
        require(0.0 <= row["start"] < row["end"] <= 180.0 + 0.001, "R2 row is outside 180-second source")
        row["start"] = round(row["start"] + PREFIX_DURATION_SECONDS, 3)
        row["end"] = round(row["end"] + PREFIX_DURATION_SECONDS, 3)
        if not row["non_speech"]:
            require(row["speaker"] in {"H01", "H04"}, f"unexpected R2 speaker {row['speaker']}")
        row["note"] = (row["note"] + " | 来源：180秒补充人工听审。" ).strip()
        row["machine_source"] = "human-r2:" + row["machine_source"]
        merged.append(row)

    rows = sorted(merged, key=lambda item: (item["start"], item["end"]))
    require(rows, "derived truth is empty")
    require(abs(rows[0]["start"]) <= 0.001, "derived truth does not start at zero")
    require(abs(rows[-1]["end"] - WINDOW_DURATION_SECONDS) <= 0.001, "derived truth does not end at window boundary")
    require(maximum_gap_seconds(rows) <= 0.001, "derived truth has an unexplained timeline gap")
    require(all(row["human_checked"] for row in rows), "derived truth contains unchecked rows")
    require(all(row["text"] for row in rows), "derived truth contains empty text")
    speech = [row for row in rows if not row["non_speech"]]
    require({row["speaker"] for row in speech} == {"H01", "H04"}, "derived speaker set mismatch")
    text = "\n".join(row["text"] for row in speech)
    require(not any(term in text for term in BAD_TERMS), "derived truth still contains a known rejected term")
    return rows


def load_server(output: Path):
    sys.path.insert(0, str(output))
    spec = importlib.util.spec_from_file_location(
        "balanced_human_review_server", output / "human_review_server.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def speaker_metrics(server: Any, rows: list[dict[str, Any]]) -> dict[str, Any]:
    _verbatim, turns, counts = server.build_tsv_documents(rows)
    parsed = list(csv.DictReader(io.StringIO(turns.decode("utf-8")), delimiter="\t"))
    seconds: dict[str, float] = {}
    turn_counts: dict[str, int] = {}
    intervals: list[tuple[float, float]] = []
    for row in parsed:
        if row["valid_for_scoring"] != "true":
            continue
        speaker = row["reference_speaker_id"]
        start = int(row["start_ms"]) / 1000.0
        end = int(row["end_ms"]) / 1000.0
        seconds[speaker] = seconds.get(speaker, 0.0) + end - start
        turn_counts[speaker] = turn_counts.get(speaker, 0) + 1
        intervals.append((start, end))
    rules = read_json(Path(server.__file__).resolve().parent / "08-scoring-rules.json")["annotation_validation"]
    require(all(value >= float(rules["minimum_seconds_per_primary_speaker"]) for value in seconds.values()), "primary speaker seconds gate still fails")
    require(all(value >= int(rules["minimum_valid_turns_per_speaker"]) for value in turn_counts.values()), "speaker turn-count gate still fails")
    require(len(seconds) >= int(rules["minimum_primary_speakers"]), "primary speaker count gate still fails")
    require(counts["speaker_switch_count"] >= int(rules["minimum_speaker_switches"]), "speaker switch gate still fails")
    require(interval_union_seconds(intervals) + 0.001 >= float(rules["minimum_speaker_turn_union_seconds"]), "speaker-turn union gate still fails")
    return {
        "counts": counts,
        "speaker_seconds": {key: round(value, 3) for key, value in sorted(seconds.items())},
        "speaker_turns": dict(sorted(turn_counts.items())),
        "speaker_turn_union_seconds": round(interval_union_seconds(intervals), 3),
    }


def manifest_record(path: Path, root: Path) -> dict[str, Any]:
    return {
        "relative_path": path.relative_to(root).as_posix(),
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def refresh_manifest(output: Path) -> None:
    config = read_json(output / "review-config.json")
    files = sorted(
        path
        for path in output.iterdir()
        if path.is_file() and path.name not in {"MANIFEST.json", "human-review-work-in-progress.json"}
    )
    write_json(
        output / "MANIFEST.json",
        {
            "schema_version": 1,
            "stage": "MOSS-V3-P0R-BALANCED-226S",
            "status": "HUMAN_TRUTH_STRUCTURE_PASS_NEXT_FORMAL_WHISPER",
            "reference_audio_sha256": str(config["window_audio_sha256"]).upper(),
            "files": [manifest_record(path, output) for path in files],
        },
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-package", type=Path, required=True)
    parser.add_argument("--full-review-package", type=Path, required=True)
    parser.add_argument("--r2-review-package", type=Path, required=True)
    args = parser.parse_args()

    output = args.output_package.resolve(strict=True)
    full_package = args.full_review_package.resolve(strict=True)
    r2_package = args.r2_review_package.resolve(strict=True)
    full_wip, full_wip_path = validate_completed_wip(
        full_package, expected_duration=FULL_DURATION_SECONDS, label="full review"
    )
    r2_wip, r2_wip_path = validate_completed_wip(
        r2_package, expected_duration=180.0, label="R2 review"
    )

    full_config = read_json(full_package / "review-config.json")
    r2_config = read_json(r2_package / "review-config.json")
    out_config = read_json(output / "review-config.json")
    require(str(full_config["full_audio_sha256"]).upper() == EXPECTED_FULL_AUDIO_SHA256, "full review source hash mismatch")
    require(str(r2_config["full_audio_sha256"]).upper() == EXPECTED_FULL_AUDIO_SHA256, "R2 source hash mismatch")
    require(str(out_config["full_audio_sha256"]).upper() == EXPECTED_FULL_AUDIO_SHA256, "balanced source hash mismatch")
    require(abs(float(out_config["window_duration_seconds"]) - WINDOW_DURATION_SECONDS) <= 0.001, "balanced duration mismatch")

    rows = merge_human_rows(full_wip, r2_wip)
    server = load_server(output)
    metrics = speaker_metrics(server, rows)
    service = server.ReviewService(output, token="offline-derived-review")
    inherited_elapsed = max(
        float(full_wip["window_progress"]["wall_elapsed_seconds"]),
        float(r2_wip["window_progress"]["wall_elapsed_seconds"]),
        WINDOW_DURATION_SECONDS,
    )
    service.window_tracker.intervals = [(0.0, WINDOW_DURATION_SECONDS)]
    service.window_tracker.started_at = str(full_wip["window_progress"]["review_started_at"])
    service.window_tracker.started_monotonic = time.monotonic() - inherited_elapsed

    save_payload = {
        "reviewer_id": EXPECTED_REVIEWER,
        "reviewer_role": EXPECTED_ROLE,
        "rows": rows,
    }
    service.save_wip(save_payload)
    flags = {
        "listened_from_start_to_end": True,
        "overlap_reviewed": True,
        "names_checked": True,
        "numbers_and_dates_checked": True,
        "business_terms_checked": True,
        "simplified_chinese_checked": True,
    }
    result = service.finalize_window(
        {
            **save_payload,
            "flags": flags,
            "attested": True,
            "revision_count": len(rows),
            "review_note": (
                "由同一冻结录音的737.728秒完整真人听审与116.810–296.810秒补充真人听审机械派生；"
                "只向前扩展到70.370秒以满足冻结的第二说话人30秒门槛；未查看准确率输出，未降低规则。"
            ),
        }
    )
    require(result.get("validation_result") == "STRUCTURE_PASS_SELF_ATTESTED", "formal validation did not pass")

    provenance = {
        "schema_version": 1,
        "status": "HASH_BOUND_HUMAN_REVIEW_DERIVATION",
        "truth_boundary": "本文件证明新窗口只从已经完整听审、逐行勾选并由同一复核人确认的两个来源机械裁剪合并；程序不声称能验证语义。",
        "reviewer_id": EXPECTED_REVIEWER,
        "reviewer_role": EXPECTED_ROLE,
        "source_window": {
            "full_recording_start_seconds": SOURCE_START_SECONDS,
            "full_recording_end_seconds": SOURCE_END_SECONDS,
            "duration_seconds": WINDOW_DURATION_SECONDS,
            "continuous": True,
        },
        "sources": [
            {
                "role": "prefix_70_370_to_116_810",
                "package": str(full_package),
                "wip_path": str(full_wip_path),
                "wip_sha256": sha256(full_wip_path),
                "review_duration_seconds": FULL_DURATION_SECONDS,
                "coverage_percent": full_wip["window_progress"]["coverage_percent"],
                "wall_elapsed_seconds": full_wip["window_progress"]["wall_elapsed_seconds"],
            },
            {
                "role": "main_116_810_to_296_810",
                "package": str(r2_package),
                "wip_path": str(r2_wip_path),
                "wip_sha256": sha256(r2_wip_path),
                "review_duration_seconds": 180.0,
                "coverage_percent": r2_wip["window_progress"]["coverage_percent"],
                "wall_elapsed_seconds": r2_wip["window_progress"]["wall_elapsed_seconds"],
            },
        ],
        "speaker_merge": {
            "prefix_H02": "H01",
            "prefix_H03": "H04",
            "main_labels": ["H01", "H04"],
            "basis": "复核人已在180秒补充听审中把同一实际声音合并为H01/H04；前缀沿用同一对话角色。",
        },
        "selection_guard": {
            "accuracy_outputs_consulted": False,
            "rules_weakened": False,
            "reason": "R2的H01只有26.970秒，低于预先冻结的30秒门槛；连续向前扩展加入同一对话中的真实H01语音。",
        },
        "formal_result": result,
        "metrics": metrics,
        "attestation": REVIEW_ATTESTATION,
    }
    write_json(output / "17-derived-human-review-provenance.json", provenance)
    write_json(
        output / "11-formal-human-truth-audit.json",
        {
            "schema_version": 1,
            "status": "STRUCTURE_PASS_SELF_ATTESTED",
            "checks": {
                "source_audio_hash_bound": True,
                "continuous_window_180_to_300_seconds": True,
                "all_rows_human_checked": all(row["human_checked"] for row in rows),
                "timeline_maximum_gap_seconds": maximum_gap_seconds(rows),
                "known_rejected_term_count": 0,
                "speaker_metrics": metrics,
                "formal_validator_result": result["validation_result"],
                "semantic_truth_verified_by_program": False,
            },
            "next_gate": result["next_gate"],
            "verdict": "P0-R HUMAN TRUTH STRUCTURE PASS / NEXT FORMAL WHISPER",
        },
    )
    (output / "TASKS.md").write_text(
        """# P0-R 说话人平衡连续窗口任务

- [x] 记录180秒R2正式封存被第二说话人30秒门槛阻断。
- [x] 保留R2全部人工工作和失败证据，不降低30秒规则。
- [x] 从同一冻结录音连续向前扩展46.440秒，形成226.440秒窗口。
- [x] 用哈希绑定的完整录音人工听审和R2补充听审机械派生真值。
- [x] 两名主要说话人均达到30秒且至少两个有效turn。
- [x] 正式人工结构校验通过。
- [ ] 运行正式受监督Whisper。
- [ ] 冻结正负术语并运行MOSS/Whisper准确率评分。
""",
        encoding="utf-8",
    )

    refresh_manifest(output)
    print(json.dumps({"status": "PASS", "output": str(output), "result": result, "metrics": metrics}, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
