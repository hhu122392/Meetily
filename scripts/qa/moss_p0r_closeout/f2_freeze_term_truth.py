from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
import sys
import unicodedata
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable


POSITIVE_TERMS = (
    ("YouTube", "business_term", "CTX-026"),
    ("PWA", "business_term", "CTX-027"),
    ("Google", "business_term", "CTX-029"),
)

NEGATIVE_REVIEW_CANDIDATES = (
    ("Karl", "person_or_alias", "CTX-023"),
    ("A/B Test", "business_term", "CTX-031"),
)

DIAGNOSTIC_ONLY_TERMS = (
    ("CGS", "business_term", None, "会前上下文没有该词，不能作为严格正向目标"),
    ("AI", "business_term", None, "会前上下文没有该词，不能作为严格正向目标"),
    ("H5", "business_term", "CTX-028", "封存窗口真人真值中未出现"),
)

EXPECTED_HASHES = {
    "human_verbatim": "7b81c3d80043bd2cfa85ee892058bc4e432f3ba67dc18fd92ff0a0b8d97dd4fc",
    "human_review": "01a51256fd4a19690270d7d6a78e9608ff60dab36bcc25676db2ec02aba3aeb4",
    "pre_meeting_context": "ded0e66a52f09b031a4f21af6a0119ff7d1ea0bcd238968a1161cc871fd0d7cd",
    "formal_whisper_full": "2ded7891c5bfbfe60c1c83c1997a381986a9165a3b4e0b633fce31158faecb59",
    "moss_p1_private": "dae289e77ffb9a1a48ccc9d49da10305529565cfbe79e968e380739f9d73f113",
}


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


def write_text(path: Path, text: str) -> None:
    path.write_text(text.rstrip() + "\n", encoding="utf-8")


def parse_bool(value: Any) -> bool:
    return str(value).strip().casefold() == "true"


def read_scored_truth_rows(path: Path) -> list[dict[str, Any]]:
    with path.open("r", encoding="utf-8-sig", newline="") as handle:
        rows = list(csv.DictReader(handle, delimiter="\t"))
    scored = [
        row
        for row in rows
        if parse_bool(row.get("valid_for_scoring"))
        and not parse_bool(row.get("non_speech"))
    ]
    scored.sort(key=lambda item: int(item["start_ms"]))
    return scored


def find_exact_casefold_spans(text: str, term: str) -> list[tuple[int, int]]:
    """Return original character spans for NFKC/casefold-equivalent exact text.

    The frozen positive terms are ASCII and the human truth already stores their
    canonical spellings.  Requiring equal normalized character length keeps the
    recorded offsets tied to the original TSV instead of a rewritten transcript.
    """

    normalized_term = unicodedata.normalize("NFKC", term).casefold()
    spans: list[tuple[int, int]] = []
    width = len(term)
    for start in range(0, max(0, len(text) - width + 1)):
        end = start + width
        candidate = text[start:end]
        if unicodedata.normalize("NFKC", candidate).casefold() == normalized_term:
            spans.append((start, end))
    return spans


def normalize_compact(value: str) -> str:
    normalized = unicodedata.normalize("NFKC", str(value)).casefold()
    return "".join(
        character
        for character in normalized
        if not unicodedata.category(character).startswith(("P", "Z"))
        and not character.isspace()
    )


def term_occurrences_in_segments(segments: Iterable[str], term: str) -> int:
    compact_term = normalize_compact(term)
    if not compact_term:
        return 0

    def boundary(character: str, before: bool) -> str:
        if character.isascii() and character.isalnum():
            return r"(?<![0-9a-z])" if before else r"(?![0-9a-z])"
        return ""

    prefix = boundary(compact_term[0], True)
    suffix = boundary(compact_term[-1], False)
    flexible = r"[\W_]*".join(re.escape(character) for character in compact_term)
    pattern = re.compile(prefix + flexible + suffix)
    return sum(
        len(pattern.findall(unicodedata.normalize("NFKC", str(text)).casefold()))
        for text in segments
    )


def formal_whisper_segments(payload: dict[str, Any]) -> list[str]:
    raw = payload.get("segments")
    if not isinstance(raw, list):
        return []
    return [
        str(item.get("text", ""))
        for item in raw
        if isinstance(item, dict) and str(item.get("text", "")).strip()
    ]


def moss_global_turn_segments(payload: dict[str, Any]) -> list[str]:
    raw = payload.get("global_turns")
    if not isinstance(raw, list):
        return []
    return [
        str(item.get("text", ""))
        for item in raw
        if isinstance(item, dict) and str(item.get("text", "")).strip()
    ]


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Freeze the auditable P0-R local positive term truth and record the negative-term blocker."
    )
    parser.add_argument("--bundle-dir", type=Path, required=True)
    parser.add_argument("--formal-dir", type=Path, required=True)
    parser.add_argument("--f1-dir", type=Path, required=True)
    parser.add_argument("--moss-private", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    paths = {
        "human_verbatim": args.bundle_dir / "04-human-verbatim.tsv",
        "human_review": args.bundle_dir / "06-human-review.json",
        "term_candidates": args.bundle_dir / "07-term-candidates.json",
        "scoring_rules": args.bundle_dir / "08-scoring-rules.json",
        "pre_meeting_context": args.bundle_dir / "16-pre-meeting-context.json",
        "human_independent_audit": args.bundle_dir / "18-independent-formal-audit.json",
        "formal_whisper_full": args.formal_dir / "02-formal-current-whisper-full.json",
        "formal_whisper_record": args.formal_dir / "03-formal-current-whisper-run-record.json",
        "f1_manifest": args.f1_dir / "MANIFEST.json",
        "f1_lock": args.f1_dir / "01-bundle-lock.json",
        "f1_audit": args.f1_dir / "02-F1-audit.json",
        "moss_p1_private": args.moss_private,
    }
    missing = sorted(name for name, path in paths.items() if not path.is_file())
    args.out_dir.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc).isoformat()

    if missing:
        failure = {
            "schema_version": 1,
            "role": "P0R_F2_TERM_TRUTH_AUDIT",
            "created_at": now,
            "status": "FAIL",
            "missing_inputs": missing,
            "decision": "F2_FAIL_INPUTS_MISSING",
        }
        write_json(args.out_dir / "03-F2-audit.json", failure)
        print(json.dumps({"status": "FAIL", "missing": missing}, ensure_ascii=False))
        return 2

    hashes = {name: sha256_file(path) for name, path in paths.items()}
    expected_hash_checks = {
        name: hashes[name].casefold() == expected.casefold()
        for name, expected in EXPECTED_HASHES.items()
    }

    review = load_json(paths["human_review"])
    independent = load_json(paths["human_independent_audit"])
    context = load_json(paths["pre_meeting_context"])
    formal = load_json(paths["formal_whisper_full"])
    formal_record = load_json(paths["formal_whisper_record"])
    moss = load_json(paths["moss_p1_private"])
    f1_lock = load_json(paths["f1_lock"])
    f1_audit = load_json(paths["f1_audit"])
    rules = load_json(paths["scoring_rules"])
    truth_rows = read_scored_truth_rows(paths["human_verbatim"])
    truth_segments = [str(row.get("verbatim_text", "")) for row in truth_rows]
    whisper_segments = formal_whisper_segments(formal)
    moss_segments = moss_global_turn_segments(moss)
    context_entries = {
        str(item.get("entry_id")): item
        for item in context.get("entries", [])
        if isinstance(item, dict)
    }

    positive_items: list[dict[str, Any]] = []
    positive_errors: list[str] = []
    for term, term_type, context_id in POSITIVE_TERMS:
        entry = context_entries.get(context_id)
        if not isinstance(entry, dict):
            positive_errors.append(f"{term}:missing_context_entry")
            continue
        if entry.get("term") != term or entry.get("type") != term_type:
            positive_errors.append(f"{term}:context_entry_mismatch")
            continue
        occurrences: list[dict[str, Any]] = []
        for segment_index, (row, text_value) in enumerate(zip(truth_rows, truth_segments)):
            for start_char, end_char in find_exact_casefold_spans(text_value, term):
                occurrences.append(
                    {
                        "segment_index": segment_index,
                        "segment_id": row.get("segment_id"),
                        "start_char": start_char,
                        "end_char": end_char,
                        "segment_start_ms": int(row["start_ms"]),
                        "segment_end_ms": int(row["end_ms"]),
                    }
                )
        if not occurrences:
            positive_errors.append(f"{term}:not_found_in_scored_human_truth")
            continue
        positive_items.append(
            {
                "term": term,
                "type": term_type,
                "context_entry_id": context_id,
                "expected_occurrences": len(occurrences),
                "human_confirmed_in_window": True,
                "reference_occurrences": occurrences,
                "confirmation_basis": (
                    "已封存真人逐字稿原文 + 06-human-review.json 中 "
                    "business_terms_checked=true、approved_as_ground_truth=true"
                ),
            }
        )

    negative_candidates: list[dict[str, Any]] = []
    for term, term_type, context_id in NEGATIVE_REVIEW_CANDIDATES:
        entry = context_entries.get(context_id)
        context_matches = bool(
            isinstance(entry, dict)
            and entry.get("term") == term
            and entry.get("type") == term_type
        )
        negative_candidates.append(
            {
                "term": term,
                "type": term_type,
                "context_entry_id": context_id,
                "context_entry_matches": context_matches,
                "human_truth_window_occurrences": term_occurrences_in_segments(
                    truth_segments, term
                ),
                "formal_whisper_window_occurrences": term_occurrences_in_segments(
                    whisper_segments, term
                ),
                "moss_full_output_occurrences": term_occurrences_in_segments(
                    moss_segments, term
                ),
                "human_confirmed_unspoken_full_audio": False,
                "status": "MACHINE_SEARCH_COMPLETE_HUMAN_FULL_AUDIO_ATTESTATION_MISSING",
            }
        )

    diagnostic_items = []
    for term, term_type, context_id, reason in DIAGNOSTIC_ONLY_TERMS:
        diagnostic_items.append(
            {
                "term": term,
                "type": term_type,
                "context_entry_id": context_id,
                "human_truth_window_occurrences": term_occurrences_in_segments(
                    truth_segments, term
                ),
                "formal_whisper_window_occurrences": term_occurrences_in_segments(
                    whisper_segments, term
                ),
                "moss_full_output_occurrences": term_occurrences_in_segments(
                    moss_segments, term
                ),
                "eligible_for_strict_positive": False,
                "reason": reason,
            }
        )

    term_rules = rules.get("term_scoring", {})
    positive_minimum = int(term_rules.get("minimum_positive_targets", 3))
    negative_minimum = int(term_rules.get("minimum_negative_targets", 2))
    positive_types = {item["type"] for item in positive_items}
    negative_types = {item["type"] for item in negative_candidates}
    formal_finished_at = formal_record.get("finished_at")

    positive_payload = {
        "schema_version": 1,
        "role": "P0R_LOCAL_POSITIVE_TERM_TRUTH",
        "scope": "LOCAL_P0R_BALANCED_226_440_SECOND_WINDOW",
        "status": "HUMAN_TRUTH_DERIVED_POSITIVE_FROZEN",
        "frozen_at": now,
        "frozen_after_formal_whisper_finished_at": formal_finished_at,
        "confirmed_by": review.get("reviewer_id"),
        "confirmed_role": review.get("reviewer_role"),
        "positive_spoken_terms": positive_items,
        "source_bindings": {
            "human_verbatim_sha256": hashes["human_verbatim"],
            "human_review_sha256": hashes["human_review"],
            "pre_meeting_context_sha256": hashes["pre_meeting_context"],
            "scoring_rules_sha256": hashes["scoring_rules"],
            "formal_whisper_full_sha256": hashes["formal_whisper_full"],
            "f1_manifest_sha256": hashes["f1_manifest"],
            "f1_lock_sha256": hashes["f1_lock"],
        },
        "truth_boundary": (
            "只冻结封存真人逐字稿中可机械定位、且在录音开始前上下文已存在的词；"
            "没有把机器候选直接当成人工真值。"
        ),
    }
    write_json(args.out_dir / "01-local-positive-terms-frozen.json", positive_payload)
    positive_payload_sha = sha256_file(
        args.out_dir / "01-local-positive-terms-frozen.json"
    )

    negative_payload = {
        "schema_version": 1,
        "role": "P0R_NEGATIVE_TERM_REVIEW_BLOCKER",
        "created_at": now,
        "status": "BLOCKED_HUMAN_FULL_AUDIO_NEGATIVE_ATTESTATION_MISSING",
        "negative_review_candidates": negative_candidates,
        "required_minimum": negative_minimum,
        "machine_search_completed": True,
        "machine_search_is_not_human_confirmation": True,
        "required_review_method": term_rules.get(
            "required_full_audio_review_method"
        ),
        "required_minimum_review_seconds": term_rules.get(
            "minimum_full_audio_review_elapsed_seconds"
        ),
        "required_attestation_exact_text": term_rules.get(
            "required_negative_attestation"
        ),
        "current_human_attestation_present": False,
        "negative_unspoken_terms": [],
        "source_bindings": {
            "full_audio_sha256": "82829a5d2011109f69c6526c6e87afd767b18f9afdfff87b93260af5073382bb",
            "window_audio_sha256": str(review.get("audio_sha256", "")).casefold(),
            "human_verbatim_sha256": hashes["human_verbatim"],
            "human_review_sha256": hashes["human_review"],
            "formal_whisper_full_sha256": hashes["formal_whisper_full"],
            "moss_p1_private_sha256": hashes["moss_p1_private"],
            "pre_meeting_context_sha256": hashes["pre_meeting_context"],
            "scoring_rules_sha256": hashes["scoring_rules"],
        },
        "truth_boundary": (
            "机器全文检索只生成候选和计数，不能证明录音中没有说出；"
            "在真人按固定声明确认前，不生成正式负向术语。"
        ),
    }
    write_json(args.out_dir / "02-negative-term-review-blocker.json", negative_payload)

    checks: dict[str, bool] = {
        "all_expected_input_hashes_match": all(expected_hash_checks.values()),
        "human_review_approved": review.get("approved_as_ground_truth") is True,
        "human_review_business_terms_checked": review.get("business_terms_checked")
        is True,
        "human_review_playback_complete": review.get("playback_coverage", {}).get(
            "complete"
        )
        is True,
        "independent_human_audit_pass": independent.get("structural_status")
        == "PASS",
        "f1_audit_pass": f1_audit.get("status") == "PASS",
        "f1_local_lane_allows_after_f2": f1_lock.get(
            "local_p0r_scoring_lane", {}
        ).get("allowed_after_f2_terms")
        is True,
        "formal_whisper_has_segments": len(whisper_segments) > 0,
        "moss_has_global_turns": len(moss_segments) > 0,
        "positive_terms_meet_minimum": len(positive_items) >= positive_minimum,
        "positive_terms_have_no_derivation_errors": not positive_errors,
        "positive_occurrences_all_nonzero": all(
            item["expected_occurrences"] >= 1 for item in positive_items
        ),
        "positive_context_entries_match": all(
            context_entries[item["context_entry_id"]].get("term") == item["term"]
            and context_entries[item["context_entry_id"]].get("type")
            == item["type"]
            for item in positive_items
        ),
        "positive_types_are_business_only": positive_types == {"business_term"},
        "strict_s8_positive_person_type_missing": "person_or_alias"
        not in positive_types,
        "negative_candidate_count_meets_minimum": len(negative_candidates)
        >= negative_minimum,
        "negative_candidate_types_cover_person_and_business": {
            "person_or_alias",
            "business_term",
        }.issubset(negative_types),
        "negative_candidates_absent_from_human_window": all(
            item["human_truth_window_occurrences"] == 0
            for item in negative_candidates
        ),
        "negative_human_attestation_missing_is_recorded": all(
            item["human_confirmed_unspoken_full_audio"] is False
            for item in negative_candidates
        ),
        "negative_terms_not_falsely_frozen": negative_payload[
            "negative_unspoken_terms"
        ]
        == [],
        "local_positive_file_hash_created": len(positive_payload_sha) == 64,
    }
    failed_checks = [name for name, passed in checks.items() if not passed]
    structural_failures = [
        name
        for name in failed_checks
        if name
        not in {
            "strict_s8_positive_person_type_missing",
        }
    ]
    # strict_s8_positive_person_type_missing is deliberately expected to be true.
    # A false value would mean a person term unexpectedly appeared and requires a
    # new review rather than silently changing this frozen target set.
    structural_status = "PASS" if not structural_failures else "FAIL"
    overall_status = (
        "PARTIAL_PASS_POSITIVE_FROZEN_NEGATIVE_HUMAN_REVIEW_BLOCKED"
        if structural_status == "PASS"
        else "FAIL"
    )
    audit = {
        "schema_version": 1,
        "role": "P0R_F2_TERM_TRUTH_AUDIT",
        "created_at": now,
        "status": overall_status,
        "structural_status": structural_status,
        "checks": checks,
        "summary": {
            "check_count": len(checks),
            "pass_count": sum(checks.values()),
            "failed_count": len(failed_checks),
            "failed_checks": failed_checks,
            "positive_term_count": len(positive_items),
            "positive_occurrence_count": sum(
                item["expected_occurrences"] for item in positive_items
            ),
            "negative_candidate_count": len(negative_candidates),
            "formal_whisper_segment_count": len(whisper_segments),
            "moss_global_turn_count": len(moss_segments),
        },
        "expected_hash_checks": expected_hash_checks,
        "positive_derivation_errors": positive_errors,
        "diagnostic_only_terms": diagnostic_items,
        "outputs": {
            "positive_terms_path": str(
                (args.out_dir / "01-local-positive-terms-frozen.json").resolve()
            ),
            "positive_terms_sha256": positive_payload_sha,
            "negative_blocker_path": str(
                (args.out_dir / "02-negative-term-review-blocker.json").resolve()
            ),
            "negative_blocker_sha256": sha256_file(
                args.out_dir / "02-negative-term-review-blocker.json"
            ),
        },
        "local_p0r_decision": (
            "ALLOW_F3_CER_AND_SPEAKER_SCORING_WITH_OVERALL_NO_GO"
            if structural_status == "PASS"
            else "STOP_BEFORE_F3"
        ),
        "strict_s8_decision": "BLOCKED",
        "strict_s8_blockers": [
            "封存窗口没有可用的会前人员/别名正向词，严格规则要求正向同时含人员和业务术语。",
            "负向术语缺少完整 737.728 秒录音的专门人工未说出声明。",
            "现有 MOSS 是 schema_version=1、stage=MOSS_V3_P1_CHUNKED，不是严格 CUDA 评分要求的 LOCKED_FULL_MOSS_OUTPUT schema_version=2。",
        ],
    }
    write_json(args.out_dir / "03-F2-audit.json", audit)
    audit_sha = sha256_file(args.out_dir / "03-F2-audit.json")

    conclusion = f"""# F2 术语真值门禁结论

- F2 状态：`{overall_status}`
- 结构审计：`{structural_status}`（{sum(checks.values())}/{len(checks)} 项为真）
- 已冻结正向术语：`YouTube`、`PWA`、`Google`
- 正向术语总出现次数：`{sum(item['expected_occurrences'] for item in positive_items)}`
- 负向候选：`Karl`、`A/B Test`
- 正式负向术语：`0` 个，原因是缺少完整 737.728 秒录音的专门人工未说出声明
- 本地下一步：允许执行 F3 的 CER、说话人和正向术语评分，但总门禁必须保持 `NO-GO`
- 严格 S8：继续 `BLOCKED`，不能把本地 P1 输出改名冒充 CUDA 正式输出
- F2 审计 SHA-256：`{audit_sha}`

## 证据边界

正向词来自已经封存并由 lili 确认的真人逐字稿，程序只机械计算字符位置和次数。负向候选的机器搜索结果不能代替真人签字，因此没有生成正式负向术语，也没有把 F2 写成完全通过。
"""
    write_text(args.out_dir / "04-F2结论.md", conclusion)

    print(
        json.dumps(
            {
                "status": overall_status,
                "structural_status": structural_status,
                "positive_terms": [item["term"] for item in positive_items],
                "positive_occurrences": {
                    item["term"]: item["expected_occurrences"]
                    for item in positive_items
                },
                "negative_candidates": [
                    {
                        "term": item["term"],
                        "human_window": item["human_truth_window_occurrences"],
                        "whisper_window": item[
                            "formal_whisper_window_occurrences"
                        ],
                        "moss_full": item["moss_full_output_occurrences"],
                    }
                    for item in negative_candidates
                ],
                "audit_sha256": audit_sha,
            },
            ensure_ascii=False,
        )
    )
    return 0 if structural_status == "PASS" else 2


if __name__ == "__main__":
    sys.exit(main())
