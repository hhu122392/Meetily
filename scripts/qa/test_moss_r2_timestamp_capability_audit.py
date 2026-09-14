import importlib.util
import unittest
import uuid
from pathlib import Path


SCRIPT = Path(__file__).with_name("moss_r2_timestamp_capability_audit.py")
SPEC = importlib.util.spec_from_file_location("moss_r2_timestamp_capability_audit", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class TimestampCapabilityAuditTests(unittest.TestCase):
    def test_write_new_json_refuses_to_replace_evidence(self) -> None:
        output = Path.cwd() / "target" / f"r2-capability-{uuid.uuid4().hex}.json"
        try:
            MODULE.write_new_json(output, {"status": "PASS"})
            with self.assertRaises(FileExistsError):
                MODULE.write_new_json(output, {"status": "FAIL"})
        finally:
            output.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
