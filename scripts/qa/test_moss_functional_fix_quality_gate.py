#!/usr/bin/env python3
"""Synthetic, standard-library tests for the Q00 short quality gate."""

from __future__ import annotations

import csv
import importlib.util
import json
from pathlib import Path
import shutil
import struct
import sys
import tempfile
import unittest
import wave


QA_DIR = Path(__file__).resolve().parent
if str(QA_DIR) not in sys.path:
    sys.path.insert(0, str(QA_DIR))
SPEC = importlib.util.spec_from_file_location(
    "moss_functional_fix_quality_gate",
    QA_DIR / "moss_functional_fix_quality_gate.py",
)
assert SPEC and SPEC.loader
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


COMMIT = "a" * 40
OLD_COMMIT = "b" * 40
RUN_ID = "Q00-aaaaaaaaaaaa-synthetic01"
STARTED_AT = "2026-09-02T00:00:00Z"
PRODUCED_AT = "2026-09-02T00:05:00Z"
COMPLETED_AT = "2026-09-02T00:10:00Z"
MOSS_DECODE_PARAMETERS_JSON = '{"language":"zh","timestamps":"segment","diarize":"on"}'


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def write_pcm_wav(path: Path, frames: int, *, rate: int = 16_000, width: int = 2) -> None:
    with wave.open(str(path), "wb") as writer:
        writer.setnchannels(1)
        writer.setsampwidth(width)
        writer.setframerate(rate)
        writer.writeframes(b"\x00" * frames * width)


def record(path: Path) -> dict[str, object]:
    size, digest = GATE.hash_file(path)
    return {"bytes": size, "sha256": digest}


class Q00QualityGateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.shared = Path(tempfile.mkdtemp(prefix="q00-quality-shared-"))
        cls.producer = cls.shared / "synthetic-producer.bin"
        cls.producer.write_bytes(b"synthetic current commit producer\n")
        cls.producer_record = record(cls.producer)
        cls.window = cls.shared / "q00-window.wav"
        write_pcm_wav(cls.window, GATE.WINDOW_FRAME_COUNT)
        cls.window_record = record(cls.window)
        cls.manifest = cls.shared / "q00-window-manifest.json"
        write_json(
            cls.manifest,
            {
                "schema_version": 1,
                "stage": GATE.PREPARE_STAGE,
                "status": "PREPARED",
                "generated_at": "2026-09-02T00:00:00Z",
                "source_commit": COMMIT,
                "selection_rule": "FIXED_RANGE_WITHOUT_MODEL_OUTPUT",
                "source_audio": {
                    "bytes": GATE.SOURCE_BYTES,
                    "sha256": GATE.SOURCE_SHA256,
                    "channels": 1,
                    "sample_rate_hz": 16_000,
                    "sample_width_bytes": 2,
                    "frame_count": GATE.SOURCE_FRAMES,
                    "duration_seconds": GATE.SOURCE_DURATION_SECONDS,
                },
                "crop": {
                    "source_start_seconds": GATE.WINDOW_START_SECONDS,
                    "source_end_seconds": GATE.WINDOW_END_SECONDS,
                    "duration_seconds": GATE.WINDOW_DURATION_SECONDS,
                    "start_frame": GATE.WINDOW_START_FRAME,
                    "frame_count": GATE.WINDOW_FRAME_COUNT,
                },
                "output_audio": {
                    **cls.window_record,
                    "channels": 1,
                    "sample_rate_hz": 16_000,
                    "sample_width_bytes": 2,
                    "frame_count": GATE.WINDOW_FRAME_COUNT,
                    "duration_seconds": GATE.WINDOW_DURATION_SECONDS,
                },
                "producer": {
                    "kind": "DETERMINISTIC_PCM_WINDOW_PREPARER",
                    "executable_path": str(cls.producer),
                    **cls.producer_record,
                },
            },
        )

    @classmethod
    def tearDownClass(cls) -> None:
        shutil.rmtree(cls.shared)

    def setUp(self) -> None:
        self.temp = Path(tempfile.mkdtemp(prefix="q00-quality-case-"))

    def tearDown(self) -> None:
        shutil.rmtree(self.temp)

    def provenance(
        self,
        role: str,
        *,
        commit: str = COMMIT,
        run_id: str = RUN_ID,
        producer_sha256: str | None = None,
        produced_at: str = PRODUCED_AT,
    ) -> dict[str, object]:
        return {
            "artifact_role": role,
            "run_id": run_id,
            "source_commit": commit,
            "window_audio_sha256": self.window_record["sha256"],
            "producer_executable_sha256": producer_sha256
            or self.producer_record["sha256"],
            "produced_at": produced_at,
        }

    def transcript(
        self,
        role: str,
        engine: str,
        *,
        text: str = "你好世界术语",
        speaker: str = "S1",
        segments: list[dict[str, object]] | None = None,
        inference_seconds: float = 10.0,
        commit: str = COMMIT,
        run_id: str = RUN_ID,
        producer_sha256: str | None = None,
        language_requested: str = "zh-CN",
        language_resolved: str = "zh-CN",
        language_resolution_source: str | None = None,
        window_audio_sha256: str | None = None,
        window_duration_seconds: float = GATE.WINDOW_DURATION_SECONDS,
        model_sha256: str | None = None,
        decode_parameters: dict[str, object] | None = None,
        decode_parameters_json: str | None = None,
        decode_parameters_sha256: str | None = None,
        override_before_speaker: str = "S2",
        override_reference_speaker: str = "S1",
    ) -> dict[str, object]:
        document: dict[str, object] = {
            "schema_version": 1,
            "engine": engine,
            "provenance": self.provenance(
                role,
                commit=commit,
                run_id=run_id,
                producer_sha256=producer_sha256,
            ),
            "segments": segments
            if segments is not None
            else [
                {
                    "start_seconds": 0.0,
                    "end_seconds": 2.0,
                    "speaker_id": speaker,
                    "text": text,
                }
            ],
        }
        if role in {"moss_raw", "whisper_same_window"}:
            engine_key = "moss" if role == "moss_raw" else "whisper"
            contract = GATE._expected_transcription_contract(engine_key)
            inference_contract: dict[str, object] = {
                "window_audio_sha256": window_audio_sha256
                or self.window_record["sha256"],
                "window_duration_seconds": window_duration_seconds,
                "language_requested": language_requested,
                "language_resolved": language_resolved,
                "language_resolution_source": language_resolution_source
                or contract["language_resolution_source"],
                "model_sha256": model_sha256
                or GATE.MODEL_CONTRACTS[engine_key]["sha256"],
            }
            if role == "moss_raw":
                raw_parameters = (
                    MOSS_DECODE_PARAMETERS_JSON
                    if decode_parameters_json is None
                    else decode_parameters_json
                )
                inference_contract["decode_parameters_json"] = raw_parameters
                inference_contract["decode_parameters_sha256"] = (
                    decode_parameters_sha256
                    or GATE.sha256_bytes(raw_parameters.encode("utf-8"))
                )
            else:
                parameters = (
                    dict(contract["decode_parameters"])
                    if decode_parameters is None
                    else decode_parameters
                )
                inference_contract["decode_parameters"] = parameters
                inference_contract["decode_parameters_sha256"] = (
                    decode_parameters_sha256
                    or GATE.sha256_bytes(GATE.canonical_json_bytes(parameters))
                )
            document["inference_contract"] = inference_contract
        if role == "moss_raw":
            document["inference_seconds"] = inference_seconds
        if role == "corrected":
            document["manual_speaker_overrides"] = [
                {
                    "wrong_override_id": "synthetic-override-1",
                    "override_id": "synthetic-override-2",
                    "segment_id": "synthetic-segment-0",
                    "segment_index": 0,
                    "source": "HUMAN_SINGLE_SEGMENT_OVERRIDE",
                    "before_speaker_id": override_before_speaker,
                    "after_speaker_id": speaker,
                    "reference_speaker_id": override_reference_speaker,
                }
            ]
        return document

    def make_case(
        self,
        *,
        truth_text: str = "你好世界术语",
        moss_text: str = "你好世界术语",
        whisper_text: str = "你好世界术语",
        corrected_text: str = "你好世界术语",
        moss_segments: list[dict[str, object]] | None = None,
        whisper_segments: list[dict[str, object]] | None = None,
        corrected_segments: list[dict[str, object]] | None = None,
        corrected_speaker: str = "S1",
        override_before_speaker: str = "S2",
        override_reference_speaker: str = "S1",
        inference_seconds: float = 10.0,
        speaker_truth_rows: list[list[object]] | None = None,
        moss_language_requested: str = "zh-CN",
        moss_language_resolved: str = "zh-CN",
        moss_language_resolution_source: str | None = None,
        whisper_language_resolved: str = "zh-CN",
        moss_window_audio_sha256: str | None = None,
        whisper_window_audio_sha256: str | None = None,
        moss_window_duration_seconds: float = GATE.WINDOW_DURATION_SECONDS,
        whisper_window_duration_seconds: float = GATE.WINDOW_DURATION_SECONDS,
        moss_model_sha256: str | None = None,
        whisper_model_sha256: str | None = None,
        moss_decode_parameters: dict[str, object] | None = None,
        moss_decode_parameters_json: str | None = None,
        whisper_decode_parameters: dict[str, object] | None = None,
        moss_decode_parameters_sha256: str | None = None,
        whisper_decode_parameters_sha256: str | None = None,
        positive_term: str = "术语",
        positive_expected: int | None = None,
        negative_term: str = "不存在",
        activation_good: bool = True,
        summary_good: bool = True,
        current_artifact_commit: str = COMMIT,
        current_artifact_run_id: str = RUN_ID,
        current_provenance_producer_sha256: str | None = None,
        binding_commit: str = COMMIT,
        corrupt_binding_role: str | None = None,
        corrupt_producer_role: str | None = None,
        empty_human_truth: bool = False,
        raw_json_bytes: bytes | None = None,
        omit_schema_role: str | None = None,
    ) -> tuple[dict[str, Path], Path]:
        human = self.temp / "human.tsv"
        with human.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
            writer.writerow(GATE.HUMAN_VERBATIM_HEADER)
            if not empty_human_truth:
                writer.writerow(
                    ["H1", "0", "2000", "S1", truth_text, "false", "false", "true", "synthetic"]
                )
        speaker = self.temp / "speaker.tsv"
        with speaker.open("w", encoding="utf-8", newline="") as stream:
            writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
            writer.writerow(GATE.SPEAKER_TRUTH_HEADER)
            for row in speaker_truth_rows or [
                ["T1", "0", "2000", "S1", "false", "true", "synthetic"]
            ]:
                writer.writerow(row)
        human_record = record(human)
        speaker_record = record(speaker)

        moss = self.temp / "moss.json"
        whisper = self.temp / "whisper.json"
        corrected = self.temp / "corrected.json"
        moss_document = self.transcript(
            "moss_raw",
            "MOSS",
            text=moss_text,
            segments=moss_segments,
            inference_seconds=inference_seconds,
            commit=current_artifact_commit,
            run_id=current_artifact_run_id,
            producer_sha256=current_provenance_producer_sha256,
            language_requested=moss_language_requested,
            language_resolved=moss_language_resolved,
            language_resolution_source=moss_language_resolution_source,
            window_audio_sha256=moss_window_audio_sha256,
            window_duration_seconds=moss_window_duration_seconds,
            model_sha256=moss_model_sha256,
            decode_parameters=moss_decode_parameters,
            decode_parameters_json=moss_decode_parameters_json,
            decode_parameters_sha256=moss_decode_parameters_sha256,
        )
        if omit_schema_role == "moss_raw":
            moss_document.pop("schema_version")
        if raw_json_bytes is None:
            write_json(moss, moss_document)
        else:
            moss.write_bytes(raw_json_bytes)
        whisper_document = self.transcript(
                "whisper_same_window",
                "WHISPER",
                text=whisper_text,
                segments=whisper_segments,
                commit=current_artifact_commit,
                run_id=current_artifact_run_id,
                producer_sha256=current_provenance_producer_sha256,
                language_resolved=whisper_language_resolved,
                window_audio_sha256=whisper_window_audio_sha256,
                window_duration_seconds=whisper_window_duration_seconds,
                model_sha256=whisper_model_sha256,
                decode_parameters=whisper_decode_parameters,
                decode_parameters_sha256=whisper_decode_parameters_sha256,
            )
        if omit_schema_role == "whisper_same_window":
            whisper_document.pop("schema_version")
        write_json(
            whisper,
            whisper_document,
        )
        corrected_document = self.transcript(
                "corrected",
                "MOSS_CORRECTED",
                text=corrected_text,
                speaker=corrected_speaker,
                segments=corrected_segments,
                commit=current_artifact_commit,
                run_id=current_artifact_run_id,
                producer_sha256=current_provenance_producer_sha256,
                override_before_speaker=override_before_speaker,
                override_reference_speaker=override_reference_speaker,
            )
        if omit_schema_role == "corrected":
            corrected_document.pop("schema_version")
        write_json(
            corrected,
            corrected_document,
        )

        positive = self.temp / "positive.json"
        normalized_truth = GATE._normalize_cer_text(truth_text)
        normalized_positive = GATE._normalize_cer_text(positive_term)
        if positive_expected is None:
            positive_expected = normalized_truth.count(normalized_positive)
        write_json(
            positive,
            {
                "schema_version": 1,
                "stage": "MOSS_FUNCTIONAL_FIX_Q00_POSITIVE_TRUTH",
                "status": "APPROVED",
                "window_audio_sha256": self.window_record["sha256"],
                "human_verbatim_sha256": human_record["sha256"],
                "speaker_truth_sha256": speaker_record["sha256"],
                "reviewer_id": "synthetic-reviewer",
                "approved_at": "2026-09-01T00:00:00Z",
                "attestation": GATE.POSITIVE_ATTESTATION,
                "terms": [
                    {
                        "term": positive_term,
                        "expected_occurrences": positive_expected,
                        "source_evidence_sha256": human_record["sha256"],
                    }
                ],
            },
        )
        negative = self.temp / "negative.json"
        write_json(
            negative,
            {
                "schema_version": 1,
                "stage": "MOSS_FUNCTIONAL_FIX_Q00_NEGATIVE_TRUTH",
                "status": "APPROVED",
                "window_audio_sha256": self.window_record["sha256"],
                "human_verbatim_sha256": human_record["sha256"],
                "speaker_truth_sha256": speaker_record["sha256"],
                "reviewer_id": "synthetic-reviewer",
                "approved_at": "2026-09-01T00:00:00Z",
                "attestation": GATE.NEGATIVE_ATTESTATION,
                "terms": [
                    {
                        "term": negative_term,
                        "expected_occurrences": 0,
                        "source_evidence_sha256": human_record["sha256"],
                    }
                ],
            },
        )

        moss_record = record(moss)
        corrected_record = record(corrected)
        activation = self.temp / "activation.json"
        write_json(
            activation,
            {
                "schema_version": 1,
                "provenance": self.provenance(
                    "activation_evidence",
                    commit=current_artifact_commit,
                    run_id=current_artifact_run_id,
                    producer_sha256=current_provenance_producer_sha256,
                ),
                "status": "ACTIVATED" if activation_good else "PENDING",
                "candidate_source": "MOSS",
                "activation_kind": "HUMAN_CONFIRMED",
                "raw_candidate_sha256": moss_record["sha256"],
                "candidate_transcript_sha256": corrected_record["sha256"]
                if activation_good
                else "0" * 64,
                "active_transcript_sha256": corrected_record["sha256"],
            },
        )
        activation_record = record(activation)
        summary = self.temp / "summary.json"
        write_json(
            summary,
            {
                "schema_version": 1,
                "provenance": self.provenance(
                    "summary_evidence",
                    commit=current_artifact_commit,
                    run_id=current_artifact_run_id,
                    producer_sha256=current_provenance_producer_sha256,
                ),
                "status": "COMPLETED",
                "source_kind": "ACTIVE_MOSS_TRANSCRIPT"
                if summary_good
                else "WHISPER_TRANSCRIPT",
                "model_family": "QWEN_2B",
                "source_transcript_sha256": corrected_record["sha256"],
                "activation_evidence_sha256": activation_record["sha256"],
            },
        )

        artifacts = {
            "window_audio": self.window,
            "window_manifest": self.manifest,
            "moss_raw": moss,
            "whisper_same_window": whisper,
            "corrected": corrected,
            "human_verbatim": human,
            "speaker_truth": speaker,
            "positive_truth": positive,
            "negative_truth": negative,
            "activation_evidence": activation,
            "summary_evidence": summary,
        }
        bindings = self.temp / "bindings.json"
        bound_artifacts: dict[str, object] = {}
        for role, path in artifacts.items():
            artifact_record = record(path)
            if role == corrupt_binding_role:
                artifact_record["sha256"] = "F" * 64
            producer_record = dict(self.producer_record)
            if role == corrupt_producer_role:
                producer_record["sha256"] = "E" * 64
            bound_artifacts[role] = {
                **artifact_record,
                "origin": GATE.ARTIFACT_ORIGINS[role],
                "source_commit": binding_commit,
                "run_id": RUN_ID,
                "producer": {
                    "executable_path": str(self.producer),
                    **producer_record,
                },
            }
        write_json(
            bindings,
            {
                "schema_version": 1,
                "stage": GATE.BINDINGS_STAGE,
                "formal_short_gate": True,
                "current_run": True,
                "run_id": RUN_ID,
                "source_commit": binding_commit,
                "started_at": STARTED_AT,
                "completed_at": COMPLETED_AT,
                "window_audio_sha256": self.window_record["sha256"],
                "artifacts": bound_artifacts,
            },
        )
        return artifacts, bindings

    def build(self, **kwargs: object) -> tuple[dict[str, object], dict[str, object], dict[str, Path], Path]:
        artifacts, bindings = self.make_case(**kwargs)
        public, private = GATE.build_score_reports(
            artifact_paths=artifacts,
            bindings_path=bindings,
            source_commit=COMMIT,
            scorer_path=self.producer,
        )
        return public, private, artifacts, bindings

    def score_to_files(self, **kwargs: object) -> tuple[dict[str, object], dict[str, Path], Path, Path, Path]:
        artifacts, bindings = self.make_case(**kwargs)
        public_path = self.temp / "public.json"
        private_path = self.temp / "private.json"
        public, _ = GATE.score_files(
            artifact_paths=artifacts,
            bindings_path=bindings,
            public_report_path=public_path,
            private_report_path=private_path,
            source_commit=COMMIT,
            scorer_path=self.producer,
        )
        return public, artifacts, bindings, public_path, private_path

    def test_01_positive_score_passes_all_hard_gates(self) -> None:
        public, _, _, _ = self.build()
        self.assertEqual(public["status"], "PASS")
        self.assertTrue(all(row["status"] == "PASS" for row in public["gates"].values()))

    def test_02_raw_cer_over_twenty_percent_fails(self) -> None:
        public, _, _, _ = self.build(moss_text="完全错误内容")
        self.assertEqual(public["gates"]["moss_raw_cer_at_most_20_percent"]["status"], "FAIL")

    def test_03_raw_moss_worse_than_whisper_fails(self) -> None:
        public, _, _, _ = self.build(moss_text="你好世界术错")
        self.assertEqual(public["gates"]["moss_raw_not_worse_than_whisper"]["status"], "FAIL")

    def test_04_corrected_cer_over_fifteen_percent_fails(self) -> None:
        public, _, _, _ = self.build(corrected_text="你好世界术错")
        self.assertEqual(public["gates"]["corrected_cer_at_most_15_percent"]["status"], "FAIL")

    def test_05_positive_occurrence_accuracy_failure_is_measured(self) -> None:
        public, _, _, _ = self.build(corrected_text="你好世界")
        gate = public["gates"]["positive_occurrence_accuracy_at_least_95_percent"]
        self.assertEqual(gate["status"], "FAIL")
        self.assertEqual(gate["actual"], 0.0)

    def test_06_negative_insertion_fails(self) -> None:
        public, _, _, _ = self.build(corrected_text="你好世界术语不存在")
        self.assertEqual(public["gates"]["negative_term_insertions_zero"]["status"], "FAIL")
        self.assertEqual(public["metrics"]["negative_term_insertions"], 1)

    def test_07_corrected_speaker_error_fails(self) -> None:
        public, _, _, _ = self.build(corrected_speaker="S2")
        self.assertEqual(
            public["gates"]["manual_single_segment_coverage_errors_zero"]["status"],
            "FAIL",
        )

    def test_08_rtf_over_one_fails(self) -> None:
        public, _, _, _ = self.build(inference_seconds=GATE.WINDOW_DURATION_SECONDS + 0.001)
        self.assertEqual(public["gates"]["moss_rtf_at_most_one"]["status"], "FAIL")

    def test_09_activation_source_binding_failure_fails(self) -> None:
        public, _, _, _ = self.build(activation_good=False)
        self.assertEqual(public["gates"]["candidate_activation_bound"]["status"], "FAIL")

    def test_10_summary_source_binding_failure_fails(self) -> None:
        public, _, _, _ = self.build(summary_good=False)
        self.assertEqual(public["gates"]["summary_source_bound"]["status"], "FAIL")

    def test_11_negative_timestamp_fails(self) -> None:
        segments = [{"start_seconds": -0.1, "end_seconds": 2.0, "speaker_id": "S1", "text": "你好世界术语"}]
        public, _, _, _ = self.build(moss_segments=segments)
        self.assertEqual(public["gates"]["all_transcript_timestamps_valid"]["status"], "FAIL")

    def test_12_non_monotonic_timestamps_fail(self) -> None:
        segments = [
            {"start_seconds": 1.0, "end_seconds": 2.0, "speaker_id": "S1", "text": "你好世"},
            {"start_seconds": 0.5, "end_seconds": 1.5, "speaker_id": "S1", "text": "界术语"},
        ]
        public, _, _, _ = self.build(moss_segments=segments)
        self.assertEqual(public["gates"]["all_transcript_timestamps_valid"]["status"], "FAIL")

    def test_13_bad_artifact_hash_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "moss_raw bytes or SHA-256"):
            self.build(corrupt_binding_role="moss_raw")

    def test_14_bad_live_producer_hash_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "producer executable hash"):
            self.build(corrupt_producer_role="moss_raw")

    def test_15_missing_human_truth_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "no valid scoring speech"):
            self.build(empty_human_truth=True)

    def test_16_old_candidate_commit_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "current source commit"):
            self.build(current_artifact_commit=OLD_COMMIT)

    def test_17_stale_binding_commit_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "source_commit is stale"):
            self.build(binding_commit=OLD_COMMIT)

    def test_18_nonfinite_json_number_is_rejected(self) -> None:
        raw = b'{"provenance":{},"engine":"MOSS","inference_seconds":NaN,"segments":[]}\n'
        with self.assertRaisesRegex(GATE.GateError, "non-finite"):
            self.build(raw_json_bytes=raw)

    def test_19_positive_expected_count_must_match_human_truth(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "expected_occurrences"):
            self.build(positive_expected=2)

    def test_20_negative_term_must_really_be_unspoken(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "Negative truth contains"):
            self.build(negative_term="术语")

    def test_21_public_report_contains_no_transcript_or_real_paths(self) -> None:
        secret = "绝密项目松鼠七号"
        public, _, _, _ = self.build(
            truth_text=f"你好{secret}术语",
            moss_text=f"你好{secret}术语",
            whisper_text=f"你好{secret}术语",
            corrected_text=f"你好{secret}术语",
        )
        encoded = json.dumps(public, ensure_ascii=False)
        self.assertNotIn(secret, encoded)
        self.assertNotIn(str(self.temp), encoded)
        self.assertNotIn(str(self.producer), encoded)
        self.assertFalse(public["privacy"]["contains_transcript_or_term_text"])

    def test_22_verify_accepts_untampered_pass(self) -> None:
        _, artifacts, bindings, public_path, private_path = self.score_to_files()
        status = GATE.verify_files(
            artifact_paths=artifacts,
            bindings_path=bindings,
            public_report_path=public_path,
            private_report_path=private_path,
            source_commit=COMMIT,
            scorer_path=self.producer,
        )
        self.assertEqual(status, "PASS")

    def test_23_verify_rejects_metric_tampering(self) -> None:
        _, artifacts, bindings, public_path, private_path = self.score_to_files()
        document = json.loads(public_path.read_text(encoding="utf-8"))
        document["metrics"]["moss_raw_cer"] = 0.0
        document["metrics"]["reference_character_count"] = 999
        write_json(public_path, document)
        with self.assertRaisesRegex(GATE.QualityGateError, "Public report"):
            GATE.verify_files(
                artifact_paths=artifacts,
                bindings_path=bindings,
                public_report_path=public_path,
                private_report_path=private_path,
                source_commit=COMMIT,
                scorer_path=self.producer,
            )

    def test_24_verify_rejects_forged_pass_status(self) -> None:
        _, artifacts, bindings, public_path, private_path = self.score_to_files(
            corrected_text="你好世界"
        )
        document = json.loads(public_path.read_text(encoding="utf-8"))
        document["status"] = "PASS"
        document["failed_gates"] = []
        write_json(public_path, document)
        with self.assertRaisesRegex(GATE.QualityGateError, "Public report"):
            GATE.verify_files(
                artifact_paths=artifacts,
                bindings_path=bindings,
                public_report_path=public_path,
                private_report_path=private_path,
                source_commit=COMMIT,
                scorer_path=self.producer,
            )

    def test_25_verify_rejects_changed_input_artifact(self) -> None:
        _, artifacts, bindings, public_path, private_path = self.score_to_files()
        artifacts["moss_raw"].write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(GATE.QualityGateError, "moss_raw bytes or SHA-256"):
            GATE.verify_files(
                artifact_paths=artifacts,
                bindings_path=bindings,
                public_report_path=public_path,
                private_report_path=private_path,
                source_commit=COMMIT,
                scorer_path=self.producer,
            )

    def test_26_raw_speaker_metrics_have_formal_hard_thresholds(self) -> None:
        public, _, _, _ = self.build()
        metrics = public["metrics"]
        self.assertEqual(metrics["raw_speaker_segment_error_rate"], 0.0)
        self.assertEqual(metrics["raw_speaker_duration_error_rate"], 0.0)
        self.assertIn("raw_speaker_false_alarm_seconds", metrics)
        self.assertEqual(
            public["gates"]["raw_speaker_segment_error_rate_at_most_10_percent"]["status"],
            "PASS",
        )
        self.assertEqual(
            public["gates"]["raw_speaker_duration_error_rate_at_most_10_percent"]["status"],
            "PASS",
        )

    def test_27_missing_raw_speaker_is_a_hard_failure(self) -> None:
        segments = [{"start_seconds": 0.0, "end_seconds": 2.0, "text": "你好世界术语"}]
        public, _, _, _ = self.build(moss_segments=segments)
        self.assertEqual(public["gates"]["raw_speaker_measurement_complete"]["status"], "FAIL")

    def test_28_prepare_is_deterministic_and_records_hash(self) -> None:
        source = self.temp / "small-source.wav"
        with wave.open(str(source), "wb") as writer:
            writer.setnchannels(1)
            writer.setsampwidth(2)
            writer.setframerate(16_000)
            samples = b"".join(struct.pack("<h", index % 30000) for index in range(64_000))
            writer.writeframes(samples)
        source_hash = record(source)["sha256"]
        output = self.temp / "window.wav"
        manifest = self.temp / "manifest.json"
        document = GATE.prepare_window(
            source_wav=source,
            output_wav=output,
            manifest_path=manifest,
            source_commit=COMMIT,
            tool_path=self.producer,
            generated_at="2026-09-02T00:00:00Z",
            expected_source_sha256=source_hash,
            expected_source_frames=64_000,
            start_seconds=1.0,
            duration_seconds=2.0,
        )
        self.assertEqual(document["output_audio"]["sha256"], record(output)["sha256"])
        with wave.open(str(output), "rb") as reader:
            self.assertEqual(reader.getnframes(), 32_000)
            self.assertEqual(reader.readframes(1), samples[32_000:32_002])

    def test_29_prepare_rejects_source_hash_mismatch(self) -> None:
        source = self.temp / "source.wav"
        write_pcm_wav(source, 32_000)
        with self.assertRaisesRegex(GATE.QualityGateError, "SHA-256 mismatch"):
            GATE.prepare_window(
                source_wav=source,
                output_wav=self.temp / "output.wav",
                manifest_path=self.temp / "manifest.json",
                source_commit=COMMIT,
                tool_path=self.producer,
                expected_source_sha256="0" * 64,
                expected_source_frames=32_000,
                start_seconds=0.0,
                duration_seconds=1.0,
            )

    def test_30_prepare_refuses_to_overwrite(self) -> None:
        source = self.temp / "source.wav"
        write_pcm_wav(source, 32_000)
        output = self.temp / "output.wav"
        output.write_bytes(b"keep")
        with self.assertRaisesRegex(GATE.QualityGateError, "Refusing to overwrite"):
            GATE.prepare_window(
                source_wav=source,
                output_wav=output,
                manifest_path=self.temp / "manifest.json",
                source_commit=COMMIT,
                tool_path=self.producer,
                expected_source_sha256=record(source)["sha256"],
                expected_source_frames=32_000,
                start_seconds=0.0,
                duration_seconds=1.0,
            )
        self.assertEqual(output.read_bytes(), b"keep")

    def test_31_prepare_rejects_wrong_pcm_format(self) -> None:
        source = self.temp / "source.wav"
        write_pcm_wav(source, 32_000, rate=8_000)
        with self.assertRaisesRegex(GATE.QualityGateError, "mono 16 kHz"):
            GATE.prepare_window(
                source_wav=source,
                output_wav=self.temp / "output.wav",
                manifest_path=self.temp / "manifest.json",
                source_commit=COMMIT,
                tool_path=self.producer,
                expected_source_sha256=record(source)["sha256"],
                expected_source_frames=32_000,
                start_seconds=0.0,
                duration_seconds=1.0,
            )

    def test_32_score_refuses_to_overwrite_reports(self) -> None:
        artifacts, bindings = self.make_case()
        public_path = self.temp / "public.json"
        private_path = self.temp / "private.json"
        public_path.write_text("keep", encoding="utf-8")
        with self.assertRaisesRegex(GATE.QualityGateError, "Refusing to overwrite"):
            GATE.score_files(
                artifact_paths=artifacts,
                bindings_path=bindings,
                public_report_path=public_path,
                private_report_path=private_path,
                source_commit=COMMIT,
                scorer_path=self.producer,
            )
        self.assertEqual(public_path.read_text(encoding="utf-8"), "keep")

    def test_33_old_run_id_is_rejected_even_on_same_commit(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "current Q00 run"):
            self.build(current_artifact_run_id="Q00-aaaaaaaaaaaa-oldrun")

    def test_34_missing_current_schema_field_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "schema_version"):
            self.build(omit_schema_role="moss_raw")

    def test_35_current_json_producer_hash_must_match_live_binding(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "producer hash"):
            self.build(current_provenance_producer_sha256="D" * 64)

    @staticmethod
    def two_speaker_truth() -> list[list[object]]:
        return [
            ["T1", "0", "1000", "S1", "false", "true", "synthetic"],
            ["T2", "1000", "2000", "S2", "false", "true", "synthetic"],
        ]

    @staticmethod
    def raw_speaker_boundary_segments(*, first_wrong_end: float) -> list[dict[str, object]]:
        segments: list[dict[str, object]] = []
        cursor = 0.0
        while cursor < 1.0 - 1e-9:
            end = min(1.0, cursor + 0.2)
            speaker = "P2" if cursor < first_wrong_end - 1e-9 else "P1"
            segments.append(
                {"start_seconds": cursor, "end_seconds": end, "speaker_id": speaker, "text": "你"}
            )
            cursor = end
        while cursor < 2.0 - 1e-9:
            end = min(2.0, cursor + 0.2)
            segments.append(
                {"start_seconds": cursor, "end_seconds": end, "speaker_id": "P2", "text": "好"}
            )
            cursor = end
        return segments

    @staticmethod
    def corrected_two_speaker_segments() -> list[dict[str, object]]:
        return [
            {"start_seconds": 0.0, "end_seconds": 1.0, "speaker_id": "S1", "text": "你好世"},
            {"start_seconds": 1.0, "end_seconds": 2.0, "speaker_id": "S2", "text": "界术语"},
        ]

    def test_36_raw_speaker_rates_above_ten_percent_fail(self) -> None:
        public, _, _, _ = self.build(
            moss_segments=self.raw_speaker_boundary_segments(first_wrong_end=0.4),
            corrected_segments=self.corrected_two_speaker_segments(),
            speaker_truth_rows=self.two_speaker_truth(),
        )
        self.assertGreater(public["metrics"]["raw_speaker_segment_error_rate"], 0.10)
        self.assertGreater(public["metrics"]["raw_speaker_duration_error_rate"], 0.10)
        self.assertEqual(
            public["gates"]["raw_speaker_segment_error_rate_at_most_10_percent"]["status"],
            "FAIL",
        )
        self.assertEqual(
            public["gates"]["raw_speaker_duration_error_rate_at_most_10_percent"]["status"],
            "FAIL",
        )

    def test_37_raw_speaker_rates_exactly_ten_percent_pass(self) -> None:
        segments = [
            {
                "start_seconds": float(index),
                "end_seconds": float(index + 1),
                "speaker_id": "P2" if index == 0 or index >= 5 else "P1",
                "text": "你",
            }
            for index in range(10)
        ]
        public, _, _, _ = self.build(
            moss_segments=segments,
            corrected_segments=[
                {"start_seconds": 0.0, "end_seconds": 5.0, "speaker_id": "S1", "text": "你好世"},
                {"start_seconds": 5.0, "end_seconds": 10.0, "speaker_id": "S2", "text": "界术语"},
            ],
            speaker_truth_rows=[
                ["T1", "0", "5000", "S1", "false", "true", "synthetic"],
                ["T2", "5000", "10000", "S2", "false", "true", "synthetic"],
            ],
        )
        self.assertAlmostEqual(public["metrics"]["raw_speaker_segment_error_rate"], 0.10)
        self.assertAlmostEqual(public["metrics"]["raw_speaker_duration_error_rate"], 0.10)
        self.assertEqual(
            public["gates"]["raw_speaker_segment_error_rate_at_most_10_percent"]["status"],
            "PASS",
        )
        self.assertEqual(
            public["gates"]["raw_speaker_duration_error_rate_at_most_10_percent"]["status"],
            "PASS",
        )

    def test_boundary_duration_rate_even_slightly_above_ten_percent_fails(self) -> None:
        segments = [
            {
                "start_seconds": 0.0,
                "end_seconds": 0.200000000001,
                "speaker_id": "P2",
                "text": "你",
            },
            {
                "start_seconds": 0.200000000001,
                "end_seconds": 1.0,
                "speaker_id": "P1",
                "text": "好",
            },
            {
                "start_seconds": 1.0,
                "end_seconds": 2.0,
                "speaker_id": "P2",
                "text": "世界",
            },
        ]
        public, _, _, _ = self.build(
            moss_segments=segments,
            corrected_segments=self.corrected_two_speaker_segments(),
            speaker_truth_rows=self.two_speaker_truth(),
        )
        self.assertGreater(public["metrics"]["raw_speaker_duration_error_rate"], 0.10)
        self.assertEqual(
            public["gates"]["raw_speaker_duration_error_rate_at_most_10_percent"]["status"],
            "FAIL",
        )

    def test_38_noop_speaker_override_fails_true_flip_gate(self) -> None:
        public, _, _, _ = self.build(override_before_speaker="S1")
        self.assertFalse(public["metrics"]["speaker_override_true_flip"])
        self.assertEqual(public["gates"]["speaker_override_true_flip"]["status"], "FAIL")

    def test_39_moss_auto_resolved_language_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "language_resolved.*auto"):
            self.build(moss_language_resolved="auto")

    def test_40_different_window_duration_fails_fairness_gate(self) -> None:
        public, _, _, _ = self.build(whisper_window_duration_seconds=226.0)
        self.assertEqual(
            public["gates"]["whisper_moss_same_226440ms_window"]["status"],
            "FAIL",
        )

    def test_41_wrong_model_hash_fails_model_binding_gate(self) -> None:
        public, _, _, _ = self.build(moss_model_sha256="F" * 64)
        self.assertEqual(
            public["gates"]["transcription_models_and_decode_parameters_bound"]["status"],
            "FAIL",
        )

    def test_42_changed_decode_parameters_fail_even_with_matching_hash(self) -> None:
        parameters = dict(GATE._expected_transcription_contract("whisper")["decode_parameters"])
        parameters["provider"] = "different-provider"
        public, _, _, _ = self.build(whisper_decode_parameters=parameters)
        self.assertEqual(
            public["gates"]["transcription_models_and_decode_parameters_bound"]["status"],
            "FAIL",
        )

    def test_43_decode_parameter_hash_tampering_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "decode_parameters_sha256"):
            self.build(moss_decode_parameters_sha256="0" * 64)

    def test_44_different_window_hash_fails_fairness_gate(self) -> None:
        public, _, _, _ = self.build(whisper_window_audio_sha256="E" * 64)
        self.assertEqual(
            public["gates"]["whisper_moss_same_226440ms_window"]["status"],
            "FAIL",
        )

    def test_45_inference_contract_rejects_extra_fields(self) -> None:
        document = self.transcript("moss_raw", "MOSS")
        contract = document["inference_contract"]
        self.assertIsInstance(contract, dict)
        contract["unbound_runtime_option"] = True
        with self.assertRaisesRegex(GATE.QualityGateError, "fields are not exact"):
            GATE._parse_inference_contract(document, "moss_raw")

    def test_46_moss_contract_missing_language_requested_is_rejected(self) -> None:
        document = self.transcript("moss_raw", "MOSS")
        document["inference_contract"].pop("language_requested")
        with self.assertRaisesRegex(GATE.QualityGateError, "fields are not exact"):
            GATE._parse_inference_contract(document, "moss_raw")

    def test_47_moss_contract_missing_language_resolved_is_rejected(self) -> None:
        document = self.transcript("moss_raw", "MOSS")
        document["inference_contract"].pop("language_resolved")
        with self.assertRaisesRegex(GATE.QualityGateError, "fields are not exact"):
            GATE._parse_inference_contract(document, "moss_raw")

    def test_48_moss_contract_missing_decode_parameters_json_is_rejected(self) -> None:
        document = self.transcript("moss_raw", "MOSS")
        document["inference_contract"].pop("decode_parameters_json")
        with self.assertRaisesRegex(GATE.QualityGateError, "fields are not exact"):
            GATE._parse_inference_contract(document, "moss_raw")

    def test_49_moss_contract_missing_decode_parameters_sha256_is_rejected(self) -> None:
        document = self.transcript("moss_raw", "MOSS")
        document["inference_contract"].pop("decode_parameters_sha256")
        with self.assertRaisesRegex(GATE.QualityGateError, "fields are not exact"):
            GATE._parse_inference_contract(document, "moss_raw")

    def test_50_moss_requested_and_resolved_language_mismatch_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "requested/resolved"):
            self.build(moss_language_resolved="zh-TW")

    def test_51_moss_wrong_requested_language_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "language_requested"):
            self.build(moss_language_requested="en-US", moss_language_resolved="en-US")

    def test_52_moss_invalid_decode_parameters_json_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "valid JSON object"):
            self.build(moss_decode_parameters_json='{"language":"zh"')

    def test_53_moss_decode_parameters_json_array_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "valid JSON object"):
            self.build(moss_decode_parameters_json='["zh"]')

    def test_54_moss_decode_parameters_sha_must_hash_raw_json_bytes(self) -> None:
        raw = '{ "language": "zh", "timestamps": "segment", "diarize": "on" }'
        canonical_hash = GATE.sha256_bytes(
            GATE.canonical_json_bytes(json.loads(raw))
        )
        with self.assertRaisesRegex(GATE.QualityGateError, "actual JSON bytes"):
            self.build(
                moss_decode_parameters_json=raw,
                moss_decode_parameters_sha256=canonical_hash,
            )

    def test_55_moss_contract_wrong_persistent_source_is_rejected(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "api_moss_get_workspace.runs"):
            self.build(moss_language_resolution_source="PRODUCT_PRIMARY_LANGUAGE")

    def test_56_moss_decode_contract_rejects_wrong_language_value(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "frozen native contract"):
            self.build(
                moss_decode_parameters_json=(
                    '{"language":"en","timestamps":"segment","diarize":"on"}'
                )
            )

    def test_57_moss_decode_contract_rejects_extra_field(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "frozen native contract"):
            self.build(
                moss_decode_parameters_json=(
                    '{"language":"zh","timestamps":"segment",'
                    '"diarize":"on","temperature":0}'
                )
            )

    def test_58_moss_decode_contract_rejects_missing_field(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "frozen native contract"):
            self.build(
                moss_decode_parameters_json=(
                    '{"language":"zh","timestamps":"segment"}'
                )
            )

    def test_59_moss_decode_contract_rejects_duplicate_field(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "valid JSON object"):
            self.build(
                moss_decode_parameters_json=(
                    '{"language":"zh","language":"en",'
                    '"timestamps":"segment","diarize":"on"}'
                )
            )

    def test_60_moss_decode_contract_rejects_nonfinite_number(self) -> None:
        with self.assertRaisesRegex(GATE.QualityGateError, "valid JSON object"):
            self.build(
                moss_decode_parameters_json=(
                    '{"language":"zh","timestamps":"segment",'
                    '"diarize":"on","temperature":NaN}'
                )
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
