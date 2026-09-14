from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path


SUPERVISOR_PATH = Path(__file__).with_name("moss_v3_p1_supervise.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_p1_supervise", SUPERVISOR_PATH)
assert SPEC is not None and SPEC.loader is not None
SUPERVISOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SUPERVISOR
SPEC.loader.exec_module(SUPERVISOR)


def sleeping_command(seconds: float = 30.0) -> list[str]:
    # The body is deliberately supplied only to the child and never returned in
    # public supervisor evidence.
    return [sys.executable, "-c", f"import time; time.sleep({seconds!r})"]


class MemoryAdmissionTests(unittest.TestCase):
    def test_3096_second_kv_estimate_is_about_12_69_gib(self) -> None:
        estimate = SUPERVISOR.estimate_memory(3096.0)
        self.assertAlmostEqual(estimate.kv_bytes / SUPERVISOR.GIB, 12.69, places=2)
        self.assertEqual(
            estimate.kv_bytes,
            3096 * SUPERVISOR.KV_BYTES_PER_AUDIO_SECOND,
        )
        self.assertGreater(estimate.required_bytes, estimate.kv_bytes)
        self.assertEqual(
            estimate.required_bytes - estimate.kv_bytes,
            SUPERVISOR.MODEL_RESIDENT_BYTES
            + SUPERVISOR.RUNTIME_WORKING_SET_RESERVE_BYTES
            + SUPERVISOR.SAFETY_MARGIN_BYTES,
        )

    def test_3096_second_monolith_is_rejected_even_with_large_free_memory(self) -> None:
        admission = SUPERVISOR.assess_memory_admission(
            3096.0, 64 * SUPERVISOR.GIB
        )
        self.assertFalse(admission.admitted)
        self.assertEqual(
            admission.reason_code, "P1_MONOLITHIC_MEMORY_REJECTED"
        )
        self.assertIn(
            "required_with_safety_exceeds_validated_single_process_budget",
            admission.reasons,
        )

    def test_short_chunk_can_pass_memory_admission_with_enough_memory(self) -> None:
        admission = SUPERVISOR.assess_memory_admission(
            600.0, 16 * SUPERVISOR.GIB
        )
        self.assertTrue(admission.admitted)
        self.assertEqual(admission.reason_code, "P1_MEMORY_ADMISSION_ACCEPTED")


class ChunkPlanTests(unittest.TestCase):
    def test_default_plan_has_600_second_chunks_and_one_second_overlap(self) -> None:
        plan = SUPERVISOR.build_chunk_plan(3096.0)
        self.assertEqual(plan["chunk_seconds"], 600.0)
        self.assertEqual(plan["overlap_seconds"], 1.0)
        self.assertEqual(plan["chunk_count"], 6)
        self.assertEqual(plan["execution_status"], "NOT_RUN")
        self.assertEqual(plan["inference_verdict"], "NOT_EVALUATED")
        self.assertTrue(plan["plan_is_not_inference_evidence"])

        chunks = plan["chunks"]
        self.assertEqual((chunks[0]["start_seconds"], chunks[0]["end_seconds"]), (0.0, 600.0))
        self.assertEqual((chunks[1]["start_seconds"], chunks[1]["end_seconds"]), (599.0, 1199.0))
        self.assertEqual(chunks[-1]["start_seconds"], 2995.0)
        self.assertEqual(chunks[-1]["end_seconds"], 3096.0)
        self.assertTrue(all(item["execution_status"] == "NOT_RUN" for item in chunks))
        self.assertTrue(all(item["inference_verdict"] == "NOT_EVALUATED" for item in chunks))

    def test_rejects_overlap_equal_to_chunk_size(self) -> None:
        with self.assertRaisesRegex(SUPERVISOR.SupervisorError, "smaller"):
            SUPERVISOR.build_chunk_plan(1000.0, 600.0, 600.0)


@unittest.skipUnless(SUPERVISOR.psutil is not None, "psutil is required for tree tests")
class ProcessSupervisionTests(unittest.TestCase):
    def assert_no_residuals(self, result: object) -> None:
        self.assertEqual(result.residual_processes_after_cleanup, ())

    def test_timeout_terminates_short_sleep_process_tree(self) -> None:
        result = SUPERVISOR.supervise_command(
            sleeping_command(),
            timeout_seconds=0.25,
            minimum_available_memory_bytes=0,
            poll_seconds=0.02,
            residual_grace_seconds=0.05,
        )
        self.assertEqual(result.termination_reason, "timeout")
        self.assert_no_residuals(result)

    def test_cancellation_terminates_short_sleep_process_tree(self) -> None:
        cancel = threading.Event()
        timer = threading.Timer(0.20, cancel.set)
        timer.start()
        try:
            result = SUPERVISOR.supervise_command(
                sleeping_command(),
                timeout_seconds=10.0,
                minimum_available_memory_bytes=0,
                poll_seconds=0.02,
                cancel_event=cancel,
                residual_grace_seconds=0.05,
            )
        finally:
            timer.cancel()
        self.assertEqual(result.termination_reason, "cancelled")
        self.assert_no_residuals(result)

    def test_low_available_memory_terminates_process_tree(self) -> None:
        result = SUPERVISOR.supervise_command(
            sleeping_command(),
            timeout_seconds=10.0,
            minimum_available_memory_bytes=1,
            poll_seconds=0.02,
            available_memory_provider=lambda: 0,
            residual_grace_seconds=0.05,
        )
        self.assertEqual(result.termination_reason, "low_available_memory")
        self.assertEqual(result.minimum_available_memory_bytes, 0)
        self.assert_no_residuals(result)

    def test_residual_process_check_observes_then_clears_sleep_process(self) -> None:
        options = SUPERVISOR._popen_creation_options()
        process = subprocess.Popen(
            sleeping_command(),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            **options,
        )
        identities = SUPERVISOR.discover_process_tree(process.pid)
        self.assertIn(process.pid, SUPERVISOR.check_residual_processes(identities))
        SUPERVISOR.terminate_process_tree(process.pid, identities)
        try:
            process.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5.0)
        deadline = time.monotonic() + 3.0
        residuals = SUPERVISOR.check_residual_processes(identities)
        while residuals and time.monotonic() < deadline:
            time.sleep(0.02)
            residuals = SUPERVISOR.check_residual_processes(identities)
        self.assertEqual(residuals, ())

    @unittest.skipUnless(os.name == "nt", "Windows Job Object test")
    def test_hard_killing_supervisor_kills_child_via_job_object(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            child_pid_file = Path(directory) / "child.pid"
            launcher = Path(directory) / "launcher.py"
            launcher.write_text(
                "\n".join(
                    [
                        "import importlib.util, sys",
                        f"path = {str(SUPERVISOR_PATH)!r}",
                        "spec = importlib.util.spec_from_file_location('forced_exit_supervisor', path)",
                        "module = importlib.util.module_from_spec(spec)",
                        "sys.modules[spec.name] = module",
                        "spec.loader.exec_module(module)",
                        "command = [sys.executable, '-c', "
                        + repr(
                            "import os,time,pathlib; "
                            "time.sleep(0.5); "
                            f"pathlib.Path({str(child_pid_file)!r}).write_text(str(os.getpid())); "
                            "time.sleep(60)"
                        )
                        + "]",
                        "module.supervise_command(command, timeout_seconds=120, "
                        "minimum_available_memory_bytes=0, poll_seconds=0.02)",
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            supervisor = subprocess.Popen(
                [sys.executable, str(launcher)],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                close_fds=True,
                **SUPERVISOR._popen_creation_options(),
            )
            deadline = time.monotonic() + 10.0
            while not child_pid_file.exists() and time.monotonic() < deadline:
                if supervisor.poll() is not None:
                    self.fail(f"supervisor exited early with {supervisor.returncode}")
                time.sleep(0.05)
            self.assertTrue(child_pid_file.exists(), "child did not publish its PID")
            child_pid = int(child_pid_file.read_text(encoding="utf-8"))
            self.assertTrue(SUPERVISOR.psutil.pid_exists(child_pid))

            subprocess.run(
                [str(SUPERVISOR.TASKKILL_PATH), "/PID", str(supervisor.pid), "/F"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
            supervisor.wait(timeout=10)
            deadline = time.monotonic() + 10.0
            while SUPERVISOR.psutil.pid_exists(child_pid) and time.monotonic() < deadline:
                time.sleep(0.05)
            if SUPERVISOR.psutil.pid_exists(child_pid):
                SUPERVISOR.terminate_process_tree(child_pid)
            self.assertFalse(
                SUPERVISOR.psutil.pid_exists(child_pid),
                "kernel job did not clean the child after supervisor hard exit",
            )


class EvidenceTests(unittest.TestCase):
    def test_public_evidence_has_no_full_path_body_command_or_traceback(self) -> None:
        private_path = r"D:\private\meeting\audio.wav"
        body = "用户会议完整正文"
        public = SUPERVISOR.make_public_evidence(
            {
                "sample_path": private_path,
                "command": [private_path, "--secret", body],
                "stdout": body,
                "stderr": body,
                "text": body,
                "message": private_path + " failed",
                "traceback": "stack " + private_path + body,
                "safe": "P1_MONOLITHIC_MEMORY_REJECTED",
            }
        )
        rendered = json.dumps(public, ensure_ascii=False)
        self.assertNotIn(r"D:\private", rendered)
        self.assertNotIn(body, rendered)
        self.assertNotIn("traceback", rendered.lower())
        self.assertNotIn("--secret", rendered)
        self.assertEqual(public["sample_path"]["file_name"], "audio.wav")
        self.assertEqual(public["safe"], "P1_MONOLITHIC_MEMORY_REJECTED")

    def test_atomic_evidence_and_sha_sidecar_are_exclusive(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "supervisor.json"
            digest = SUPERVISOR.write_public_evidence_atomic(
                target,
                {
                    "status": "PLAN_ONLY",
                    "sample_path": r"D:\private\audio.wav",
                    "text": "private transcript",
                },
            )
            parsed = json.loads(target.read_text(encoding="utf-8"))
            self.assertEqual(parsed["status"], "PLAN_ONLY")
            self.assertNotIn("D:\\private", target.read_text(encoding="utf-8"))
            self.assertNotIn("private transcript", target.read_text(encoding="utf-8"))
            sidecar = target.with_suffix(".json.sha256")
            self.assertEqual(
                sidecar.read_text(encoding="utf-8"),
                f"{digest}  {target.name}\n",
            )
            self.assertEqual(digest, SUPERVISOR.sha256_file(target))
            with self.assertRaisesRegex(SUPERVISOR.SupervisorError, "overwrite"):
                SUPERVISOR.write_public_evidence_atomic(
                    target, {"status": "SHOULD_NOT_OVERWRITE"}
                )


if __name__ == "__main__":
    unittest.main()
