#!/usr/bin/env python3
from __future__ import annotations

import argparse
import dataclasses
import hashlib
import importlib.util
import json
import math
import os
import sys
import time
import traceback
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType
from typing import Any


SCRIPT_PATH = Path(__file__).resolve()
BASE_PATH = SCRIPT_PATH.with_name("moss_v3_p1_run.py")
PRIVATE_ROOT = Path(r"D:\MeetilyData\private-evidence\moss-r1").resolve()
N_CTX = 32_768
N_THREADS = 0
KV_TYPE = "auto"
BACKEND = "cpu"
SYSTEM_MEMORY_MARGIN_BYTES = 4 * 1024 * 1024 * 1024


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_base() -> ModuleType:
    spec = importlib.util.spec_from_file_location("moss_v3_p1_base_for_r1", BASE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("unable_to_import_frozen_p1_base")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def atomic_json_new(path: Path, payload: dict[str, Any]) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    descriptor = os.open(str(path), flags, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
    except Exception:
        try:
            path.unlink(missing_ok=True)
        except OSError:
            pass
        raise
    digest = hashlib.sha256(encoded.encode("utf-8")).hexdigest()
    sidecar = path.with_name(path.name + ".sha256")
    sidecar.write_text(f"{digest}  {path.name}\n", encoding="ascii")
    return digest


def require_new_file(path: Path, label: str) -> None:
    if path.exists() or path.with_name(path.name + ".sha256").exists():
        raise RuntimeError(f"{label}_already_exists")


def validate_private_output_path(path: Path) -> Path:
    resolved = path.resolve()
    if PRIVATE_ROOT not in resolved.parents:
        raise RuntimeError("private_output_outside_fixed_root")
    return resolved


def ensure_private_output(path: Path) -> None:
    resolved = validate_private_output_path(path)
    PRIVATE_ROOT.mkdir(parents=True, exist_ok=True)
    resolved.parent.mkdir(parents=True, exist_ok=True)


def select_device(
    base: ModuleType, module: Any, backend: str
) -> tuple[Any, dict[str, Any]]:
    if backend == "vulkan":
        return base.select_exact_vulkan_device(module)
    devices = list(module.backends())
    records = [base.device_record(device) for device in devices]
    matches = [
        device
        for device in devices
        if str(getattr(device, "kind", "")).casefold() == "cpu"
        and str(getattr(device, "device_type", "")).casefold() == "cpu"
    ]
    if not module.backend_available(backend):
        raise RuntimeError("cpu_backend_unavailable")
    if len(matches) != 1:
        raise RuntimeError(f"expected_exactly_one_cpu_device_got_{len(matches)}")
    return matches[0], {
        "selection_rule": "exactly_one_registered_cpu_device",
        "registered_devices": records,
        "selected": base.device_record(matches[0]),
    }


def make_public_payload(
    *,
    status: str,
    started_at: str,
    finished_at: str,
    wall_seconds: float,
    model: dict[str, Any],
    source: dict[str, Any],
    prepared: dict[str, Any],
    session_limits: dict[str, Any],
    admission: dict[str, Any],
    result_record: dict[str, Any] | None,
    output_gate: dict[str, Any] | None,
    memory: dict[str, Any] | None,
    private_path: Path | None,
    private_sha256: str | None,
    error: dict[str, Any] | None,
    backend: str = BACKEND,
    n_ctx: int = N_CTX,
) -> dict[str, Any]:
    segment_count = 0
    speaker_count = 0
    first_start = None
    last_end = None
    normalized_characters = None
    if isinstance(result_record, dict):
        raw_turns = result_record.get("raw_turns")
        if isinstance(raw_turns, list):
            segment_count = len(raw_turns)
            labels = {
                str(item.get("speaker_label", ""))
                for item in raw_turns
                if isinstance(item, dict) and str(item.get("speaker_label", ""))
            }
            speaker_count = len(labels)
            starts = [
                float(item["start_seconds"])
                for item in raw_turns
                if isinstance(item, dict) and item.get("start_seconds") is not None
            ]
            ends = [
                float(item["end_seconds"])
                for item in raw_turns
                if isinstance(item, dict) and item.get("end_seconds") is not None
            ]
            first_start = min(starts) if starts else None
            last_end = max(ends) if ends else None
        text_value = str(result_record.get("text", ""))
        normalized_characters = sum(character.isalnum() for character in text_value)
    return {
        "schema_version": 1,
        "role": "MOSS_V3_R1_MONOLITHIC_32K_PUBLIC_EVIDENCE",
        "status": status,
        "started_at": started_at,
        "finished_at": finished_at,
        "wall_seconds": wall_seconds,
        "configuration": {
            "n_ctx": n_ctx,
            "n_threads": N_THREADS,
            "kv_type": KV_TYPE,
            "backend": backend,
            "language": "zh",
            "timestamps": "segment",
            "diarize": "on",
        },
        "model": {
            "bytes": model.get("bytes"),
            "sha256": model.get("sha256"),
        },
        "source": {
            "bytes": source.get("bytes"),
            "sha256": source.get("sha256"),
            "duration_seconds": prepared.get("duration_seconds"),
        },
        "session_limits": session_limits,
        "memory_admission": admission,
        "output_summary": {
            "turn_count": segment_count,
            "speaker_label_count": speaker_count,
            "first_turn_seconds": first_start,
            "last_turn_seconds": last_end,
            "normalized_character_count": normalized_characters,
            "structural_gate_state": (
                output_gate.get("state") if isinstance(output_gate, dict) else None
            ),
        },
        "memory": memory,
        "private_evidence": (
            {
                "file_name": private_path.name,
                "bytes": private_path.stat().st_size,
                "sha256": private_sha256,
                "restricted": True,
            }
            if private_path is not None
            and private_path.is_file()
            and private_sha256 is not None
            else None
        ),
        "error": error,
        "transcript_text_included": False,
        "runner": {
            "path_sha256": hashlib.sha256(str(SCRIPT_PATH).encode("utf-8")).hexdigest(),
            "file_sha256": sha256_file(SCRIPT_PATH),
            "frozen_p1_base_sha256": sha256_file(BASE_PATH),
        },
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run the frozen 737.728s MOSS sample in one bounded 32768-context session."
    )
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--sample", type=Path, required=True)
    parser.add_argument("--preflight-output", type=Path, required=True)
    parser.add_argument("--private-output", type=Path, required=True)
    parser.add_argument("--public-output", type=Path, required=True)
    parser.add_argument("--backend", choices=("cpu", "vulkan"), default=BACKEND)
    parser.add_argument("--n-ctx", type=int, choices=(16_384, 32_768), default=N_CTX)
    return parser


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8")
    args = build_parser().parse_args()
    backend = str(args.backend)
    n_ctx = int(args.n_ctx)
    for path, label in (
        (args.preflight_output, "preflight_output"),
        (args.private_output, "private_output"),
        (args.public_output, "public_output"),
    ):
        require_new_file(path, label)
    ensure_private_output(args.private_output)
    args.preflight_output.parent.mkdir(parents=True, exist_ok=True)
    args.public_output.parent.mkdir(parents=True, exist_ok=True)

    base = load_base()
    started_at = now_iso()
    wall_started = time.perf_counter()
    monitor: Any = None
    model_record: dict[str, Any] = {}
    source_record: dict[str, Any] = {}
    prepared_record: dict[str, Any] = {}
    session_limits: dict[str, Any] = {}
    admission: dict[str, Any] = {}
    result_record: dict[str, Any] | None = None
    output_gate: dict[str, Any] | None = None
    memory: dict[str, Any] | None = None
    try:
        inference_environment = base.validate_inference_environment()
        frozen_toolchain = base.validate_test_toolchain()
        host = base.capture_and_validate_host()
        module, runtime = base.load_transcribe_cpp()
        device, device_inventory = select_device(base, module, backend)
        model_record = base.validate_model(
            args.model, base.EXPECTED_MODEL_BYTES, base.EXPECTED_MODEL_SHA256
        )
        sample = base.EXPECTED_SAMPLES["business_737s"]
        sample_args = argparse.Namespace(
            sample_name="business_737s",
            sample=args.sample,
            sample_bytes=int(sample["bytes"]),
            sample_sha256=str(sample["sha256"]),
        )
        base.validate_sample_lock(sample_args)
        source_record = base.validate_locked_file(
            args.sample,
            int(sample["bytes"]),
            str(sample["sha256"]),
            "R1 frozen full source sample",
        )

        monitor = base.MemoryMonitor()
        monitor.start()
        prepared_record, conversion = base.prepare_audio(
            "business_737s", source_record
        )
        activity = base.probe_audio_activity(
            Path(str(prepared_record["path"])),
            float(prepared_record["duration_seconds"]),
        )
        pcm, pcm_record = base.load_pcm_float32(Path(str(prepared_record["path"])))
        base.validate_pcm_lock("business_737s", pcm_record)

        model_started = time.perf_counter()
        with module.Model(args.model, backend=backend, device=device) as model:
            model_load_seconds = time.perf_counter() - model_started
            if (
                model.device != device
                or str(getattr(model.device, "kind", "")).casefold() != backend
                or str(model.backend).casefold() not in {
                backend,
                str(device.name).casefold(),
                }
            ):
                raise RuntimeError("model_did_not_stay_on_selected_cpu_device")
            with model.session(
                n_threads=N_THREADS, kv_type=KV_TYPE, n_ctx=n_ctx
            ) as session:
                session_limits = dataclasses.asdict(session.limits)
                if int(session_limits.get("effective_n_ctx", 0)) != n_ctx:
                    raise RuntimeError("session_effective_n_ctx_mismatch")
                max_audio_ms = int(session_limits.get("effective_max_audio_ms", 0))
                duration_ms = int(round(float(prepared_record["duration_seconds"]) * 1000))
                if max_audio_ms <= 0 or duration_ms > max_audio_ms:
                    raise RuntimeError("full_audio_exceeds_bounded_session_limit")
                psutil = sys.modules.get("psutil")
                if psutil is None:
                    raise RuntimeError("frozen_psutil_not_loaded")
                available = int(psutil.virtual_memory().available)
                max_kv_bytes = int(session_limits.get("max_kv_bytes", 0))
                required = max_kv_bytes + SYSTEM_MEMORY_MARGIN_BYTES
                if max_kv_bytes <= 0 or available < required:
                    raise RuntimeError("insufficient_memory_for_bounded_monolithic_session")
                admission = {
                    "status": "PASS",
                    "available_system_bytes": available,
                    "required_system_bytes": required,
                    "max_kv_bytes": max_kv_bytes,
                    "duration_ms": duration_ms,
                    "maximum_audio_ms": max_audio_ms,
                }
                preflight = {
                    "schema_version": 1,
                    "role": "MOSS_V3_R1_MONOLITHIC_32K_PREFLIGHT",
                    "status": "READY_FOR_INFERENCE",
                    "created_at": now_iso(),
                    "configuration": {
                        "n_ctx": n_ctx,
                        "n_threads": N_THREADS,
                        "kv_type": KV_TYPE,
                        "backend": backend,
                    },
                    "model": base.file_record(args.model),
                    "source": base.file_record(args.sample),
                    "prepared": {
                        "bytes": prepared_record.get("bytes"),
                        "sha256": prepared_record.get("sha256"),
                        "duration_seconds": prepared_record.get("duration_seconds"),
                    },
                    "session_limits": session_limits,
                    "memory_admission": admission,
                    "host_fingerprint_sha256": host.get("fingerprint_sha256"),
                    "inference_environment": inference_environment,
                    "runtime_version": runtime.get("version"),
                    "device_count": len(device_inventory["registered_devices"]),
                    "device_inventory": device_inventory,
                    "frozen_toolchain_state": "PASS",
                    "runner_sha256": sha256_file(SCRIPT_PATH),
                    "frozen_p1_base_sha256": sha256_file(BASE_PATH),
                    "model_load_seconds": model_load_seconds,
                    "transcript_text_included": False,
                }
                atomic_json_new(args.preflight_output, preflight)
                inference_started = time.perf_counter()
                native_result = session.run(
                    pcm,
                    language="zh",
                    timestamps="segment",
                    diarize="on",
                )
                inference_seconds = time.perf_counter() - inference_started
        del pcm
        result_record = base.result_record(native_result)
        del native_result
        try:
            output_gate = base.validate_output(
                "business_737s",
                result_record,
                float(prepared_record["duration_seconds"]),
                activity,
            )
            structural_status = "PASS"
            structural_error = None
        except Exception as gate_exc:
            structural_status = "FAIL"
            structural_error = {
                "type": type(gate_exc).__name__,
                "message": str(gate_exc),
            }
        if monitor is not None:
            memory = monitor.stop()
            monitor = None
        finished_at = now_iso()
        private_payload = {
            "schema_version": 1,
            "role": "MOSS_V3_R1_MONOLITHIC_32K_PRIVATE_OUTPUT",
            "status": "COMPLETED",
            "structural_status": structural_status,
            "structural_error": structural_error,
            "started_at": started_at,
            "finished_at": finished_at,
            "wall_seconds": time.perf_counter() - wall_started,
            "inference_seconds": inference_seconds,
            "inference_rtf": inference_seconds
            / float(prepared_record["duration_seconds"]),
            "configuration": {
                "n_ctx": n_ctx,
                "n_threads": N_THREADS,
                "kv_type": KV_TYPE,
                "backend": backend,
            },
            "model": model_record,
            "source": source_record,
            "prepared": prepared_record,
            "conversion": conversion,
            "activity": activity,
            "pcm_load": pcm_record,
            "session_limits": session_limits,
            "memory_admission": admission,
            "output": result_record,
            "output_gate": output_gate,
            "memory": memory,
            "post_integrity": {
                "model": base.validate_locked_file(
                    args.model,
                    base.EXPECTED_MODEL_BYTES,
                    base.EXPECTED_MODEL_SHA256,
                    "R1 model after inference",
                ),
                "source": base.validate_locked_file(
                    args.sample,
                    int(sample["bytes"]),
                    str(sample["sha256"]),
                    "R1 source after inference",
                ),
            },
            "runner": {
                "path": str(SCRIPT_PATH),
                "sha256": sha256_file(SCRIPT_PATH),
                "frozen_p1_base_path": str(BASE_PATH),
                "frozen_p1_base_sha256": sha256_file(BASE_PATH),
            },
            "release_go": False,
        }
        private_sha = atomic_json_new(args.private_output, private_payload)
        public_payload = make_public_payload(
            status="COMPLETED",
            started_at=started_at,
            finished_at=finished_at,
            wall_seconds=time.perf_counter() - wall_started,
            model=model_record,
            source=source_record,
            prepared=prepared_record,
            session_limits=session_limits,
            admission=admission,
            result_record=result_record,
            output_gate=output_gate,
            memory=memory,
            private_path=args.private_output,
            private_sha256=private_sha,
            error=structural_error,
            backend=backend,
            n_ctx=n_ctx,
        )
        public_sha = atomic_json_new(args.public_output, public_payload)
        print(
            json.dumps(
                {
                    "status": "COMPLETED",
                    "structural_status": structural_status,
                    "public_sha256": public_sha,
                    "private_sha256": private_sha,
                    "turn_count": public_payload["output_summary"]["turn_count"],
                    "speaker_label_count": public_payload["output_summary"][
                        "speaker_label_count"
                    ],
                    "inference_rtf": private_payload["inference_rtf"],
                },
                ensure_ascii=False,
            )
        )
        return 0
    except (Exception, KeyboardInterrupt) as exc:
        if monitor is not None:
            try:
                memory = monitor.stop()
            except Exception:
                memory = None
        finished_at = now_iso()
        error = {
            "type": type(exc).__name__,
            "message": str(exc),
            "traceback_sha256": hashlib.sha256(
                "".join(
                    traceback.format_exception(type(exc), exc, exc.__traceback__)
                ).encode("utf-8")
            ).hexdigest(),
        }
        failure_private = {
            "schema_version": 1,
            "role": "MOSS_V3_R1_MONOLITHIC_32K_PRIVATE_OUTPUT",
            "status": "FAIL",
            "started_at": started_at,
            "finished_at": finished_at,
            "wall_seconds": time.perf_counter() - wall_started,
            "configuration": {
                "n_ctx": n_ctx,
                "n_threads": N_THREADS,
                "kv_type": KV_TYPE,
                "backend": backend,
            },
            "model": model_record,
            "source": source_record,
            "prepared": prepared_record,
            "session_limits": session_limits,
            "memory_admission": admission,
            "memory": memory,
            "error": error,
            "release_go": False,
        }
        private_sha = None
        if not args.private_output.exists():
            private_sha = atomic_json_new(args.private_output, failure_private)
        public_payload = make_public_payload(
            status="FAIL",
            started_at=started_at,
            finished_at=finished_at,
            wall_seconds=time.perf_counter() - wall_started,
            model=model_record,
            source=source_record,
            prepared=prepared_record,
            session_limits=session_limits,
            admission=admission,
            result_record=None,
            output_gate=None,
            memory=memory,
            private_path=args.private_output if args.private_output.exists() else None,
            private_sha256=private_sha,
            error=error,
            backend=backend,
            n_ctx=n_ctx,
        )
        if not args.public_output.exists():
            atomic_json_new(args.public_output, public_payload)
        print(
            json.dumps(
                {"status": "FAIL", "error_type": error["type"], "error": error["message"]},
                ensure_ascii=False,
            )
        )
        return 2


if __name__ == "__main__":
    sys.exit(main())
