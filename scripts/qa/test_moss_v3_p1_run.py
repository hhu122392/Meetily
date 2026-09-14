from __future__ import annotations

import importlib.util
import json
import os
import tempfile
import unittest
from unittest import mock
from types import SimpleNamespace
from pathlib import Path


RUNNER_PATH = Path(__file__).with_name("moss_v3_p1_run.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_p1_run", RUNNER_PATH)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class FrozenIntelVulkanIdentityTests(unittest.TestCase):
    @staticmethod
    def mapped_identity(paths: dict[Path, str]) -> dict[str, Path]:
        return {os.path.normcase(str(path)): path for path in paths}

    def test_inference_discovered_compiler_module_is_frozen(self) -> None:
        expected_path = Path(
            r"C:\Windows\System32\DriverStore\FileRepository"
            r"\iigd_dch.inf_amd64_15cb41d17b4923f1\igc-default64.dll"
        )
        self.assertEqual(
            RUNNER.EXPECTED_INTEL_VULKAN_MODULES.get(expected_path),
            "E90D7BBEE270E57A80669AC18B92ACB7B57C78DD5771B9D4E090874D8D39D837",
        )

    def test_device_only_profile_does_not_require_inference_compiler_module(self) -> None:
        expected, required = RUNNER.native_module_profile("device_only")
        compiler_path = next(
            path for path in expected if path.name.lower() == "igc-default64.dll"
        )
        self.assertNotIn(compiler_path, required)
        RUNNER.validate_required_native_module_presence(
            required, self.mapped_identity(required)
        )

    def test_inference_profile_rejects_missing_compiler_module(self) -> None:
        expected, required = RUNNER.native_module_profile("inference")
        without_compiler = {
            path: digest
            for path, digest in expected.items()
            if path.name.lower() != "igc-default64.dll"
        }
        with self.assertRaisesRegex(RUNNER.GateError, "igc-default64.dll"):
            RUNNER.validate_required_native_module_presence(
                required, self.mapped_identity(without_compiler)
            )


class SilenceParserTests(unittest.TestCase):
    def test_silence_window_is_not_coarser_than_end_tolerance(self) -> None:
        self.assertLessEqual(
            RUNNER.SILENCE_MIN_DURATION_SECONDS,
            RUNNER.OUTPUT_END_TOLERANCE_SECONDS,
        )

    def test_closes_trailing_silence_at_audio_end(self) -> None:
        log = """
[silencedetect] silence_start: 0
[silencedetect] silence_end: 25.314271 | silence_duration: 25.314271
[silencedetect] silence_start: 81.094958
"""
        intervals = RUNNER.parse_silence_intervals(log, 107.5413125)
        self.assertEqual(len(intervals), 2)
        self.assertAlmostEqual(intervals[-1]["start_seconds"], 81.094958)
        self.assertAlmostEqual(intervals[-1]["end_seconds"], 107.5413125)

    def test_uses_reported_duration_when_start_line_is_missing(self) -> None:
        log = "[silencedetect] silence_end: 4.0 | silence_duration: 1.5"
        intervals = RUNNER.parse_silence_intervals(log, 10.0)
        self.assertAlmostEqual(intervals[0]["start_seconds"], 2.5)
        self.assertAlmostEqual(intervals[0]["end_seconds"], 4.0)


class OutputValidationTests(unittest.TestCase):
    def test_malformed_native_timestamps_are_retained_in_restricted_diagnostics(self) -> None:
        native = SimpleNamespace(
            text="诊断文本",
            raw_text="[2.00][S01]诊断文本[1.00]",
            language="",
            timestamp_kind="segment",
            segments=[],
            speaker_segments=[],
        )
        with self.assertRaises(RUNNER.GateError) as caught:
            RUNNER.result_record(native)
        diagnostic = RUNNER.partial_result_record(caught.exception)
        self.assertEqual(diagnostic["raw_text"], native.raw_text)
        self.assertIn("serialization_error", diagnostic)

    @staticmethod
    def valid_output(last_timestamp: float = 81.31) -> dict:
        start_timestamp = 79.0 if last_timestamp >= 80.0 else max(0.0, last_timestamp - 10.0)
        raw_text = (
            f"[{start_timestamp}][S01]核心功能验收。下面暂停录音十秒，恢复后继续[{last_timestamp}]"
        )
        return {
            "text": "核心功能验收。下面暂停录音十秒，恢复后继续",
            "raw_text": raw_text,
            "language": "zh",
            "timestamp_kind": "segment",
            "segments": [
                {
                    "text": "核心功能验收。下面暂停录音十秒，恢复后继续",
                    "t0_ms": round(start_timestamp * 1000),
                    "t1_ms": round(last_timestamp * 1000),
                    "speaker_id": 1,
                }
            ],
            "speaker_segments": [
                {
                    "t0_ms": round(start_timestamp * 1000),
                    "t1_ms": round(last_timestamp * 1000),
                    "speaker_id": 1,
                }
            ],
            "raw_turns": RUNNER.parse_raw_turns(raw_text),
            "last_timestamp_seconds": last_timestamp,
            "last_timestamp_source": "strict_raw_turn_end",
        }

    def test_trailing_silence_is_not_misclassified_as_missing_speech(self) -> None:
        verdict = RUNNER.validate_output(
            "real_107s",
            self.valid_output(),
            107.5413125,
            {
                "last_activity_end_seconds": 81.094958,
                "significant_active_intervals": [
                    {"start_seconds": 79.0, "end_seconds": 81.094958}
                ],
            },
        )
        self.assertEqual(verdict["status"], "PASS")

    def test_missing_final_activity_fails(self) -> None:
        with self.assertRaisesRegex(RUNNER.GateError, "final non-silent audio"):
            RUNNER.validate_output(
                "real_107s",
                self.valid_output(last_timestamp=70.0),
                107.5413125,
                {
                    "last_activity_end_seconds": 81.094958,
                    "significant_active_intervals": [
                        {"start_seconds": 79.0, "end_seconds": 81.094958}
                    ],
                },
            )

    def test_isolated_early_non_silence_is_reported_without_fake_accuracy_gate(self) -> None:
        output = self.valid_output(last_timestamp=90.0)
        verdict = RUNNER.validate_output(
            "real_107s",
            output,
            107.5413125,
            {
                "last_activity_end_seconds": 90.0,
                "significant_active_intervals": [
                    {"start_seconds": 4.095, "end_seconds": 4.606},
                    {"start_seconds": 79.0, "end_seconds": 90.0},
                ],
            },
        )
        self.assertEqual(verdict["status"], "PASS")
        self.assertEqual(
            verdict["checks"]["start_alignment_status"],
            "DIAGNOSTIC_WARNING",
        )
        self.assertFalse(
            verdict["checks"]["start_alignment_is_semantic_accuracy_gate"]
        )
        self.assertRegex(
            verdict["diagnostic_warnings"][0],
            "first transcript timestamp misses",
        )

    def test_missing_known_anchor_is_reported_without_becoming_accuracy_gate(self) -> None:
        output = self.valid_output()
        output["text"] = "核心功能验收"
        output["raw_turns"][0]["text"] = "核心功能验收"
        output["segments"][0]["text"] = "核心功能验收"
        verdict = RUNNER.validate_output(
            "real_107s",
            output,
            107.5413125,
            {
                "last_activity_end_seconds": 81.094958,
                "significant_active_intervals": [
                    {"start_seconds": 79.0, "end_seconds": 81.094958}
                ],
            },
        )
        self.assertEqual(verdict["status"], "PASS")
        self.assertEqual(verdict["known_anchor_probe_status"], "FAIL")

    def test_one_tail_segment_cannot_fake_long_audio_completeness(self) -> None:
        output = {
            "text": "尾部一句",
            "raw_text": "[3095.0][S01]尾部一句[3096.6]",
            "timestamp_kind": "segment",
            "segments": [
                {"text": "尾部一句", "t0_ms": 3095000, "t1_ms": 3096600, "speaker_id": 1}
            ],
            "speaker_segments": [
                {"t0_ms": 3095000, "t1_ms": 3096600, "speaker_id": 1}
            ],
            "raw_turns": RUNNER.parse_raw_turns(
                "[3095.0][S01]尾部一句[3096.6]"
            ),
            "last_timestamp_seconds": 3096.6,
            "last_timestamp_source": "strict_raw_turn_end",
        }
        with self.assertRaises(RUNNER.GateError):
            RUNNER.validate_output(
                "long_3096s",
                output,
                3096.62,
                {
                    "last_activity_end_seconds": 3096.62,
                    "significant_active_intervals": [
                        {"start_seconds": 0.0, "end_seconds": 3096.62}
                    ],
                },
            )

    def test_repeated_full_span_segments_cannot_fake_long_audio(self) -> None:
        raw_turns = [
            {
                "start_seconds": 0.0,
                "end_seconds": 3096.62,
                "speaker_id": 1,
                "speaker_label": "S01",
                "text": "x",
            }
            for _ in range(26)
        ]
        output = {
            "text": "x" * 26,
            "raw_text": "synthetic",
            "timestamp_kind": "segment",
            "segments": [
                {"text": "x", "t0_ms": 0, "t1_ms": 3096620, "speaker_id": 1}
                for _ in range(26)
            ],
            "speaker_segments": [
                {"t0_ms": 0, "t1_ms": 3096620, "speaker_id": 1}
                for _ in range(26)
            ],
            "raw_turns": raw_turns,
            "last_timestamp_seconds": 3096.62,
            "last_timestamp_source": "strict_raw_turn_end",
        }
        with self.assertRaises(RUNNER.GateError):
            RUNNER.validate_output(
                "long_3096s",
                output,
                3096.62,
                {
                    "last_activity_end_seconds": 3096.62,
                    "significant_active_intervals": [
                        {"start_seconds": 0.0, "end_seconds": 3096.62}
                    ],
                },
            )

    def test_distributed_forty_percent_omission_fails(self) -> None:
        turns = []
        segments = []
        for index in range(156):
            start = index * 20.0
            end = min(3096.62, start + 12.0)
            if start >= 3096.62:
                break
            turns.append(
                {
                    "start_seconds": start,
                    "end_seconds": end,
                    "speaker_id": 1,
                    "speaker_label": "S01",
                    "text": "x",
                }
            )
            segments.append(
                {"text": "x", "t0_ms": round(start * 1000), "t1_ms": round(end * 1000), "speaker_id": 1}
            )
        output = {
            "text": "x" * len(turns),
            "raw_text": "synthetic",
            "timestamp_kind": "segment",
            "segments": segments,
            "speaker_segments": [dict(row) for row in segments],
            "raw_turns": turns,
            "last_timestamp_seconds": 3096.62,
            "last_timestamp_source": "strict_raw_turn_end",
        }
        with self.assertRaisesRegex(RUNNER.GateError, "covers too little"):
            RUNNER.validate_output(
                "long_3096s",
                output,
                3096.62,
                {
                    "last_activity_end_seconds": 3096.62,
                    "significant_active_intervals": [
                        {"start_seconds": 0.0, "end_seconds": 3096.62}
                    ],
                },
            )

    def test_full_timeline_with_one_character_per_turn_fails_density(self) -> None:
        turns = []
        segments = []
        cursor = 0.0
        while cursor < 3096.62:
            end = min(3096.62, cursor + 120.0)
            turns.append(
                {
                    "start_seconds": cursor,
                    "end_seconds": end,
                    "speaker_id": 1,
                    "speaker_label": "S01",
                    "text": "x",
                }
            )
            segments.append(
                {"text": "x", "t0_ms": round(cursor * 1000), "t1_ms": round(end * 1000), "speaker_id": 1}
            )
            cursor = end
        output = {
            "text": "x" * len(turns),
            "raw_text": "synthetic",
            "timestamp_kind": "segment",
            "segments": segments,
            "speaker_segments": [dict(row) for row in segments],
            "raw_turns": turns,
            "last_timestamp_seconds": 3096.62,
            "last_timestamp_source": "strict_raw_turn_end",
        }
        with self.assertRaisesRegex(RUNNER.GateError, "text density"):
            RUNNER.validate_output(
                "long_3096s",
                output,
                3096.62,
                {
                    "last_activity_end_seconds": 3096.62,
                    "significant_active_intervals": [
                        {"start_seconds": 0.0, "end_seconds": 3096.62}
                    ],
                },
            )

    def test_text_sources_must_match(self) -> None:
        output = self.valid_output()
        output["raw_turns"][0]["text"] = "另一个原始正文"
        with self.assertRaisesRegex(RUNNER.GateError, "text differs"):
            RUNNER.validate_output(
                "real_107s",
                output,
                107.5413125,
                {
                    "last_activity_end_seconds": 81.094958,
                    "significant_active_intervals": [
                        {"start_seconds": 79.0, "end_seconds": 81.094958}
                    ],
                },
            )

    def test_wrong_language_is_not_a_structural_runtime_pass(self) -> None:
        output = self.valid_output()
        output["language"] = "en"
        with self.assertRaisesRegex(RUNNER.GateError, "output language"):
            RUNNER.validate_output(
                "real_107s",
                output,
                107.5413125,
                {
                    "last_activity_end_seconds": 81.094958,
                    "significant_active_intervals": [
                        {"start_seconds": 79.0, "end_seconds": 81.094958}
                    ],
                },
            )

    def test_native_empty_language_metadata_is_unreported_not_fake_zh(self) -> None:
        output = self.valid_output()
        output["language"] = ""
        verdict = RUNNER.validate_output(
            "real_107s",
            output,
            107.5413125,
            {
                "last_activity_end_seconds": 81.094958,
                "significant_active_intervals": [
                    {"start_seconds": 79.0, "end_seconds": 81.094958}
                ],
            },
        )
        self.assertEqual(verdict["native_reported_language"], "UNREPORTED")


class RawTurnParserTests(unittest.TestCase):
    def test_accepts_measured_small_same_speaker_boundary_overlap(self) -> None:
        raw_text = "[0.00][S01]第一段[1.00][0.60][S01]第二段[2.00]"
        turns = RUNNER.parse_raw_turns(raw_text)
        output = {
            "text": "第一段第二段",
            "raw_text": raw_text,
            "language": "zh",
            "timestamp_kind": "segment",
            "segments": [
                {"text": "第一段", "t0_ms": 0, "t1_ms": 1000, "speaker_id": 1},
                {"text": "第二段", "t0_ms": 600, "t1_ms": 2000, "speaker_id": 1},
            ],
            "speaker_segments": [
                {"t0_ms": 0, "t1_ms": 1000, "speaker_id": 1},
                {"t0_ms": 600, "t1_ms": 2000, "speaker_id": 1},
            ],
            "raw_turns": turns,
            "last_timestamp_seconds": 2.0,
            "last_timestamp_source": "strict_raw_turn_end",
        }
        verdict = RUNNER.validate_output(
            "short_12s",
            output,
            2.0,
            {
                "last_activity_end_seconds": 2.0,
                "significant_active_intervals": [
                    {"start_seconds": 0.0, "end_seconds": 2.0}
                ],
            },
        )
        self.assertEqual(verdict["status"], "PASS")
        self.assertEqual(
            verdict["checks"]["maximum_observed_same_speaker_overlap_seconds"],
            0.4,
        )

    def test_parses_complete_turns_and_keeps_bracket_number_in_text(self) -> None:
        turns = RUNNER.parse_raw_turns(
            "[0.10][S01]正文里的[123]不是边界[1.20][1.30][S02]第二段[2.40]"
        )
        self.assertEqual(len(turns), 2)
        self.assertIn("[123]", turns[0]["text"])
        self.assertEqual(turns[-1]["speaker_id"], 2)

    def test_rejects_bare_speaker_label_with_parser_filled_time(self) -> None:
        with self.assertRaises(RUNNER.GateError):
            RUNNER.parse_raw_turns("[S01]只有文字")

    def test_rejects_missing_explicit_end_marker(self) -> None:
        with self.assertRaisesRegex(RUNNER.GateError, "lacks an explicit end timestamp"):
            RUNNER.parse_raw_turns("[0.10][S01]没有结尾")

    def test_rejects_reversed_timestamps(self) -> None:
        with self.assertRaisesRegex(RUNNER.GateError, "reversed time"):
            RUNNER.parse_raw_turns("[2.00][S01]倒退[1.00]")

    def test_rejects_same_speaker_overlap_or_end_regression(self) -> None:
        with self.assertRaisesRegex(RUNNER.GateError, "overlap exceeds"):
            RUNNER.parse_raw_turns(
                "[0.00][S01]第一段[2.00][1.00][S01]倒退重叠[1.50]"
            )

    def test_rejects_speaker_label_longer_than_two_digits(self) -> None:
        with self.assertRaises(RUNNER.GateError):
            RUNNER.parse_raw_turns("[0.00][S0001]非法标签[1.00]")

    def test_rejects_trailing_material_after_explicit_end(self) -> None:
        with self.assertRaises(RUNNER.GateError):
            RUNNER.parse_raw_turns("[0.10][S01]正文[1.00]尾巴")


class RtfGateTests(unittest.TestCase):
    def test_business_sample_accepts_exactly_one(self) -> None:
        self.assertEqual(RUNNER.validate_rtf("business_737s", 1.0), 1.0)

    def test_business_sample_rejects_above_one(self) -> None:
        with self.assertRaisesRegex(RUNNER.GateError, "RTF gate failed"):
            RUNNER.validate_rtf("business_737s", 1.000001)

    def test_other_sample_has_no_rtf_limit(self) -> None:
        self.assertIsNone(RUNNER.validate_rtf("short_12s", 10.0))


class FrozenInputLockTests(unittest.TestCase):
    def test_read_json_rejects_duplicate_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "ambiguous.json"
            path.write_text('{"stage":"wrong","stage":"MOSS_V3_P1"}', encoding="utf-8")
            with self.assertRaisesRegex(RUNNER.GateError, "duplicate JSON key"):
                RUNNER.read_json(path, "ambiguous test JSON")

    def test_lock_document_matches_exact_schema_and_content(self) -> None:
        lock = RUNNER.read_json(RUNNER.LOCK_PATH, "P1 lock file")
        RUNNER.validate_lock_document(lock)
        changed = json.loads(json.dumps(lock))
        changed["runtime"]["version"] = "0.2.3"
        with self.assertRaisesRegex(RUNNER.GateError, "exact frozen"):
            RUNNER.validate_lock_document(changed)

    def test_source_lock_checks_bytes_and_hash(self) -> None:
        frozen = RUNNER.EXPECTED_SAMPLES["short_12s"]
        args = SimpleNamespace(
            sample_name="short_12s",
            sample_bytes=frozen["bytes"],
            sample_sha256=frozen["sha256"],
        )
        RUNNER.validate_sample_lock(args)
        args.sample_sha256 = "0" * 64
        with self.assertRaisesRegex(RUNNER.GateError, "sample hash"):
            RUNNER.validate_sample_lock(args)

    def test_prepared_lock_checks_independent_canonical_hash(self) -> None:
        frozen = RUNNER.EXPECTED_SAMPLES["real_107s"]["prepared"]
        RUNNER.validate_prepared_lock("real_107s", dict(frozen))
        changed = dict(frozen)
        changed["sha256"] = "0" * 64
        with self.assertRaisesRegex(RUNNER.GateError, "prepared WAV sha256"):
            RUNNER.validate_prepared_lock("real_107s", changed)

    def test_loaded_pcm_requires_exact_frames_and_both_payload_hashes(self) -> None:
        prepared = RUNNER.EXPECTED_SAMPLES["short_12s"]["prepared"]
        record = {
            "samples": prepared["frames"],
            "source_pcm_s16le_sha256": prepared["pcm_s16le_sha256"],
            "float32_sha256": prepared["float32_sha256"],
        }
        RUNNER.validate_pcm_lock("short_12s", record)
        for key, wrong_value in (
            ("samples", record["samples"] - 1),
            ("source_pcm_s16le_sha256", "0" * 64),
            ("float32_sha256", "F" * 64),
        ):
            with self.subTest(key=key):
                changed = dict(record)
                changed[key] = wrong_value
                with self.assertRaisesRegex(RUNNER.GateError, "loaded PCM"):
                    RUNNER.validate_pcm_lock("short_12s", changed)


class RunArgumentTests(unittest.TestCase):
    class RaisingParser:
        @staticmethod
        def error(message: str) -> None:
            raise ValueError(message)

    @staticmethod
    def args(sample_name: str) -> SimpleNamespace:
        return SimpleNamespace(
            model=Path("model.gguf"),
            model_bytes=1,
            model_sha256="0" * 64,
            sample=Path("sample.wav"),
            sample_bytes=1,
            sample_sha256="0" * 64,
            sample_name=sample_name,
            evidence=Path("evidence"),
        )

    def test_dynamic_person_name_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "EXPECTED_SAMPLES"):
            RUNNER.validate_run_arguments(self.RaisingParser(), self.args("Mico"))

    def test_short_and_real_sample_names_are_accepted(self) -> None:
        RUNNER.validate_run_arguments(self.RaisingParser(), self.args("short_12s"))
        RUNNER.validate_run_arguments(self.RaisingParser(), self.args("real_107s"))

    def test_long_sample_is_rejected_by_monolithic_entry(self) -> None:
        with self.assertRaisesRegex(ValueError, "monolithic execution is forbidden"):
            RUNNER.validate_run_arguments(self.RaisingParser(), self.args("long_3096s"))


class EnvironmentGateTests(unittest.TestCase):
    def test_clean_environment_passes(self) -> None:
        self.assertEqual(RUNNER.validate_inference_environment()["state"], "PASS")

    def test_ggml_backend_injection_is_rejected(self) -> None:
        previous = os.environ.get("GGML_BACKEND_PATH")
        os.environ["GGML_BACKEND_PATH"] = r"D:\\untrusted"
        try:
            with self.assertRaisesRegex(RUNNER.GateError, "GGML_BACKEND_PATH"):
                RUNNER.validate_inference_environment()
        finally:
            if previous is None:
                os.environ.pop("GGML_BACKEND_PATH", None)
            else:
                os.environ["GGML_BACKEND_PATH"] = previous

    def test_transcribe_strategy_override_is_rejected(self) -> None:
        previous = os.environ.get("TRANSCRIBE_NO_FLASH")
        os.environ["TRANSCRIBE_NO_FLASH"] = "1"
        try:
            with self.assertRaisesRegex(RUNNER.GateError, "TRANSCRIBE_NO_FLASH"):
                RUNNER.validate_inference_environment()
        finally:
            if previous is None:
                os.environ.pop("TRANSCRIBE_NO_FLASH", None)
            else:
                os.environ["TRANSCRIBE_NO_FLASH"] = previous


class EvidenceTests(unittest.TestCase):
    def test_public_evidence_normalizes_native_capability_tuples(self) -> None:
        public = RUNNER.make_public_evidence(
            {
                "capabilities": {
                    "languages": ("en", "zh"),
                    "translate_target_languages": (),
                }
            }
        )
        self.assertEqual(public["capabilities"]["languages"], ["en", "zh"])
        self.assertEqual(public["capabilities"]["translate_target_languages"], [])
        RUNNER.assert_public_evidence_safe(public)

    def test_public_evidence_directory_is_direct_and_matches_validated_commit(self) -> None:
        commit = "a" * 40
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            with (
                mock.patch.object(RUNNER, "PUBLIC_EVIDENCE_ROOT", root),
                mock.patch.object(RUNNER, "enforce_and_audit_restricted_acl") as acl,
            ):
                accepted = root / f"MOSS-V3-P1-FINAL-{commit[:7]}-20260829"
                self.assertEqual(
                    RUNNER.ensure_evidence_directory(accepted, commit), accepted.resolve()
                )
                self.assertEqual(acl.call_count, 2)
                with self.assertRaisesRegex(RUNNER.GateError, "does not match"):
                    RUNNER.ensure_evidence_directory(
                        root / "MOSS-V3-P1-FINAL-deadbee-20260829", commit
                    )
                with self.assertRaisesRegex(RUNNER.GateError, "direct child"):
                    RUNNER.ensure_evidence_directory(
                        root / "nested" / f"MOSS-V3-P1-FINAL-{commit[:7]}", commit
                    )

    def test_unknown_ascii_keys_and_timestamp_suffix_are_redacted(self) -> None:
        public = RUNNER.make_public_evidence(
            {
                "Mico": 1,
                "customer-123": 2,
                "captured_at": "2026-08-29T00:00:00+08:00 SECRET",
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        self.assertNotIn("Mico", rendered)
        self.assertNotIn("customer-123", rendered)
        self.assertNotIn("SECRET", rendered)
        self.assertNotEqual(public["captured_at"], "2026-08-29T00:00:00+08:00 SECRET")

    def test_unknown_container_is_dropped_instead_of_fingerprinted(self) -> None:
        candidate_hash = "A" * 64
        public = RUNNER.make_public_evidence(
            {
                "unknown_container": {"sha256": candidate_hash},
                "participant_map": {"Mico": 1},
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        self.assertEqual(public, {})
        self.assertNotIn(candidate_hash, rendered)
        self.assertNotIn("Mico", rendered)

    def test_final_public_dto_rejects_fields_appended_after_scrubbing(self) -> None:
        with self.assertRaisesRegex(RUNNER.GateError, "fixed-schema DTO"):
            RUNNER.assert_public_evidence_safe(
                {"status": "PASS", "leaked_after_scrub": "Mico"}
            )

    def test_public_evidence_does_not_leak_dynamic_human_keys_or_turn_timing(self) -> None:
        public = RUNNER.make_public_evidence(
            {
                "participant_map": {"张三": 1, "Mico": 2, "M100": 3},
                "raw_turns": [
                    {"start_seconds": 1.2, "end_seconds": 3.4, "speaker_id": 1}
                ],
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        self.assertNotIn("张三", rendered)
        self.assertNotIn("Mico", rendered)
        self.assertNotIn("M100", rendered)
        self.assertNotIn("start_seconds", rendered)
        self.assertNotIn("speaker_id", rendered)

    def test_public_evidence_removes_raw_content_paths_and_traceback(self) -> None:
        secret = "用户会议里的原始逐字稿"
        full_path = r"D:\\private\\meeting\\audio.wav"
        public = RUNNER.make_public_evidence(
            {
                "text": secret,
                "raw_text": secret + " raw",
                "message": full_path + " failed",
                "traceback": "secret stack " + full_path,
                "path": full_path,
                "nested": [{"sample_path": full_path, "stderr": secret}],
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        self.assertNotIn(secret, rendered)
        self.assertNotIn("private", rendered)
        self.assertNotIn("traceback", rendered)
        self.assertNotIn("stderr", rendered)
        self.assertEqual(public["path"]["suffix"], ".wav")
        RUNNER.assert_public_evidence_safe(public)

    def test_unknown_path_key_and_anchor_are_not_public(self) -> None:
        public = RUNNER.make_public_evidence(
            {
                "arbitrary_path": r"D:\\Users\\liuxin\\meeting.wav",
                "conversion": {"output_path": r"D:\\MeetilyData\\private\\secret.wav"},
                "required_text_anchors": ["SECRET_LITERAL"],
                "failures": ["missing SECRET_LITERAL"],
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        self.assertNotIn(":\\", rendered)
        self.assertNotIn("liuxin", rendered)
        self.assertNotIn("SECRET_LITERAL", rendered)
        RUNNER.assert_public_evidence_safe(public)

    def test_unknown_human_text_fields_are_default_redacted(self) -> None:
        public = RUNNER.make_public_evidence(
            {
                "participant_name": "Mico",
                "business_term": "M100",
                "notes": ["user-secret-sentence"],
                "file_name": "monthly-secret-client-20260829.wav",
                "participants": [{"name": "Ben", "description": "VIP-client-secret"}],
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        for secret in (
            "Mico",
            "M100",
            "user-secret-sentence",
            "monthly-secret-client",
            "Ben",
            "VIP-client-secret",
        ):
            self.assertNotIn(secret, rendered)

    def test_atomic_evidence_is_parseable_hashed_and_never_overwritten(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "evidence.json"
            digest = RUNNER.write_evidence_atomic(target, {"status": "PASS"})
            self.assertEqual(json.loads(target.read_text(encoding="utf-8"))["status"], "PASS")
            sidecar = target.with_suffix(".json.sha256").read_text(encoding="utf-8")
            self.assertIn(digest, sidecar)
            with self.assertRaisesRegex(RUNNER.GateError, "refusing to overwrite"):
                RUNNER.write_evidence_atomic(target, {"status": "FAIL"})

    def test_restricted_attestation_binds_public_and_private_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public_root = root / "public-root"
            private_root = root / "private-root"
            run_name = "MOSS-V3-P1-RUN-aaaaaaa"
            public_dir = public_root / run_name
            private_dir = private_root / run_name
            public_dir.mkdir(parents=True)
            private_dir.mkdir(parents=True)
            public_path = public_dir / "moss-p1-short.json"
            private_path = private_dir / "moss-p1-short.private.json"
            private_hash = RUNNER.write_evidence_atomic(private_path, {"raw_text": "secret"})
            public_hash = RUNNER.write_evidence_atomic(
                public_path,
                {
                    "status": "PASS",
                    "private_evidence": {
                        "bytes": private_path.stat().st_size,
                        "sha256": private_hash,
                        "restricted": True,
                    },
                    "restricted_attestation_required": True,
                },
            )
            with (
                mock.patch.object(RUNNER, "PUBLIC_EVIDENCE_ROOT", public_root.resolve()),
                mock.patch.object(RUNNER, "PRIVATE_EVIDENCE_ROOT", private_root.resolve()),
                mock.patch.object(RUNNER, "audit_restricted_acl"),
            ):
                attestation = RUNNER.write_public_attestation_atomic(
                    private_dir, public_path, public_hash, private_path, private_hash
                )
                payload = json.loads(attestation.read_text(encoding="utf-8"))
                self.assertEqual(payload["public_sha256"], public_hash)
                self.assertEqual(payload["private_sha256"], private_hash)
                self.assertTrue(attestation.with_suffix(".json.sha256").is_file())
                verified = RUNNER.verify_evidence_bundle(
                    public_dir, private_dir, public_path, private_path, attestation
                )
                self.assertEqual(verified["public_sha256"], public_hash)

    def test_bundle_verifier_rejects_missing_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public_root = root / "public-root"
            private_root = root / "private-root"
            run_name = "MOSS-V3-P1-RUN-aaaaaaa"
            public_dir = public_root / run_name
            private_dir = private_root / run_name
            public_dir.mkdir(parents=True)
            private_dir.mkdir(parents=True)
            public_path = public_dir / "moss-p1-short.json"
            private_path = private_dir / "moss-p1-short.private.json"
            private_hash = RUNNER.write_evidence_atomic(private_path, {"raw_text": "secret"})
            public_hash = RUNNER.write_evidence_atomic(
                public_path,
                {
                    "status": "PASS",
                    "private_evidence": {
                        "bytes": private_path.stat().st_size,
                        "sha256": private_hash,
                        "restricted": True,
                    },
                    "restricted_attestation_required": True,
                },
            )
            with (
                mock.patch.object(RUNNER, "PUBLIC_EVIDENCE_ROOT", public_root.resolve()),
                mock.patch.object(RUNNER, "PRIVATE_EVIDENCE_ROOT", private_root.resolve()),
                mock.patch.object(RUNNER, "audit_restricted_acl"),
            ):
                attestation = RUNNER.write_public_attestation_atomic(
                    private_dir, public_path, public_hash, private_path, private_hash
                )
                public_path.with_suffix(".json.sha256").unlink()
                with self.assertRaisesRegex(RUNNER.GateError, "sidecar is missing"):
                    RUNNER.verify_evidence_bundle(
                        public_dir, private_dir, public_path, private_path, attestation
                    )

    def test_bundle_verifier_rejects_missing_private_reference(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            public_root = root / "public-root"
            private_root = root / "private-root"
            run_name = "MOSS-V3-P1-RUN-aaaaaaa"
            public_dir = public_root / run_name
            private_dir = private_root / run_name
            public_dir.mkdir(parents=True)
            private_dir.mkdir(parents=True)
            public_path = public_dir / "moss-p1-short.json"
            private_path = private_dir / "moss-p1-short.private.json"
            private_hash = RUNNER.write_evidence_atomic(private_path, {"raw_text": "secret"})
            public_hash = RUNNER.write_evidence_atomic(public_path, {"status": "PASS"})
            with (
                mock.patch.object(RUNNER, "PUBLIC_EVIDENCE_ROOT", public_root.resolve()),
                mock.patch.object(RUNNER, "PRIVATE_EVIDENCE_ROOT", private_root.resolve()),
                mock.patch.object(RUNNER, "audit_restricted_acl"),
                self.assertRaisesRegex(RUNNER.GateError, "does not bind"),
            ):
                RUNNER.write_public_attestation_atomic(
                    private_dir, public_path, public_hash, private_path, private_hash
                )


if __name__ == "__main__":
    unittest.main()
