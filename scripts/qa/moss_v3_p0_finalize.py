#!/usr/bin/env python3
"""Finalize the split P0-D/P0-R verdict without inventing human ground truth."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
from typing import Any


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def json_read(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8-sig"))


def file_record(path: Path, *, root: Path | None = None) -> dict[str, Any]:
    return {
        "path": str(path),
        "relative_path": path.relative_to(root).as_posix() if root else None,
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()

    repo = args.repo.resolve()
    evidence = args.evidence_dir.resolve()
    old_truth = (
        repo
        / "target"
        / "release"
        / "docs"
        / "方案"
        / "证据"
        / "MOSS-S8-M00-R3-20260828-110430"
    )
    ui_audit = (
        repo
        / "target"
        / "release"
        / "docs"
        / "方案"
        / "证据"
        / "RUN-20260828-UAT-CLEAR-15MIN-MONTHLY"
        / "清晰音频转录基线审计.md"
    )

    required_evidence = [
        evidence / "00-source-state.json",
        evidence / "01-input-manifest.json",
        evidence / "02-environment.json",
        evidence / "03-source-tests.json",
        evidence / "05-data-integrity.json",
    ]
    truth_names = [
        "17-human-speaker-identity-anchors.json",
        "23-human-speaker-sample-mapping.json",
        "26-human-term-confirmations.json",
        "32-human-package-name-confirmation.json",
        "41-version-number-unresolvable-exclusion.json",
        "04-human-verbatim.tsv",
        "05-human-speaker-turns.tsv",
        "06-human-review.json",
        "07-hotwords-frozen.json",
    ]
    required = [*required_evidence, *(old_truth / name for name in truth_names), ui_audit]
    missing = [str(path) for path in required if not path.is_file()]
    if missing:
        raise FileNotFoundError(f"P0 evidence is incomplete: {missing}")

    source = json_read(evidence / "00-source-state.json")
    inputs = json_read(evidence / "01-input-manifest.json")
    tests = json_read(evidence / "03-source-tests.json")
    integrity = json_read(evidence / "05-data-integrity.json")
    human_review = json_read(old_truth / "06-human-review.json")
    hotwords = json_read(old_truth / "07-hotwords-frozen.json")
    verbatim_lines = (old_truth / "04-human-verbatim.tsv").read_text(encoding="utf-8-sig").splitlines()
    turns_lines = (old_truth / "05-human-speaker-turns.tsv").read_text(encoding="utf-8-sig").splitlines()

    truth_payload = {
        "schema_version": 1,
        "scope": "MOSS-V3-P0-HUMAN-TRUTH-INVENTORY",
        "status": "PARTIAL_HUMAN_CONFIRMATION_NOT_FULL_GROUND_TRUTH",
        "confirmed_evidence": [
            file_record(old_truth / name)
            for name in [
                "17-human-speaker-identity-anchors.json",
                "23-human-speaker-sample-mapping.json",
                "26-human-term-confirmations.json",
                "32-human-package-name-confirmation.json",
                "41-version-number-unresolvable-exclusion.json",
            ]
        ],
        "full_ground_truth": {
            "verbatim_data_rows": max(0, len(verbatim_lines) - 1),
            "speaker_turn_data_rows": max(0, len(turns_lines) - 1),
            "review_status": human_review.get("review_status"),
            "approved_as_ground_truth": human_review.get("approved_as_ground_truth"),
            "positive_term_count": len(hotwords.get("positive_spoken_terms", [])),
            "negative_term_count": len(hotwords.get("negative_unspoken_terms", [])),
            "negative_scope_confirmed_full_audio": hotwords.get(
                "negative_scope_confirmed_full_audio"
            ),
        },
        "unscorable_metrics": [
            "737.728s full-audio CER",
            "737.728s full speaker-turn error rate",
            "737.728s full speaker-duration error rate",
            "negative-term false insertion rate",
        ],
        "rule": "Machine output, model summaries, candidate timelines and business context are not human ground truth.",
    }
    truth_path = evidence / "04-human-truth-inventory.json"
    truth_path.write_text(json.dumps(truth_payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    p0d_checks = {
        "baseline_commit_frozen": source.get("baseline_commit")
        == "cb98b28a0f466e4a508a2199c11fedeef552dfdc",
        "tracked_worktree_clean_at_freeze": source.get("tracked_dirty_count") == 0,
        "six_inputs_hashed": inputs.get("input_count") == 6
        and all(item.get("sha256") for item in inputs.get("inputs", [])),
        "source_regression_passed": tests.get("status") == "PASS",
        "database_online_backup_integrity_ok": integrity.get("database", {}).get("integrity_check")
        == "ok",
        "release_files_hashed": len(integrity.get("release_files", [])) == 5
        and all(item.get("sha256") for item in integrity.get("release_files", [])),
        "settings_hashed": len(integrity.get("settings", [])) == 4,
        "existing_models_hashed": integrity.get("models", {}).get("file_count") == 9,
        "current_release_ui_baseline_recorded": "AFD71F6CB15435D7116CB45E81769DD8EA5FC61BB39E84605C7CF6CF8F201B44"
        in ui_audit.read_text(encoding="utf-8-sig"),
        "truth_boundary_explicit": truth_payload["status"]
        == "PARTIAL_HUMAN_CONFIRMATION_NOT_FULL_GROUND_TRUTH",
    }
    p0r_checks = {
        "full_verbatim_present": truth_payload["full_ground_truth"]["verbatim_data_rows"] > 0,
        "full_speaker_turns_present": truth_payload["full_ground_truth"][
            "speaker_turn_data_rows"
        ]
        > 0,
        "human_review_approved": truth_payload["full_ground_truth"]["approved_as_ground_truth"]
        is True,
        "negative_term_scope_confirmed": truth_payload["full_ground_truth"][
            "negative_scope_confirmed_full_audio"
        ]
        is True,
    }
    verdict = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0",
        "p0_development_baseline": {
            "status": "PASS" if all(p0d_checks.values()) else "FAIL",
            "checks": p0d_checks,
        },
        "p0_release_ground_truth": {
            "status": "PASS" if all(p0r_checks.values()) else "BLOCKED",
            "checks": p0r_checks,
            "blocker": "A real listener has not completed and approved the full 737.728-second reference.",
        },
        "next_action": "P1 runtime/performance gate is allowed. L2 accuracy and release claims remain blocked.",
        "ui_regression_boundary": {
            "evidence": file_record(ui_audit),
            "passed": [
                "application launch",
                "15-minute import",
                "Whisper persistence",
                "template selection",
                "Qwen 3.5 2B summary completion",
                "needs-review fact protection",
                "interrupted meeting recovery",
            ],
            "not_yet_release_verified": [
                "AFD build live recording bubbles",
                "AFD build stop convergence",
                "AFD build transcript edit-save-refresh-reopen",
                "single uninterrupted recording-to-summary end-to-end run",
            ],
        },
    }
    verdict_path = evidence / "06-p0-verdict.json"
    verdict_path.write_text(json.dumps(verdict, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    manifest_files = sorted(
        path
        for path in evidence.iterdir()
        if path.is_file() and path.name != "MANIFEST.json"
    )
    manifest = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0",
        "p0_d_status": verdict["p0_development_baseline"]["status"],
        "p0_r_status": verdict["p0_release_ground_truth"]["status"],
        "files": [file_record(path, root=evidence) for path in manifest_files],
    }
    manifest_path = evidence / "MANIFEST.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(
        json.dumps(
            {
                "p0_d": manifest["p0_d_status"],
                "p0_r": manifest["p0_r_status"],
                "manifest": str(manifest_path),
            },
            ensure_ascii=False,
        )
    )
    return 0 if manifest["p0_d_status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
