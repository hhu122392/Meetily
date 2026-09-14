#!/usr/bin/env python3
"""Focused standard-library tests for the one-shot formal Q00 runner."""

from __future__ import annotations

import csv
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


QA_DIR = Path(__file__).resolve().parent
REPO = QA_DIR.parents[1]
if str(QA_DIR) not in sys.path:
    sys.path.insert(0, str(QA_DIR))

import moss_functional_fix_quality_gate as GATE  # noqa: E402
import moss_functional_fix_q00_formal as FORMAL  # noqa: E402


COMMIT = "a" * 40
TERMS = ["M100", "YouTube", "PWA", "H5", "Google", "VIP", "A/B Test", "TG"]
MOSS_DECODE_PARAMETERS_JSON = '{"language":"zh","timestamps":"segment","diarize":"on"}'


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def write_speaker_truth(path: Path) -> None:
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
        writer.writerow(GATE.SPEAKER_TRUTH_HEADER)
        writer.writerow(["T01", "0", "1000", "H01", "false", "true", "test"])
        writer.writerow(["T02", "1000", "2000", "H04", "false", "true", "test"])


class FormalQ00RunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = Path(tempfile.mkdtemp(prefix="q00-formal-test-"))

    def tearDown(self) -> None:
        shutil.rmtree(self.temp, ignore_errors=True)

    def moss_native_decode_proof(self) -> tuple[dict[str, object], dict[str, object]]:
        digest = GATE.sha256_bytes(MOSS_DECODE_PARAMETERS_JSON.encode("utf-8"))
        proof = {
            "source": "api_moss_get_workspace.runs",
            "language_requested": "zh-CN",
            "language_resolved": "zh-CN",
            "decode_parameters_json": MOSS_DECODE_PARAMETERS_JSON,
            "decode_parameters_sha256": digest,
            "recomputed_decode_parameters_sha256": digest,
            "api_fields": {
                "languageRequested": "zh-CN",
                "languageResolved": "zh-CN",
                "decodeParametersJson": MOSS_DECODE_PARAMETERS_JSON,
                "decodeParametersSha256": digest,
            },
            "passed": True,
        }
        persisted = {
            "language_requested": "zh-CN",
            "language_resolved": "zh-CN",
            "decode_parameters_json": MOSS_DECODE_PARAMETERS_JSON,
            "decode_parameters_sha256": digest.lower(),
        }
        return proof, persisted

    def test_frozen_truth_manifest_has_exact_flat_contract(self) -> None:
        path = REPO / "target" / "release" / "docs" / "方案" / "MOSS功能修复计划-20260902" / "Q00-FROZEN-TRUTH-MANIFEST.json"
        self.assertEqual(
            GATE._safe_file_record(path),
            {"bytes": GATE.FROZEN_TRUTH_MANIFEST_BYTES, "sha256": GATE.FROZEN_TRUTH_MANIFEST_SHA256},
        )
        document = GATE.require_mapping(GATE.read_json(path, "frozen truth manifest"), "frozen truth manifest")
        files = GATE.require_mapping(document.get("files"), "frozen truth files")
        self.assertEqual(
            set(files),
            {
                "truth_package_manifest",
                "human_verbatim",
                "speaker_truth",
                "human_review",
                "derived_review_provenance",
                "pre_meeting_context",
                "positive_truth",
                "negative_truth",
            },
        )
        for role, raw in files.items():
            row = GATE.require_mapping(raw, f"{role} truth row")
            relative = GATE.require_string(row.get("relative_path"), f"{role} relative path")
            self.assertEqual(relative, Path(relative).name)
            self.assertNotIn("..", Path(relative).parts)
        negative = REPO / "target" / "release" / "docs" / "方案" / "MOSS功能修复计划-20260902" / "Q00-NEGATIVE-TRUTH.json"
        self.assertEqual(
            GATE._safe_file_record(negative),
            {"bytes": GATE.FROZEN_NEGATIVE_TRUTH_BYTES, "sha256": GATE.FROZEN_NEGATIVE_TRUTH_SHA256},
        )

    def test_frozen_truth_checkout_filter_preserves_exact_bytes(self) -> None:
        cases = {
            "target/release/docs/方案/MOSS功能修复计划-20260902/Q00-FROZEN-TRUTH-MANIFEST.json": {
                "bytes": GATE.FROZEN_TRUTH_MANIFEST_BYTES,
                "sha256": GATE.FROZEN_TRUTH_MANIFEST_SHA256,
            },
            "target/release/docs/方案/MOSS功能修复计划-20260902/Q00-NEGATIVE-TRUTH.json": {
                "bytes": GATE.FROZEN_NEGATIVE_TRUTH_BYTES,
                "sha256": GATE.FROZEN_NEGATIVE_TRUTH_SHA256,
            },
        }
        for relative_path, expected in cases.items():
            with self.subTest(relative_path=relative_path):
                attribute = subprocess.run(
                    ["git", "check-attr", "text", "--", relative_path],
                    cwd=REPO,
                    check=True,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                ).stdout.strip()
                self.assertTrue(attribute.endswith(": text: unset"), attribute)
                checkout_bytes = subprocess.check_output(
                    [
                        "git",
                        "cat-file",
                        "--filters",
                        f"--path={relative_path}",
                        f"HEAD:{relative_path}",
                    ],
                    cwd=REPO,
                )
                self.assertEqual(
                    {"bytes": len(checkout_bytes), "sha256": GATE.sha256_bytes(checkout_bytes)},
                    expected,
                )

    def test_product_context_is_canonical_and_bound_before_moss(self) -> None:
        metadata = self.temp / "metadata.json"
        context = self.temp / "pre-meeting.json"
        speaker = self.temp / "speaker.tsv"
        write_json(metadata, {"id": "meeting-1", "transcripts": []})
        write_json(context, {"entries": [{"type": "business_term", "term": term} for term in TERMS]})
        write_speaker_truth(speaker)

        result = FORMAL._attach_product_context(
            metadata_path=metadata,
            premeeting_context_path=context,
            speaker_truth_path=speaker,
            run_id="Q00-aaaaaaaaaaaa-0123456789abcdef0123456789abcdef",
        )
        snapshot = result["snapshot"]
        self.assertEqual(snapshot["context_sha256"], GATE._meeting_context_snapshot_sha256(snapshot))
        self.assertEqual([item["canonical"] for item in snapshot["terms"]], TERMS)
        self.assertEqual([item["person_id"] for item in snapshot["people"]], ["H01", "H04"])
        self.assertTrue(all(item["attendance"] == "attending" for item in snapshot["people"]))
        changed = dict(snapshot)
        changed["reason"] = "changed-after-hash"
        self.assertNotEqual(snapshot["context_sha256"], GATE._meeting_context_snapshot_sha256(changed))

    def test_speaker_mapping_plans_a_real_wrong_then_correct_override(self) -> None:
        speaker = self.temp / "speaker.tsv"
        write_speaker_truth(speaker)
        candidate = [
            {"segmentId": "segment-1", "startMs": 0, "endMs": 1000, "speakerLabel": "SPEAKER_00"},
            {"segmentId": "segment-2", "startMs": 1000, "endMs": 2000, "speakerLabel": "SPEAKER_01"},
        ]
        bindings, segment_id, wrong_person_id, correct_person_id = FORMAL._speaker_bindings(
            candidate, speaker
        )
        self.assertEqual(
            bindings,
            [
                {"speaker_label": "SPEAKER_00", "person_id": "H01"},
                {"speaker_label": "SPEAKER_01", "person_id": "H04"},
            ],
        )
        self.assertEqual(
            (segment_id, wrong_person_id, correct_person_id),
            ("segment-1", "H04", "H01"),
        )
        self.assertNotEqual(wrong_person_id, correct_person_id)

    def test_persisted_override_history_proves_a_real_true_flip(self) -> None:
        truth = [
            {"turn_id": "T01", "start": 0.0, "end": 1.0, "speaker": "H01"},
            {"turn_id": "T02", "start": 1.0, "end": 2.0, "speaker": "H04"},
        ]
        history = [
            {
                "override_id": "wrong-1",
                "person_id": "H04",
                "reason_code": "MANUAL_PERSON_OVERRIDE",
                "revoked_at": "2026-09-05T00:00:00Z",
            },
            {
                "override_id": "correct-2",
                "person_id": "H01",
                "reason_code": "MANUAL_PERSON_OVERRIDE",
                "revoked_at": None,
            },
        ]
        audit = FORMAL._speaker_override_audit(
            history=history,
            active_override=history[1],
            segment={"segment_id": "segment-1", "start_ms": 0, "end_ms": 1000},
            segment_index=0,
            speaker_truth=truth,
        )
        self.assertEqual(audit["before_speaker_id"], "H04")
        self.assertEqual(audit["after_speaker_id"], "H01")
        self.assertEqual(audit["reference_speaker_id"], "H01")

    def test_persisted_noop_override_history_is_rejected(self) -> None:
        history = [
            {
                "override_id": "first-1",
                "person_id": "H01",
                "reason_code": "MANUAL_PERSON_OVERRIDE",
                "revoked_at": "2026-09-05T00:00:00Z",
            },
            {
                "override_id": "second-2",
                "person_id": "H01",
                "reason_code": "MANUAL_PERSON_OVERRIDE",
                "revoked_at": None,
            },
        ]
        with self.assertRaisesRegex(GATE.QualityGateError, "wrong-to-correct flip"):
            FORMAL._speaker_override_audit(
                history=history,
                active_override=history[1],
                segment={"segment_id": "segment-1", "start_ms": 0, "end_ms": 1000},
                segment_index=0,
                speaker_truth=[
                    {"turn_id": "T01", "start": 0.0, "end": 1.0, "speaker": "H01"}
                ],
            )

    def test_formal_session_records_real_runner_timing_and_cleanup(self) -> None:
        private_root = self.temp / "private"
        public_root = self.temp / "public"
        private_root.mkdir()
        public_root.mkdir()
        clean_status = {
            "format": "git-status-porcelain-v1-z-with-all-untracked",
            "entry_count": 0,
            "raw_sha256": GATE.sha256_bytes(b""),
        }
        with mock.patch.object(GATE, "_require_current_commit", return_value=COMMIT), mock.patch.object(
            GATE, "_repository_status_record", return_value=clean_status
        ):
            session = GATE.begin_formal_session(
                repo=REPO,
                source_commit=COMMIT,
                private_root=private_root,
                public_root=public_root,
                formal_runner_path=REPO / "scripts" / "qa" / "moss_functional_fix_q00_formal.py",
                node_path=Path(sys.executable),
                cdp_script_path=REPO / "scripts" / "qa" / "moss-functional-ft-cdp.mjs",
            )
            marker = GATE.mark_moss_start(session_path=Path(session["session_path"]))
            run_directory = Path(session["run_directory"])
            state = run_directory / "state.json"
            output = run_directory / "output.json"
            request = run_directory / "request.json"
            for path in (state, output, request):
                path.write_text("{}\n", encoding="utf-8")
            app = self.temp / "candidate.exe"
            app.write_bytes(b"candidate")
            app_record = {"pid": 1234, "executable_path": str(app), **GATE._safe_file_record(app)}
            cleanup = {
                "completed": True,
                "consecutive_zero_scans": 2,
                "residual_processes": [],
                "cdp_listener_closed": True,
            }
            receipt = GATE.finish_formal_session(
                repo=REPO,
                source_commit=COMMIT,
                session_path=Path(session["session_path"]),
                marker_path=Path(marker["marker_path"]),
                moss_completed_monotonic_ns=marker["started_monotonic_ns"] + 1_000_000_000,
                runner_exit_code=0,
                timed_out=False,
                runner_arguments=[
                    str(REPO / "scripts" / "qa" / "moss-functional-ft-cdp.mjs"),
                    "moss-complete",
                    str(state),
                    str(output),
                    str(request),
                ],
                runner_cwd=REPO,
                app_process=app_record,
                cleanup=cleanup,
            )
        self.assertEqual(receipt["status"], "COMPLETED")
        self.assertEqual(receipt["runner_exit_code"], 0)
        self.assertEqual(receipt["timing"]["moss_elapsed_seconds"], 1.0)
        self.assertEqual(receipt["app_process"]["bytes"], len(b"candidate"))

    def test_formal_session_rejects_zero_byte_app_record(self) -> None:
        app = self.temp / "candidate.exe"
        app.write_bytes(b"candidate")
        record = {"pid": 1, "executable_path": str(app), "bytes": 0, "sha256": GATE._safe_file_record(app)["sha256"]}
        with self.assertRaisesRegex(GATE.QualityGateError, "app process record"):
            GATE._validated_app_process_record(record)

    def test_failed_setup_runs_exact_cleanup(self) -> None:
        local = self.temp / "local"
        roaming = self.temp / "roaming"
        local.mkdir()
        roaming.mkdir()
        manifest = {"installer": {"sha256": "A" * 64}, "installed_files": [], "version": "1.0.0"}
        failed_run = {"timed_out": False, "exit_code": 9}
        cleanup = {"status": "PASS", "errors": []}
        with mock.patch.dict(os.environ, {"LOCALAPPDATA": str(local), "APPDATA": str(roaming)}), mock.patch.object(
            FORMAL, "_registry_state", return_value=None
        ), mock.patch.object(FORMAL, "_run_process", return_value=failed_run), mock.patch.object(
            FORMAL, "_cleanup_install", return_value=cleanup
        ) as cleanup_call:
            with self.assertRaisesRegex(GATE.QualityGateError, "silent installation failed"):
                FORMAL._prepare_isolated_install(
                    installer=Path(sys.executable),
                    manifest=manifest,
                    models={},
                    runtime_root=self.temp,
                    run_id="Q00-test",
                    source_commit=COMMIT,
                )
        cleanup_call.assert_called_once()
        partial = cleanup_call.call_args.args[0]
        self.assertEqual(partial["install_root"], local / GATE.APPROVED_PRODUCT_NAME)
        self.assertEqual(partial["install_run"], failed_run)

    def test_cleanup_refuses_unowned_data_root(self) -> None:
        data_root = self.temp / "unowned-data"
        data_root.mkdir()
        install = {
            "install_root": self.temp / "absent-install",
            "data_root": data_root,
            "webview_root": self.temp / "absent-webview",
            "backup_root": self.temp / "absent-backup",
            "owner": {"run_id": "not-on-disk"},
        }
        result = FORMAL._cleanup_install(install)
        self.assertEqual(result["status"], "FAIL")
        self.assertTrue(data_root.is_dir())
        self.assertIn("data_root owner marker mismatch; not removed", result["errors"])

    def test_formal_runner_uses_fixed_product_actions_and_immutable_snapshot(self) -> None:
        source = (QA_DIR / "moss_functional_fix_q00_formal.py").read_text(encoding="utf-8")
        for action in (
            'action="bootstrap"',
            'action="import-audio"',
            'action="moss-complete"',
            'action="moss-review-q00"',
            'action="moss-activate"',
            'action="summary-generate"',
        ):
            self.assertIn(action, source)
        self.assertIn('label="04a-moss-review-wrong-speaker"', source)
        self.assertIn('label="04b-moss-review-correct-speaker"', source)
        self.assertNotIn("bootstrap_browser.get(\"primaryLanguage\")", source)
        self.assertNotIn('"moss_language_resolved": bootstrap', source)
        self.assertIn('moss["document"].get("native_decode_contract")', source)
        self.assertIn('"moss_native_decode_contract": native_decode_contract', source)
        self.assertIn('"language_requested": moss_native_decode["language_requested"]', source)
        self.assertIn('"decode_parameters_json": moss_native_decode["decode_parameters_json"]', source)
        self.assertIn("whisper_request.get(\"language\")", source)
        self.assertIn('str(run_row["audio_sha256"]).upper()', source)
        self.assertIn('str(run_row["model_sha256"]).upper()', source)
        self.assertIn('gate._safe_file_record(models["whisper"])', source)
        self.assertIn("?mode=ro&immutable=1", source)
        self.assertIn('source_sidecars_absent": True', source)
        parser_options = {action.dest for action in FORMAL.build_parser()._actions}
        self.assertNotIn("producer", parser_options)
        self.assertNotIn("runner", parser_options)

    def test_formal_native_decode_proof_accepts_only_persisted_api_fields(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        actual = FORMAL._validated_moss_native_decode_contract(proof, persisted)
        self.assertEqual(
            actual,
            {
                "source": "api_moss_get_workspace.runs",
                "language_requested": "zh-CN",
                "language_resolved": "zh-CN",
                "decode_parameters_json": MOSS_DECODE_PARAMETERS_JSON,
                "decode_parameters_sha256": GATE.sha256_bytes(
                    MOSS_DECODE_PARAMETERS_JSON.encode("utf-8")
                ),
            },
        )

    def test_formal_native_decode_proof_rejects_missing_api_field(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        proof["api_fields"].pop("languageRequested")
        with self.assertRaisesRegex(GATE.QualityGateError, "API fields are not exact"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_formal_native_decode_proof_rejects_local_storage_source(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        proof["source"] = "localStorage.primaryLanguage"
        with self.assertRaisesRegex(GATE.QualityGateError, "api_moss_get_workspace.runs"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_formal_native_decode_proof_rejects_auto_resolved_language(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        proof["language_resolved"] = "auto"
        proof["api_fields"]["languageResolved"] = "auto"
        persisted["language_resolved"] = "auto"
        with self.assertRaisesRegex(GATE.QualityGateError, "language_resolved.*auto"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_formal_native_decode_proof_rejects_requested_resolved_mismatch(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        proof["language_resolved"] = "zh-TW"
        proof["api_fields"]["languageResolved"] = "zh-TW"
        persisted["language_resolved"] = "zh-TW"
        with self.assertRaisesRegex(GATE.QualityGateError, "requested/resolved"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_formal_native_decode_proof_rejects_invalid_json(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        proof["decode_parameters_json"] = "{bad"
        proof["api_fields"]["decodeParametersJson"] = "{bad"
        persisted["decode_parameters_json"] = "{bad"
        with self.assertRaisesRegex(GATE.QualityGateError, "valid JSON object"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_formal_native_decode_proof_rejects_sha_mismatch(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        proof["decode_parameters_sha256"] = "0" * 64
        proof["api_fields"]["decodeParametersSha256"] = "0" * 64
        proof["recomputed_decode_parameters_sha256"] = "0" * 64
        persisted["decode_parameters_sha256"] = "0" * 64
        with self.assertRaisesRegex(GATE.QualityGateError, "actual JSON bytes"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_formal_native_decode_proof_rejects_database_mismatch(self) -> None:
        proof, persisted = self.moss_native_decode_proof()
        persisted["language_requested"] = "auto"
        with self.assertRaisesRegex(GATE.QualityGateError, "persisted MOSS run"):
            FORMAL._validated_moss_native_decode_contract(proof, persisted)

    def test_quality_gate_format_documents_persisted_moss_decode_contract(self) -> None:
        path = (
            REPO
            / "target"
            / "release"
            / "docs"
            / "方案"
            / "MOSS功能修复计划-20260902"
            / "QUALITY-GATE-FORMAT.md"
        )
        document = path.read_text(encoding="utf-8")
        for token in (
            '"language_requested": "zh-CN"',
            '"language_resolved": "zh-CN"',
            '"language_resolution_source": "api_moss_get_workspace.runs"',
            '"decode_parameters_json":',
            '"decode_parameters_sha256":',
            "api_moss_get_workspace.runs",
            "languageResolved=auto",
            "请求语言和解析语言不一致",
            "不是合法 JSON 对象",
            "真实 UTF-8 字节重算",
        ):
            with self.subTest(token=token):
                self.assertIn(token, document)


if __name__ == "__main__":
    unittest.main(verbosity=2)
