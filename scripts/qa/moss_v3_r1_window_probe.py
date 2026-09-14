from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCRIPT_PATH = Path(__file__).resolve()
BASE_PATH = SCRIPT_PATH.with_name("moss_v3_p1_run.py")
PRIVATE_ROOT = Path(r"D:\MeetilyData\private-evidence\moss-r1").resolve()
SAMPLE_RATE = 16_000
N_CTX = 4_096


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json_new(path: Path, payload: dict[str, Any]) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
    descriptor = os.open(str(path), flags, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(encoded)
        handle.flush()
        os.fsync(handle.fileno())
    digest = hashlib.sha256(encoded.encode("utf-8")).hexdigest()
    path.with_name(path.name + ".sha256").write_text(
        f"{digest}  {path.name}\n", encoding="ascii"
    )
    return digest


def load_base() -> Any:
    spec = importlib.util.spec_from_file_location("moss_v3_r1_window_base", BASE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("unable_to_import_frozen_base")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run one bounded MOSS R1 text/timestamp window against the frozen full audio."
    )
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--sample", type=Path, required=True)
    parser.add_argument("--start-seconds", type=float, required=True)
    parser.add_argument("--end-seconds", type=float, required=True)
    parser.add_argument("--term", action="append", default=[])
    parser.add_argument("--private-output", type=Path, required=True)
    parser.add_argument("--public-output", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    private_path = args.private_output.resolve()
    if PRIVATE_ROOT not in private_path.parents:
        raise RuntimeError("private_output_outside_fixed_root")
    for path in (args.private_output, args.public_output):
        if path.exists() or path.with_name(path.name + ".sha256").exists():
            raise FileExistsError(path)
    if args.start_seconds < 0 or args.end_seconds <= args.start_seconds:
        raise ValueError("invalid_window")

    base = load_base()
    base.validate_inference_environment()
    base.validate_test_toolchain()
    base.capture_and_validate_host()
    native, runtime = base.load_transcribe_cpp()
    device, inventory = base.select_exact_vulkan_device(native)
    model_record = base.validate_model(
        args.model, base.EXPECTED_MODEL_BYTES, base.EXPECTED_MODEL_SHA256
    )
    sample = base.EXPECTED_SAMPLES["business_737s"]
    base.validate_sample_lock(
        argparse.Namespace(
            sample_name="business_737s",
            sample=args.sample,
            sample_bytes=int(sample["bytes"]),
            sample_sha256=str(sample["sha256"]),
        )
    )
    source_record = base.validate_locked_file(
        args.sample,
        int(sample["bytes"]),
        str(sample["sha256"]),
        "R1 window source",
    )
    prepared, conversion = base.prepare_audio("business_737s", source_record)
    pcm, pcm_record = base.load_pcm_float32(Path(str(prepared["path"])))
    base.validate_pcm_lock("business_737s", pcm_record)
    source_duration = float(prepared["duration_seconds"])
    if args.end_seconds > source_duration:
        raise ValueError("window_exceeds_source")
    start_sample = round(args.start_seconds * SAMPLE_RATE)
    end_sample = round(args.end_seconds * SAMPLE_RATE)
    window_pcm = pcm[start_sample:end_sample]
    window_duration = len(window_pcm) / SAMPLE_RATE

    started = datetime.now(timezone.utc).isoformat()
    wall_started = time.perf_counter()
    with native.Model(args.model, backend="vulkan", device=device) as model:
        with model.session(n_threads=0, kv_type="auto", n_ctx=N_CTX) as session:
            limits = session.limits
            if round(window_duration * 1000) > int(limits.effective_max_audio_ms):
                raise RuntimeError("window_exceeds_session_limit")
            inference_started = time.perf_counter()
            native_result = session.run(
                window_pcm,
                language="zh",
                timestamps="segment",
                diarize="on",
            )
            inference_seconds = time.perf_counter() - inference_started
    result = base.result_record(native_result)
    raw_turns = result.get("raw_turns")
    if not isinstance(raw_turns, list) or not raw_turns:
        raise RuntimeError("window_has_no_raw_turns")
    global_turns = []
    for turn in raw_turns:
        copied = dict(turn)
        copied["start_seconds"] = float(copied["start_seconds"]) + args.start_seconds
        copied["end_seconds"] = float(copied["end_seconds"]) + args.start_seconds
        global_turns.append(copied)

    private_payload = {
        "schema_version": 1,
        "role": "MOSS_V3_R1_WINDOW_PRIVATE_OUTPUT",
        "status": "COMPLETED",
        "started_at": started,
        "finished_at": datetime.now(timezone.utc).isoformat(),
        "wall_seconds": time.perf_counter() - wall_started,
        "inference_seconds": inference_seconds,
        "inference_rtf": inference_seconds / window_duration,
        "configuration": {
            "backend": "vulkan",
            "n_ctx": N_CTX,
            "source_start_seconds": args.start_seconds,
            "source_end_seconds": args.end_seconds,
            "window_duration_seconds": window_duration,
        },
        "model": model_record,
        "source": source_record,
        "prepared": prepared,
        "conversion": conversion,
        "pcm_load": pcm_record,
        "runtime": runtime.get("loaded"),
        "device_inventory": inventory,
        "output": result,
        "global_turns": global_turns,
        "release_go": False,
    }
    private_sha = write_json_new(args.private_output, private_payload)
    text = "".join(str(turn.get("text", "")) for turn in global_turns)
    term_counts = {
        term: text.casefold().count(str(term).casefold()) for term in args.term
    }
    public_payload = {
        "schema_version": 1,
        "role": "MOSS_V3_R1_WINDOW_PUBLIC_EVIDENCE",
        "status": "COMPLETED",
        "window": {
            "source_start_seconds": args.start_seconds,
            "source_end_seconds": args.end_seconds,
            "duration_seconds": window_duration,
        },
        "configuration": {"backend": "vulkan", "n_ctx": N_CTX},
        "output_summary": {
            "turn_count": len(global_turns),
            "speaker_count": len(
                {str(turn.get("speaker_label", "")) for turn in global_turns}
            ),
            "first_start_seconds": min(
                float(turn["start_seconds"]) for turn in global_turns
            ),
            "last_end_seconds": max(float(turn["end_seconds"]) for turn in global_turns),
            "term_counts": term_counts,
        },
        "performance": {
            "inference_seconds": inference_seconds,
            "inference_rtf": inference_seconds / window_duration,
        },
        "private_evidence": {
            "file_name": args.private_output.name,
            "bytes": args.private_output.stat().st_size,
            "sha256": private_sha,
            "restricted": True,
        },
        "model_sha256": model_record["sha256"],
        "audio_sha256": source_record["sha256"],
        "runner_sha256": sha256_file(SCRIPT_PATH),
        "transcript_text_included": False,
    }
    public_sha = write_json_new(args.public_output, public_payload)
    print(
        json.dumps(
            {
                "status": "COMPLETED",
                "term_counts": term_counts,
                "turn_count": len(global_turns),
                "inference_rtf": private_payload["inference_rtf"],
                "private_sha256": private_sha,
                "public_sha256": public_sha,
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
