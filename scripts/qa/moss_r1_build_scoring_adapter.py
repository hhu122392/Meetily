from __future__ import annotations

import argparse
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


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


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Adapt one monolithic MOSS R1 output to the frozen local P0-R scorer schema."
    )
    parser.add_argument("--monolithic", type=Path, required=True)
    parser.add_argument("--base-manifest", type=Path, required=True)
    parser.add_argument("--private-adapter", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--audit", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    monolithic = load_json(args.monolithic)
    base_manifest = load_json(args.base_manifest)
    if monolithic.get("status") != "COMPLETED":
        raise ValueError("monolithic output is not completed")
    if monolithic.get("structural_status") != "PASS":
        raise ValueError("monolithic structural status is not PASS")
    output = monolithic.get("output")
    if not isinstance(output, dict):
        raise ValueError("monolithic output object is missing")
    raw_turns = output.get("raw_turns")
    if not isinstance(raw_turns, list) or not raw_turns:
        raise ValueError("monolithic raw turns are missing")

    global_turns: list[dict[str, Any]] = []
    for index, turn in enumerate(raw_turns):
        if not isinstance(turn, dict):
            raise ValueError(f"invalid raw turn at index {index}")
        start = float(turn["start_seconds"])
        end = float(turn["end_seconds"])
        if start < 0 or end <= start:
            raise ValueError(f"invalid raw turn timestamp at index {index}")
        global_turns.append(
            {
                "chunk_index": 0,
                "global_start_ms": int(round(start * 1000)),
                "global_end_ms": int(round(end * 1000)),
                "speaker_label": str(turn.get("speaker_label", "")),
                "speaker_id": turn.get("speaker_id"),
                "text": str(turn.get("text", "")),
            }
        )

    inference_rtf = float(monolithic["inference_rtf"])
    adapter = {
        "schema_version": 1,
        "role": "MOSS_V3_R1_MONOLITHIC_P0R_PRIVATE_SCORING_ADAPTER",
        "stage": "R1_MONOLITHIC_VULKAN16K",
        "run_state": "COMPLETED",
        "release_go": False,
        "cross_chunk_speaker_identity_proven": True,
        "speaker_identity_scope": "one_full_audio_session_no_chunk_boundary",
        "global_turns": global_turns,
        "global_gate": {
            "state": "PASS",
            "total_rtf": inference_rtf,
            "source_structural_status": monolithic.get("structural_status"),
        },
        "source_binding": {
            "path": str(args.monolithic.resolve()),
            "bytes": args.monolithic.stat().st_size,
            "sha256": sha256_file(args.monolithic),
            "model_sha256": monolithic.get("model", {}).get("sha256"),
            "audio_sha256": monolithic.get("source", {}).get("sha256"),
            "configuration": monolithic.get("configuration"),
        },
        "adapter_rule": "raw turns copied without text or timestamp modification; seconds converted to rounded milliseconds",
    }
    adapter_sha = write_json_new(args.private_adapter, adapter)

    entries = base_manifest.get("entries")
    if not isinstance(entries, list):
        raise ValueError("base manifest entries are missing")
    replaced = 0
    new_entries: list[dict[str, Any]] = []
    for entry in entries:
        if not isinstance(entry, dict):
            raise ValueError("invalid base manifest entry")
        copied = dict(entry)
        if copied.get("role") == "moss_p1_private_output":
            copied.update(
                {
                    "path": str(args.private_adapter.resolve()),
                    "bytes": args.private_adapter.stat().st_size,
                    "sha256": adapter_sha,
                    "sensitive": True,
                    "storage": "EXTERNAL_REFERENCE_NOT_COPIED",
                }
            )
            replaced += 1
        new_entries.append(copied)
    if replaced != 1:
        raise ValueError(f"expected one MOSS role replacement, got {replaced}")

    manifest = dict(base_manifest)
    manifest.update(
        {
            "bundle_id": "MOSS-P0R-R1-MONOLITHIC-VULKAN16K-20260831",
            "created_at": datetime.now(timezone.utc).isoformat(),
            "base_manifest": {
                "path": str(args.base_manifest.resolve()),
                "bytes": args.base_manifest.stat().st_size,
                "sha256": sha256_file(args.base_manifest),
            },
            "entries": new_entries,
        }
    )
    manifest_sha = write_json_new(args.manifest, manifest)

    audit = {
        "schema_version": 1,
        "role": "MOSS_R1_SCORING_ADAPTER_AUDIT",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "status": "PASS",
        "checks": {
            "source_completed": True,
            "source_structural_status_pass": True,
            "raw_turns_nonempty": True,
            "one_manifest_role_replaced": replaced == 1,
            "single_session_speaker_scope": True,
            "transcript_not_embedded_in_public_audit": True,
        },
        "source": {
            "bytes": args.monolithic.stat().st_size,
            "sha256": sha256_file(args.monolithic),
            "turn_count": len(global_turns),
            "inference_rtf": inference_rtf,
        },
        "private_adapter": {
            "file_name": args.private_adapter.name,
            "bytes": args.private_adapter.stat().st_size,
            "sha256": adapter_sha,
            "restricted": True,
        },
        "manifest": {
            "path": str(args.manifest.resolve()),
            "bytes": args.manifest.stat().st_size,
            "sha256": manifest_sha,
        },
        "transcript_text_included": False,
    }
    audit_sha = write_json_new(args.audit, audit)
    print(
        json.dumps(
            {
                "status": "PASS",
                "turn_count": len(global_turns),
                "inference_rtf": inference_rtf,
                "adapter_sha256": adapter_sha,
                "manifest_sha256": manifest_sha,
                "audit_sha256": audit_sha,
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
