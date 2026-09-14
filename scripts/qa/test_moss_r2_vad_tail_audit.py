import importlib.util
import math
import struct
import unittest
import uuid
import wave
from pathlib import Path


SCRIPT = Path(__file__).with_name("moss_r2_vad_tail_audit.py")
SPEC = importlib.util.spec_from_file_location("moss_r2_vad_tail_audit", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class VadTailAuditTests(unittest.TestCase):
    def test_detects_known_tail_and_accepts_matching_model_timestamp(self) -> None:
        source = Path.cwd() / "target" / f"r2-tail-{uuid.uuid4().hex}.wav"
        try:
            sample_rate = 16_000
            with wave.open(str(source), "wb") as audio:
                audio.setnchannels(1)
                audio.setsampwidth(2)
                audio.setframerate(sample_rate)
                samples = []
                for index in range(sample_rate * 2):
                    seconds = index / sample_rate
                    value = (
                        int(8_000 * math.sin(2 * math.pi * 440 * seconds))
                        if 0.5 <= seconds < 1.5
                        else 0
                    )
                    samples.append(value)
                audio.writeframes(struct.pack(f"<{len(samples)}h", *samples))
            digest = MODULE.sha256_file(source)
            result = MODULE.audit(source, digest, 1_500)
            self.assertEqual(result["status"], "PASS")
            self.assertEqual(result["selected_last_active_end_ms"], 1_500)
            self.assertEqual(result["absolute_tail_error_ms"], 0)
        finally:
            source.unlink(missing_ok=True)

    def test_wrong_hash_fails(self) -> None:
        source = Path.cwd() / "target" / f"r2-empty-{uuid.uuid4().hex}.wav"
        try:
            with wave.open(str(source), "wb") as audio:
                audio.setnchannels(1)
                audio.setsampwidth(2)
                audio.setframerate(16_000)
                audio.writeframes(struct.pack("<320h", *([1000] * 320)))
            result = MODULE.audit(source, "0" * 64, 20)
            self.assertEqual(result["status"], "FAIL")
            self.assertFalse(result["checks"]["source_hash_matches"])
        finally:
            source.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
