#!/usr/bin/env python3
"""Run the one-shot formal Q00 gate against the installed isolated candidate.

The scorer intentionally does not know how to manufacture product evidence.
This runner owns that job: install the exact candidate, seed only hash-bound
models/runtime files, import the frozen window through Meetily/Whisper, run the
real MOSS review/activation and Qwen summary chain, stop the app cleanly, and
export immutable evidence from the resulting SQLite database.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import sys
import time
from typing import Any, Sequence
import urllib.request
import uuid
import winreg


QA_ROOT = Path(__file__).resolve().parent
if str(QA_ROOT) not in sys.path:
    sys.path.insert(0, str(QA_ROOT))

import moss_functional_fix_quality_gate as gate  # noqa: E402


FORMAL_RUNNER_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_FORMAL_RUN"
OWNER_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_ISOLATED_OWNER"
RUNTIME_CONTRACT_BYTES = 125
RUNTIME_CONTRACT_SHA256 = "C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73"
MODEL_DESTINATIONS = {
    "whisper": Path("models") / gate.MODEL_CONTRACTS["whisper"]["filename"],
    "qwen_2b": Path("models") / "summary" / gate.MODEL_CONTRACTS["qwen_2b"]["filename"],
    "moss": Path("models") / "moss" / gate.MODEL_CONTRACTS["moss"]["filename"],
}


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def _path_record(path: Path) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    return {"path": str(resolved), **gate._safe_file_record(resolved)}


def _write_atomic(path: Path, value: Any) -> None:
    path = Path(os.path.abspath(path))
    if not path.parent.is_dir():
        raise gate.QualityGateError(f"Atomic JSON parent is missing: {path.parent}")
    temporary = path.parent / f".{path.name}.{uuid.uuid4().hex}.tmp"
    try:
        with temporary.open("x", encoding="utf-8", newline="\n") as stream:
            json.dump(value, stream, ensure_ascii=False, sort_keys=True, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _copy_exclusive(source: Path, destination: Path, *, size: int, sha256: str, label: str) -> dict[str, Any]:
    source_record = gate._require_exact_file(source, size=size, sha256=sha256, label=label)
    if destination.exists():
        raise gate.QualityGateError(f"Refusing to overwrite {label}: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    gate.reject_symlink(destination.parent, boundary=destination.parent)
    with source.open("rb") as reader, destination.open("xb") as writer:
        while True:
            block = reader.read(4 * 1024 * 1024)
            if not block:
                break
            writer.write(block)
        writer.flush()
        os.fsync(writer.fileno())
    actual = gate._safe_file_record(destination)
    if actual != source_record or gate._safe_file_record(source) != source_record:
        raise gate.QualityGateError(f"{label} changed or copied incorrectly")
    return actual


def _directory_file_records(root: Path) -> list[dict[str, Any]]:
    resolved = root.resolve(strict=True)
    records: list[dict[str, Any]] = []
    for path in sorted(resolved.rglob("*"), key=lambda item: item.as_posix().casefold()):
        gate.reject_symlink(path, boundary=resolved)
        if not path.is_file():
            continue
        records.append(
            {
                "relative_path": path.relative_to(resolved).as_posix(),
                **gate._safe_file_record(path),
            }
        )
    return records


def _copy_runtime(source_root: Path, destination_root: Path) -> dict[str, Any]:
    source = source_root.resolve(strict=True)
    if not source.is_dir() or destination_root.exists():
        raise gate.QualityGateError("MOSS runtime source is missing or isolated destination already exists")
    source_records = _directory_file_records(source)
    if len(source_records) < 4:
        raise gate.QualityGateError("MOSS runtime package is incomplete")
    contract = next((item for item in source_records if item["relative_path"] == "contract.json"), None)
    if contract != {
        "relative_path": "contract.json",
        "bytes": RUNTIME_CONTRACT_BYTES,
        "sha256": RUNTIME_CONTRACT_SHA256,
    }:
        raise gate.QualityGateError("MOSS runtime contract is not the approved v0.2.2 package")
    for record in source_records:
        _copy_exclusive(
            source / Path(*record["relative_path"].split("/")),
            destination_root / Path(*record["relative_path"].split("/")),
            size=record["bytes"],
            sha256=record["sha256"],
            label=f"MOSS runtime {record['relative_path']}",
        )
    if _directory_file_records(destination_root) != source_records:
        raise gate.QualityGateError("Isolated MOSS runtime copy differs from its source package")
    return {
        "root": str(source),
        "contract": _path_record(source / "contract.json"),
        "files": source_records,
    }


def _run_process(
    executable: Path,
    arguments: Sequence[str],
    *,
    cwd: Path,
    timeout_seconds: int,
    environment: dict[str, str] | None = None,
) -> dict[str, Any]:
    program = executable.resolve(strict=True)
    started_at = _utc_now()
    process = subprocess.Popen(
        [str(program), *map(str, arguments)],
        cwd=str(cwd.resolve(strict=True)),
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    timed_out = False
    try:
        stdout, stderr = process.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        subprocess.run(
            ["taskkill.exe", "/PID", str(process.pid), "/T", "/F"],
            capture_output=True,
            text=True,
            check=False,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        stdout, stderr = process.communicate(timeout=30)
    return {
        "program": _path_record(program),
        "arguments": [str(item) for item in arguments],
        "working_directory": str(cwd.resolve(strict=True)),
        "pid": process.pid,
        "started_at": started_at,
        "completed_at": _utc_now(),
        "exit_code": int(process.returncode),
        "timed_out": timed_out,
        "stdout": stdout,
        "stderr": stderr,
    }


def _registry_state(product_name: str) -> dict[str, Any] | None:
    key_name = rf"Software\Microsoft\Windows\CurrentVersion\Uninstall\{product_name}"
    try:
        key = winreg.OpenKey(winreg.HKEY_CURRENT_USER, key_name)
    except FileNotFoundError:
        return None
    try:
        values = {}
        for name in ("DisplayName", "DisplayVersion", "InstallLocation", "UninstallString"):
            values[name] = winreg.QueryValueEx(key, name)[0]
        return values
    finally:
        winreg.CloseKey(key)


def _load_build_manifest(path: Path, installer: Path, source_commit: str) -> dict[str, Any]:
    document = gate.require_mapping(gate.read_json(path, "candidate build manifest"), "candidate build manifest")
    if (
        document.get("schema_version") != 1
        or document.get("stage") != gate.CANDIDATE_BUILD_MANIFEST_STAGE
        or document.get("status") != gate.PASS
        or document.get("role") != "candidate"
        or document.get("source_commit") != source_commit
        or document.get("product_name") != gate.APPROVED_PRODUCT_NAME
        or document.get("bundle_id") != gate.APPROVED_BUNDLE_ID
    ):
        raise gate.QualityGateError("Candidate build manifest identity is wrong")
    installer_record = gate._safe_file_record(installer.resolve(strict=True))
    declared_installer = gate.require_mapping(document.get("installer"), "candidate installer record")
    if (
        declared_installer.get("bytes") != installer_record["bytes"]
        or str(declared_installer.get("sha256", "")).upper() != installer_record["sha256"]
    ):
        raise gate.QualityGateError("Candidate installer does not match the build manifest")
    installed = gate.require_list(document.get("installed_files"), "candidate installed files")
    by_role = {gate.require_string(item.get("role"), "installed role"): item for item in installed}
    if len(by_role) != len(installed) or set(by_role) != set(gate.PROGRAM_CONTRACTS):
        raise gate.QualityGateError("Candidate build manifest does not contain the exact seven installed roles")
    return document


def _prepare_isolated_install(
    *,
    installer: Path,
    manifest: dict[str, Any],
    models: dict[str, Path],
    runtime_root: Path,
    run_id: str,
    source_commit: str,
) -> dict[str, Any]:
    local = Path(os.environ["LOCALAPPDATA"]).resolve(strict=True)
    roaming = Path(os.environ["APPDATA"]).resolve(strict=True)
    install_root = local / gate.APPROVED_PRODUCT_NAME
    data_root = roaming / gate.APPROVED_BUNDLE_ID
    webview_root = local / gate.APPROVED_BUNDLE_ID
    backup_root = Path(str(data_root) + ".rollback-backups")
    for path in (install_root, data_root, webview_root, backup_root):
        if path.exists():
            raise gate.QualityGateError(f"Unowned isolated Q00 path already exists: {path}")
    if _registry_state(gate.APPROVED_PRODUCT_NAME) is not None:
        raise gate.QualityGateError("Isolated Q00 uninstall registry key already exists")
    owner = {
        "schema_version": 1,
        "stage": OWNER_STAGE,
        "run_id": run_id,
        "source_commit": source_commit,
        "candidate_sha256": manifest["installer"]["sha256"].upper(),
        "created_at": _utc_now(),
    }
    partial: dict[str, Any] = {
        "install_root": install_root,
        "data_root": data_root,
        "webview_root": webview_root,
        "backup_root": backup_root,
        "installed_records": {},
        "models": {},
        "runtime": None,
        "owner": owner,
        "install_run": None,
    }
    try:
        install_run = _run_process(
            installer,
            ["/S"],
            cwd=installer.resolve(strict=True).parent,
            timeout_seconds=1_200,
        )
        partial["install_run"] = install_run
        if install_run["timed_out"] or install_run["exit_code"] != 0:
            raise gate.QualityGateError("Candidate silent installation failed")
        deadline = time.monotonic() + 30
        registry = None
        while time.monotonic() < deadline:
            registry = _registry_state(gate.APPROVED_PRODUCT_NAME)
            if registry is not None and install_root.is_dir():
                break
            time.sleep(0.25)
        if registry is None:
            raise gate.QualityGateError("Candidate installer did not create the isolated registry identity")
        if (
            registry["DisplayName"] != gate.APPROVED_PRODUCT_NAME
            or registry["DisplayVersion"] != manifest.get("version")
            or os.path.normcase(os.path.abspath(registry["InstallLocation"]))
            != os.path.normcase(os.path.abspath(install_root))
        ):
            raise gate.QualityGateError("Installed candidate registry identity/version/path is wrong")
        installed_records: dict[str, dict[str, Any]] = {}
        manifest_roles = {item["role"]: item for item in manifest["installed_files"]}
        for role, relative in gate.PROGRAM_CONTRACTS.items():
            path = install_root / Path(*relative.split("/"))
            actual = gate._safe_file_record(path)
            declared = manifest_roles[role]
            if actual["bytes"] != declared.get("bytes") or actual["sha256"] != str(
                declared.get("sha256", "")
            ).upper():
                raise gate.QualityGateError(f"Installed candidate file differs from manifest: {role}")
            installed_records[role] = {"path": path.resolve(strict=True), **actual}
        partial["installed_records"] = installed_records

        data_root.mkdir()
        webview_root.mkdir()
        gate._write_json_exclusive(data_root / ".q00-owner.private.json", owner)
        gate._write_json_exclusive(webview_root / ".q00-owner.private.json", owner)

        validated_models: dict[str, dict[str, Any]] = {}
        for role, source in models.items():
            contract = gate.MODEL_CONTRACTS[role]
            target = data_root / MODEL_DESTINATIONS[role]
            _copy_exclusive(
                source,
                target,
                size=int(contract["bytes"]),
                sha256=str(contract["sha256"]),
                label=f"Q00 {role} model",
            )
            validated_models[role] = _path_record(source)
        partial["models"] = validated_models
        partial["runtime"] = _copy_runtime(runtime_root, data_root / "runtime" / "moss")
        return partial
    except Exception as exc:
        cleanup = _cleanup_install(partial)
        if cleanup["status"] != "PASS":
            raise gate.QualityGateError(
                f"Q00 isolated setup failed and exact cleanup also failed: {cleanup['errors']}"
            ) from exc
        raise


def _port_available(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        try:
            listener.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


def _start_app(install: dict[str, Any], port: int) -> tuple[subprocess.Popen[Any], dict[str, Any]]:
    if not _port_available(port):
        raise gate.QualityGateError(f"Configured Q00 CDP port is not free: {port}")
    main = install["installed_records"]["main_executable"]
    environment = os.environ.copy()
    environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
        f"--remote-debugging-port={port} --remote-allow-origins=http://127.0.0.1:{port}"
    )
    started_at = _utc_now()
    process = subprocess.Popen(
        [str(main["path"])],
        cwd=str(install["install_root"]),
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    deadline = time.monotonic() + 120
    target = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise gate.QualityGateError("Candidate app exited before its exact CDP target became ready")
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=2) as response:
                targets = json.load(response)
            pages = [
                item
                for item in targets
                if item.get("type") == "page"
                and str(item.get("url", "")).startswith(("http://tauri.localhost", "http://localhost:"))
            ]
            if len(pages) == 1:
                target = pages[0]
                break
        except (OSError, ValueError):
            pass
        time.sleep(0.5)
    if target is None:
        raise gate.QualityGateError("Candidate app did not expose one exact CDP page")
    time.sleep(3)
    return process, {
        "pid": process.pid,
        "executable_path": str(main["path"]),
        "bytes": main["bytes"],
        "sha256": main["sha256"],
        "started_at": started_at,
        "cdp_port": port,
        "cdp_target_id": target["id"],
    }


def _invoke_cdp(
    *,
    node: Path,
    cdp_script: Path,
    repo: Path,
    state_path: Path,
    raw_root: Path,
    action: str,
    label: str,
    request: dict[str, Any],
    port: int,
    target_id: str,
    timeout_seconds: int,
) -> dict[str, Any]:
    output_path = raw_root / f"{label}.json"
    request_path = raw_root / f"{label}.request.private.json"
    gate._write_json_exclusive(request_path, request)
    arguments = [str(cdp_script), action, str(state_path), str(output_path), str(request_path)]
    environment = os.environ.copy()
    environment["CDP_PORT"] = str(port)
    environment["CDP_TARGET_ID"] = target_id
    started_ns = time.monotonic_ns()
    run = _run_process(
        node,
        arguments,
        cwd=repo,
        timeout_seconds=timeout_seconds,
        environment=environment,
    )
    completed_ns = time.monotonic_ns()
    if run["timed_out"] or run["exit_code"] != 0 or not output_path.is_file():
        raise gate.QualityGateError(f"CDP action failed: {action}: {run['stderr'][-1000:]}")
    document = gate.require_mapping(gate.read_json(output_path, f"CDP {action}"), f"CDP {action}")
    return {
        "run": run,
        "document": document,
        "output_path": output_path,
        "request_path": request_path,
        "arguments": arguments,
        "started_monotonic_ns": started_ns,
        "completed_monotonic_ns": completed_ns,
    }


def _canonical_sha256(value: Any) -> str:
    return gate.sha256_bytes(
        json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).lower()


def _attach_product_context(
    *, metadata_path: Path, premeeting_context_path: Path, speaker_truth_path: Path, run_id: str
) -> dict[str, Any]:
    metadata = gate.require_mapping(gate.read_json(metadata_path, "import metadata"), "import metadata")
    if "meeting_context" in metadata:
        raise gate.QualityGateError("Imported meeting unexpectedly already contains a meeting context")
    source = gate.require_mapping(
        gate.read_json(premeeting_context_path, "frozen pre-meeting context"),
        "frozen pre-meeting context",
    )
    business_terms = [
        gate.require_string(item.get("term"), "frozen business term")
        for item in gate.require_list(source.get("entries"), "frozen pre-meeting entries")
        if item.get("type") == "business_term"
    ]
    if business_terms != ["M100", "YouTube", "PWA", "H5", "Google", "VIP", "A/B Test", "TG"]:
        raise gate.QualityGateError("Frozen product business-term context is not exact")
    truth = gate._load_speaker_truth(speaker_truth_path)
    people_ids = sorted({item["speaker"] for item in truth})
    if people_ids != ["H01", "H04"]:
        raise gate.QualityGateError("Frozen Q00 speaker truth identities are not the exact two-person set")
    people = [
        {
            "person_id": person_id,
            "display_name": person_id,
            "aliases": [],
            "department": None,
            "role": "Q00 frozen reference speaker",
            "enabled": True,
        }
        for person_id in people_ids
    ]
    terms = [
        {
            "term_id": f"term_{index:02d}",
            "canonical": term,
            "aliases": [],
            "category": "business_term",
            "enabled": True,
        }
        for index, term in enumerate(business_terms, 1)
    ]
    profile = {
        "schema_version": 1,
        "fixed_meeting_mechanism": "Q00 frozen 226.440 second business review",
        "people": people,
        "terms": terms,
    }
    nonce = run_id.rsplit("-", 1)[-1]
    context_id = f"ctx_recording_{nonce}"
    snapshot = {
        "context_id": context_id,
        "revision": 1,
        "reason": "recording_start",
        "captured_at": _utc_now(),
        "source": {
            "template_id": "template_q00_frozen_business_review",
            "template_version": 1,
            "template_file_sha256": gate.FROZEN_TRUTH_MANIFEST_SHA256.lower(),
            "profile_sha256": _canonical_sha256(profile),
        },
        "fixed_meeting_mechanism": profile["fixed_meeting_mechanism"],
        "people": [
            {
                "person_id": item["person_id"],
                "display_name": item["display_name"],
                "aliases": item["aliases"],
                "department": item["department"],
                "role": item["role"],
                "attendance": "attending",
            }
            for item in people
        ],
        "host_person_id": "H01",
        "terms": [
            {
                "term_id": item["term_id"],
                "canonical": item["canonical"],
                "aliases": item["aliases"],
                "category": item["category"],
            }
            for item in terms
        ],
        "context_sha256": "",
    }
    snapshot["context_sha256"] = _canonical_sha256(snapshot)
    metadata["meeting_context"] = {
        "schema_version": 1,
        "recording_context_id": context_id,
        "current_context_id": context_id,
        "contexts": [snapshot],
    }
    _write_atomic(metadata_path, metadata)
    return {"profile": profile, "snapshot": snapshot, "metadata": _path_record(metadata_path)}


def _speaker_bindings(
    candidate_segments: list[dict[str, Any]], speaker_truth_path: Path
) -> tuple[list[dict[str, str]], str, str, str]:
    predicted = [
        {
            "start": int(item["startMs"]) / 1000.0,
            "end": int(item["endMs"]) / 1000.0,
            "speaker": gate.require_string(item.get("speakerLabel"), "candidate speaker label"),
        }
        for item in candidate_segments
    ]
    truth = gate._load_speaker_truth(speaker_truth_path)
    mapping, complete = gate._speaker_mapping(predicted, truth)
    if not complete or set(mapping) != {item["speaker"] for item in predicted}:
        raise gate.QualityGateError("MOSS anonymous speaker labels could not be completely mapped to frozen truth")
    bindings = [
        {"speaker_label": label, "person_id": mapping[label]}
        for label in sorted(mapping)
    ]
    target: dict[str, Any] | None = None
    correct_person: str | None = None
    for candidate, predicted_segment in zip(candidate_segments, predicted):
        overlap, reference_turn = max(
            (gate._overlap_seconds(predicted_segment, truth_turn), truth_turn)
            for truth_turn in truth
        )
        mapped_person = mapping[predicted_segment["speaker"]]
        if overlap > 0.0 and mapped_person == reference_turn["speaker"]:
            target = candidate
            correct_person = str(reference_turn["speaker"])
            break
    if target is None or correct_person is None:
        raise gate.QualityGateError(
            "Q00 candidate has no segment suitable for a truth-bound speaker true flip"
        )
    wrong_people = sorted({item["speaker"] for item in truth} - {correct_person})
    if len(wrong_people) != 1:
        raise gate.QualityGateError(
            "Q00 true-flip coverage requires exactly one wrong frozen person for the target segment"
        )
    return (
        bindings,
        gate.require_string(target.get("segmentId"), "candidate override segment"),
        wrong_people[0],
        correct_person,
    )


def _speaker_override_audit(
    *,
    history: list[Any],
    active_override: Any,
    segment: Any,
    segment_index: int,
    speaker_truth: list[dict[str, Any]],
) -> dict[str, Any]:
    if len(history) != 2:
        raise gate.QualityGateError(
            "Q00 true-flip target must contain exactly one wrong and one corrected override"
        )
    wrong_override, corrected_override = history
    before_person = str(wrong_override["person_id"] or "")
    after_person = str(corrected_override["person_id"] or "")
    segment_for_truth = {
        "start": int(segment["start_ms"]) / 1000.0,
        "end": int(segment["end_ms"]) / 1000.0,
    }
    overlap, reference_turn = max(
        (
            gate._overlap_seconds(segment_for_truth, truth_turn),
            truth_turn,
        )
        for truth_turn in speaker_truth
    )
    reference_person = str(reference_turn["speaker"])
    if (
        wrong_override["revoked_at"] is None
        or corrected_override["revoked_at"] is not None
        or wrong_override["reason_code"] != "MANUAL_PERSON_OVERRIDE"
        or corrected_override["reason_code"] != "MANUAL_PERSON_OVERRIDE"
        or overlap <= 0.0
        or not before_person
        or before_person == after_person
        or after_person != reference_person
        or str(corrected_override["override_id"]) != str(active_override["override_id"])
    ):
        raise gate.QualityGateError(
            "Q00 speaker override history is not a real wrong-to-correct flip"
        )
    return {
        "wrong_override_id": str(wrong_override["override_id"]),
        "override_id": str(corrected_override["override_id"]),
        "segment_id": str(segment["segment_id"]),
        "segment_index": segment_index,
        "source": "HUMAN_SINGLE_SEGMENT_OVERRIDE",
        "before_speaker_id": before_person,
        "after_speaker_id": after_person,
        "reference_speaker_id": reference_person,
    }


def _powershell_process_scan(install_root: Path) -> list[dict[str, Any]]:
    powershell = Path(os.environ["SystemRoot"]) / "System32" / "WindowsPowerShell" / "v1.0" / "powershell.exe"
    root = str(install_root.resolve(strict=True)).replace("'", "''").rstrip("\\") + "\\"
    command = (
        "$items = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | "
        f"Where-Object {{ $_.ExecutablePath -and $_.ExecutablePath.StartsWith('{root}', "
        "[System.StringComparison]::OrdinalIgnoreCase) }} | "
        "Select-Object @{n='pid';e={[int]$_.ProcessId}},@{n='parent_pid';e={[int]$_.ParentProcessId}},"
        "@{n='executable_path';e={[string]$_.ExecutablePath}}); "
        "ConvertTo-Json -Compress -InputObject $items"
    )
    completed = subprocess.run(
        [str(powershell), "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    if completed.returncode != 0:
        raise gate.QualityGateError("Could not scan exact candidate processes")
    value = json.loads(completed.stdout or "[]")
    if isinstance(value, dict):
        return [value]
    if not isinstance(value, list):
        raise gate.QualityGateError("Exact candidate process scan returned an invalid shape")
    return value


def _stop_app(
    *, process: subprocess.Popen[Any], node: Path, repo: Path, port: int, target_id: str, install_root: Path, raw_root: Path
) -> dict[str, Any]:
    exit_path = raw_root / "candidate-exit.private.json"
    environment = os.environ.copy()
    environment["CDP_PORT"] = str(port)
    environment["CDP_TARGET_ID"] = target_id
    exit_run = _run_process(
        node,
        [str(repo / "scripts" / "qa" / "cdp-exit-app.mjs"), str(exit_path)],
        cwd=repo,
        timeout_seconds=30,
        environment=environment,
    )
    deadline = time.monotonic() + 15
    while process.poll() is None and time.monotonic() < deadline:
        time.sleep(0.25)
    forced = False
    if process.poll() is None:
        forced = True
        subprocess.run(
            ["taskkill.exe", "/PID", str(process.pid), "/T", "/F"],
            capture_output=True,
            text=True,
            check=False,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        process.wait(timeout=30)
    helper_deadline = time.monotonic() + 30
    while time.monotonic() < helper_deadline and _powershell_process_scan(install_root):
        time.sleep(0.5)
    scans = []
    for attempt in range(1, 3):
        rows = _powershell_process_scan(install_root)
        scans.append({"attempt": attempt, "processes": rows, "process_count": len(rows)})
        if attempt == 1:
            time.sleep(2)
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as client:
        client.settimeout(1)
        listener_closed = client.connect_ex(("127.0.0.1", port)) != 0
    residual = scans[-1]["processes"]
    completed = not residual and all(item["process_count"] == 0 for item in scans) and listener_closed
    return {
        "completed": completed,
        "consecutive_zero_scans": sum(item["process_count"] == 0 for item in scans),
        "cdp_listener_closed": listener_closed,
        "residual_processes": residual,
        "scans": scans,
        "graceful_exit": exit_run,
        "forced_tree_termination": forced,
    }


def _snapshot_installed_files(install: dict[str, Any], run_directory: Path, manifest: dict[str, Any]) -> Path:
    root = run_directory / "candidate-installed-snapshot"
    root.mkdir()
    by_role = {item["role"]: item for item in manifest["installed_files"]}
    for role, relative in gate.PROGRAM_CONTRACTS.items():
        declared = by_role[role]
        _copy_exclusive(
            install["installed_records"][role]["path"],
            root / Path(*relative.split("/")),
            size=int(declared["bytes"]),
            sha256=str(declared["sha256"]),
            label=f"installed candidate snapshot {role}",
        )
    return root


def _snapshot_database(data_root: Path, run_directory: Path) -> Path:
    source = data_root / "meeting_minutes.sqlite"
    if not source.is_file():
        raise gate.QualityGateError("Product database is missing after the Q00 run")
    deadline = time.monotonic() + 15
    sidecars = [Path(str(source) + suffix) for suffix in ("-wal", "-shm", "-journal")]
    while time.monotonic() < deadline and any(path.exists() for path in sidecars):
        time.sleep(0.25)
    if any(path.exists() for path in sidecars):
        raise gate.QualityGateError("Product database did not reach a sidecar-free clean-exit state")
    record = gate._safe_file_record(source)
    destination = run_directory / "product-state.sqlite"
    _copy_exclusive(
        source,
        destination,
        size=record["bytes"],
        sha256=record["sha256"],
        label="clean-exit product database",
    )
    return destination


def _provenance(
    *, role: str, run_id: str, source_commit: str, producer_sha256: str, session_nonce: str, produced_at: str
) -> dict[str, Any]:
    return {
        "artifact_role": role,
        "run_id": run_id,
        "source_commit": source_commit,
        "window_audio_sha256": gate.WINDOW_SHA256,
        "producer_executable_sha256": producer_sha256,
        "session_nonce": session_nonce,
        "produced_at": produced_at,
    }


def _validated_moss_native_decode_contract(
    raw_proof: Any, persisted_run: Any
) -> dict[str, str]:
    proof = gate.require_mapping(raw_proof, "MOSS native decode proof")
    if set(proof) != {
        "source",
        "language_requested",
        "language_resolved",
        "decode_parameters_json",
        "decode_parameters_sha256",
        "recomputed_decode_parameters_sha256",
        "api_fields",
        "passed",
    }:
        raise gate.QualityGateError("MOSS native decode proof fields are not exact")
    if proof.get("source") != "api_moss_get_workspace.runs":
        raise gate.QualityGateError(
            "MOSS native decode proof must come from api_moss_get_workspace.runs"
        )
    if proof.get("passed") is not True:
        raise gate.QualityGateError("MOSS native decode proof did not pass at its API producer")

    api_fields = gate.require_mapping(
        proof.get("api_fields"), "MOSS native decode API fields"
    )
    if set(api_fields) != {
        "languageRequested",
        "languageResolved",
        "decodeParametersJson",
        "decodeParametersSha256",
    }:
        raise gate.QualityGateError("MOSS native decode API fields are not exact")

    language_requested = gate.require_string(
        proof.get("language_requested"), "MOSS language_requested"
    )
    language_resolved = gate.require_string(
        proof.get("language_resolved"), "MOSS language_resolved"
    )
    if language_requested != gate.FAIR_COMPARISON_LANGUAGE:
        raise gate.QualityGateError(
            f"MOSS language_requested must be explicit {gate.FAIR_COMPARISON_LANGUAGE}"
        )
    if language_resolved.casefold() == "auto":
        raise gate.QualityGateError("MOSS language_resolved cannot be auto")
    if language_requested != language_resolved:
        raise gate.QualityGateError("MOSS requested/resolved language mismatch")

    decode_json, decode_parameters, calculated_sha256 = gate._parse_decode_parameters_json(
        proof.get("decode_parameters_json"), "MOSS decode_parameters_json"
    )
    if decode_parameters != gate.MOSS_NATIVE_DECODE_PARAMETERS_CONTRACT:
        raise gate.QualityGateError(
            "MOSS decode_parameters_json is not the exact frozen native contract"
        )
    declared_sha256 = gate.require_sha256(
        proof.get("decode_parameters_sha256"), "MOSS decode_parameters_sha256"
    )
    recomputed_sha256 = gate.require_sha256(
        proof.get("recomputed_decode_parameters_sha256"),
        "MOSS recomputed_decode_parameters_sha256",
    )
    if declared_sha256 != calculated_sha256 or recomputed_sha256 != calculated_sha256:
        raise gate.QualityGateError(
            "MOSS decode_parameters_sha256 does not match the actual JSON bytes"
        )

    expected_api_fields = {
        "languageRequested": language_requested,
        "languageResolved": language_resolved,
        "decodeParametersJson": decode_json,
        "decodeParametersSha256": gate.require_string(
            proof.get("decode_parameters_sha256"), "MOSS raw decode parameter SHA-256"
        ),
    }
    if api_fields != expected_api_fields:
        raise gate.QualityGateError(
            "MOSS native decode proof differs from its api_moss_get_workspace.runs fields"
        )

    persisted_values = {
        "language_requested": language_requested,
        "language_resolved": language_resolved,
        "decode_parameters_json": decode_json,
        "decode_parameters_sha256": declared_sha256,
    }
    try:
        persisted_matches = (
            str(persisted_run["language_requested"]) == language_requested
            and str(persisted_run["language_resolved"]) == language_resolved
            and str(persisted_run["decode_parameters_json"]) == decode_json
            and str(persisted_run["decode_parameters_sha256"]).upper()
            == declared_sha256
        )
    except (KeyError, IndexError, TypeError) as exc:
        raise gate.QualityGateError(
            "Persisted MOSS run is missing native decode contract fields"
        ) from exc
    if not persisted_matches:
        raise gate.QualityGateError(
            "MOSS native decode proof does not equal the persisted MOSS run"
        )
    return {
        "source": "api_moss_get_workspace.runs",
        **persisted_values,
    }


def _export_product_evidence(
    *,
    database_path: Path,
    state_path: Path,
    run_directory: Path,
    run_id: str,
    source_commit: str,
    session_nonce: str,
    program_records: dict[str, dict[str, Any]],
    speaker_truth_path: Path,
    transcription_run_evidence: dict[str, Any],
) -> tuple[dict[str, Path], dict[str, str]]:
    state = gate.require_mapping(gate.read_json(state_path, "Q00 product state"), "Q00 product state")
    meeting_id = gate.require_string(state.get("meeting_id"), "Q00 meeting_id")
    moss_run_id = gate.require_string(state.get("moss_run_id"), "Q00 moss_run_id")
    activation_id = gate.require_string(state.get("activation_id"), "Q00 activation_id")
    summary_runs = gate.require_list(state.get("summary_runs"), "Q00 summary runs")
    if len(summary_runs) != 1:
        raise gate.QualityGateError("Q00 must contain exactly one summary generation")
    summary_id = gate.require_string(summary_runs[0].get("generation_id"), "Q00 summary generation_id")
    speaker_truth = gate._load_speaker_truth(speaker_truth_path)
    before = gate._safe_file_record(database_path)
    uri = database_path.as_uri() + "?mode=ro&immutable=1"
    connection = sqlite3.connect(uri, uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA query_only=ON")
        if connection.execute("PRAGMA integrity_check").fetchone()[0] != "ok":
            raise gate.QualityGateError("Q00 product database snapshot failed integrity_check")
        run = connection.execute(
            "SELECT * FROM moss_transcription_runs WHERE run_id=? AND meeting_id=?",
            (moss_run_id, meeting_id),
        ).fetchall()
        if len(run) != 1:
            raise gate.QualityGateError("Q00 snapshot lacks one exact completed MOSS run")
        run_row = run[0]
        moss_native_decode = _validated_moss_native_decode_contract(
            transcription_run_evidence.get("moss_native_decode_contract"), run_row
        )
        raw_rows = connection.execute(
            "SELECT * FROM moss_candidate_segments WHERE run_id=? ORDER BY segment_index", (moss_run_id,)
        ).fetchall()
        if not raw_rows:
            raise gate.QualityGateError("Q00 snapshot has no MOSS candidate segments")
        materialized = []
        override_audits = []
        for segment_index, row in enumerate(raw_rows):
            text = str(row["raw_text"])
            corrections = connection.execute(
                "SELECT * FROM moss_term_corrections WHERE segment_id=? AND reverted_at IS NULL ORDER BY revision",
                (row["segment_id"],),
            ).fetchall()
            for correction in corrections:
                text = gate._replace_char_range(
                    text,
                    int(correction["start_char"]),
                    int(correction["end_char"]),
                    str(correction["original_text"]),
                    str(correction["replacement_text"]),
                )
                if text != str(correction["result_text"]):
                    raise gate.QualityGateError("Q00 correction replay differs from persisted result_text")
            overrides = connection.execute(
                "SELECT * FROM moss_segment_overrides WHERE segment_id=? AND revoked_at IS NULL",
                (row["segment_id"],),
            ).fetchall()
            if len(overrides) > 1:
                raise gate.QualityGateError("Q00 snapshot contains duplicate active segment overrides")
            override = overrides[0] if overrides else None
            if override is not None and override["replacement_text"] is not None:
                text = str(override["replacement_text"])
            if override is not None:
                history = connection.execute(
                    "SELECT * FROM moss_segment_overrides WHERE segment_id=? ORDER BY revision",
                    (row["segment_id"],),
                ).fetchall()
                override_audits.append(
                    _speaker_override_audit(
                        history=history,
                        active_override=override,
                        segment=row,
                        segment_index=segment_index,
                        speaker_truth=speaker_truth,
                    )
                )
            bindings = connection.execute(
                "SELECT * FROM moss_speaker_bindings WHERE run_id=? AND speaker_label=? AND revoked_at IS NULL",
                (moss_run_id, row["speaker_label"]),
            ).fetchall()
            if len(bindings) != 1:
                raise gate.QualityGateError("Q00 snapshot lacks one exact active binding for every speaker label")
            person_id = (
                str(override["person_id"])
                if override is not None and override["person_id"] is not None
                else str(bindings[0]["person_id"])
            )
            materialized.append(
                {
                    "start_seconds": int(row["start_ms"]) / 1000.0,
                    "end_seconds": int(row["end_ms"]) / 1000.0,
                    "speaker_id": person_id,
                    "text": text,
                    "candidate_segment_id": str(row["segment_id"]),
                }
            )
        activations = connection.execute(
            "SELECT * FROM moss_activation_snapshots WHERE activation_id=? AND meeting_id=? AND run_id=?",
            (activation_id, meeting_id, moss_run_id),
        ).fetchall()
        if len(activations) != 1:
            raise gate.QualityGateError("Q00 snapshot lacks one exact activation")
        activation = activations[0]
        pre_activation = json.loads(str(activation["pre_activation_transcripts_json"]))
        whisper_segments = [
            {
                "start_seconds": item.get("audio_start_time"),
                "end_seconds": item.get("audio_end_time"),
                "speaker_id": item.get("speaker") or "",
                "text": item.get("transcript"),
            }
            for item in pre_activation
        ]
        summaries = connection.execute(
            "SELECT * FROM summary_generation_history WHERE generation_id=? AND meeting_id=?",
            (summary_id, meeting_id),
        ).fetchall()
        if len(summaries) != 1:
            raise gate.QualityGateError("Q00 snapshot lacks one exact summary generation")
        summary = summaries[0]
    finally:
        connection.close()
    if gate._safe_file_record(database_path) != before:
        raise gate.QualityGateError("Immutable Q00 database changed while evidence was exported")
    if len(override_audits) != 1:
        raise gate.QualityGateError("Q00 must persist exactly one true-flip speaker coverage audit")
    produced_at = _utc_now()
    main_sha = program_records["main_executable"]["sha256"]
    moss_sha = program_records["moss_helper"]["sha256"]
    artifacts = {
        "moss_raw": run_directory / "moss-raw.json",
        "whisper_same_window": run_directory / "whisper-same-window.json",
        "corrected": run_directory / "moss-corrected.json",
        "activation_evidence": run_directory / "activation-evidence.json",
        "summary_evidence": run_directory / "summary-evidence.json",
    }
    whisper_contract = gate._expected_transcription_contract("whisper")
    whisper_language = gate.require_string(
        transcription_run_evidence.get("whisper_language_resolved"),
        "Whisper import language",
    )
    whisper_window = gate.require_mapping(
        transcription_run_evidence.get("whisper_window"), "Whisper input window"
    )
    whisper_model = gate.require_mapping(
        transcription_run_evidence.get("whisper_model"), "Whisper model record"
    )
    moss_window_duration = gate._finite_number(
        transcription_run_evidence.get("moss_window_duration_seconds"),
        "MOSS requested window duration",
        minimum=0.0,
    )
    whisper_window_duration = gate._finite_number(
        transcription_run_evidence.get("whisper_window_duration_seconds"),
        "Whisper requested window duration",
        minimum=0.0,
    )
    documents = {
        "moss_raw": {
            "schema_version": 1,
            "engine": "MOSS",
            "inference_seconds": int(run_row["wall_elapsed_ms"]) / 1000.0,
            "inference_contract": {
                "window_audio_sha256": str(run_row["audio_sha256"]).upper(),
                "window_duration_seconds": moss_window_duration,
                "language_requested": moss_native_decode["language_requested"],
                "language_resolved": moss_native_decode["language_resolved"],
                "language_resolution_source": moss_native_decode["source"],
                "model_sha256": str(run_row["model_sha256"]).upper(),
                "decode_parameters_json": moss_native_decode["decode_parameters_json"],
                "decode_parameters_sha256": moss_native_decode[
                    "decode_parameters_sha256"
                ],
            },
            "segments": [
                {
                    "start_seconds": int(row["start_ms"]) / 1000.0,
                    "end_seconds": int(row["end_ms"]) / 1000.0,
                    "speaker_id": str(row["speaker_label"]),
                    "text": str(row["raw_text"]),
                }
                for row in raw_rows
            ],
            "provenance": _provenance(
                role="moss_raw", run_id=run_id, source_commit=source_commit,
                producer_sha256=moss_sha, session_nonce=session_nonce, produced_at=produced_at,
            ),
        },
        "whisper_same_window": {
            "schema_version": 1,
            "engine": "WHISPER",
            "inference_contract": {
                "window_audio_sha256": str(whisper_window.get("sha256", "")).upper(),
                "window_duration_seconds": whisper_window_duration,
                "language_requested": whisper_language,
                "language_resolved": whisper_language,
                "language_resolution_source": whisper_contract["language_resolution_source"],
                "model_sha256": str(whisper_model.get("sha256", "")).upper(),
                "decode_parameters": whisper_contract["decode_parameters"],
                "decode_parameters_sha256": whisper_contract["decode_parameters_sha256"],
            },
            "segments": whisper_segments,
            "provenance": _provenance(
                role="whisper_same_window", run_id=run_id, source_commit=source_commit,
                producer_sha256=main_sha, session_nonce=session_nonce, produced_at=produced_at,
            ),
        },
        "corrected": {
            "schema_version": 1,
            "engine": "MOSS_CORRECTED",
            "segments": materialized,
            "manual_speaker_overrides": override_audits,
            "provenance": _provenance(
                role="corrected", run_id=run_id, source_commit=source_commit,
                producer_sha256=main_sha, session_nonce=session_nonce, produced_at=produced_at,
            ),
        },
        "activation_evidence": {
            "schema_version": 1,
            "status": "ACTIVATED",
            "candidate_source": "MOSS",
            "activation_kind": "HUMAN_CONFIRMED",
            "activation_id": activation_id,
            "meeting_id": meeting_id,
            "moss_run_id": moss_run_id,
            "activated_transcript_sha256": str(activation["activated_transcript_sha256"]),
            "provenance": _provenance(
                role="activation_evidence", run_id=run_id, source_commit=source_commit,
                producer_sha256=main_sha, session_nonce=session_nonce, produced_at=produced_at,
            ),
        },
        "summary_evidence": {
            "schema_version": 1,
            "status": "COMPLETED",
            "source_kind": "ACTIVE_MOSS_TRANSCRIPT",
            "model_family": "QWEN_2B",
            "generation_id": summary_id,
            "meeting_id": meeting_id,
            "moss_run_id": moss_run_id,
            "transcript_sha256": str(summary["transcript_sha256"]),
            "model_name": str(summary["model_name"]),
            "provenance": _provenance(
                role="summary_evidence", run_id=run_id, source_commit=source_commit,
                producer_sha256=main_sha, session_nonce=session_nonce, produced_at=produced_at,
            ),
        },
    }
    for role, path in artifacts.items():
        gate._write_json_exclusive(path, documents[role])
    return artifacts, {
        "meeting_id": meeting_id,
        "moss_run_id": moss_run_id,
        "activation_id": activation_id,
        "summary_generation_id": summary_id,
    }


def _copy_truth_package(
    *, repo: Path, run_directory: Path, truth_package_root: Path, positive_truth_source: Path
) -> dict[str, Path]:
    destination = run_directory / "truth"
    destination.mkdir()
    fixed = {
        "truth_package_manifest": (
            truth_package_root / "MANIFEST.json", "MANIFEST.json",
            gate.FROZEN_PACKAGE_MANIFEST_BYTES, gate.FROZEN_PACKAGE_MANIFEST_SHA256,
        ),
        "human_verbatim": (
            truth_package_root / "04-human-verbatim.tsv", "04-human-verbatim.tsv",
            gate.FROZEN_HUMAN_VERBATIM_BYTES, gate.FROZEN_HUMAN_VERBATIM_SHA256,
        ),
        "speaker_truth": (
            truth_package_root / "05-human-speaker-turns.tsv", "05-human-speaker-turns.tsv",
            gate.FROZEN_SPEAKER_TRUTH_BYTES, gate.FROZEN_SPEAKER_TRUTH_SHA256,
        ),
        "human_review": (
            truth_package_root / "06-human-review.json", "06-human-review.json",
            gate.FROZEN_HUMAN_REVIEW_BYTES, gate.FROZEN_HUMAN_REVIEW_SHA256,
        ),
        "derived_review_provenance": (
            truth_package_root / "17-derived-human-review-provenance.json", "17-derived-human-review-provenance.json",
            gate.FROZEN_REVIEW_PROVENANCE_BYTES, gate.FROZEN_REVIEW_PROVENANCE_SHA256,
        ),
        "pre_meeting_context": (
            truth_package_root / "16-pre-meeting-context.json", "16-pre-meeting-context.json",
            gate.FROZEN_PREMEETING_CONTEXT_BYTES, gate.FROZEN_PREMEETING_CONTEXT_SHA256,
        ),
        "positive_truth": (
            positive_truth_source, "01-local-positive-terms-frozen.json",
            gate.FROZEN_POSITIVE_TRUTH_BYTES, gate.FROZEN_POSITIVE_TRUTH_SHA256,
        ),
        "negative_truth": (
            repo / "target" / "release" / "docs" / "方案" / "MOSS功能修复计划-20260902" / "Q00-NEGATIVE-TRUTH.json",
            "Q00-NEGATIVE-TRUTH.json", gate.FROZEN_NEGATIVE_TRUTH_BYTES, gate.FROZEN_NEGATIVE_TRUTH_SHA256,
        ),
        "manifest": (
            repo / "target" / "release" / "docs" / "方案" / "MOSS功能修复计划-20260902" / "Q00-FROZEN-TRUTH-MANIFEST.json",
            "Q00-FROZEN-TRUTH-MANIFEST.json", gate.FROZEN_TRUTH_MANIFEST_BYTES, gate.FROZEN_TRUTH_MANIFEST_SHA256,
        ),
    }
    copied: dict[str, Path] = {"root": destination}
    for role, (source, name, size, digest) in fixed.items():
        target = destination / name
        _copy_exclusive(source, target, size=size, sha256=digest, label=f"frozen truth {role}")
        copied[role] = target
    return copied


def _artifact_binding(
    *,
    path: Path,
    origin: str,
    source_commit: str,
    run_id: str,
    producer: Path,
    exporter: Path | None = None,
) -> dict[str, Any]:
    producer_record = _path_record(producer)
    binding = {
        "origin": origin,
        "source_commit": source_commit,
        "run_id": run_id,
        **gate._safe_file_record(path),
        "producer": {
            "executable_path": producer_record["path"],
            "bytes": producer_record["bytes"],
            "sha256": producer_record["sha256"],
        },
    }
    if exporter is not None:
        exporter_record = _path_record(exporter)
        binding["exporter"] = {
            "executable_path": exporter_record["path"],
            "bytes": exporter_record["bytes"],
            "sha256": exporter_record["sha256"],
        }
    return binding


def _owner_matches(path: Path, owner: dict[str, Any]) -> bool:
    marker = path / ".q00-owner.private.json"
    if not marker.is_file():
        return False
    return gate.read_json(marker, "Q00 isolated owner") == owner


def _cleanup_install(install: dict[str, Any] | None) -> dict[str, Any]:
    if install is None:
        return {"status": "NOT_INSTALLED", "completed_at": _utc_now()}
    errors = []
    uninstall_run = None
    uninstaller = install["install_root"] / gate.PROGRAM_CONTRACTS["uninstaller"]
    if uninstaller.is_file():
        cleanup_cwd = Path(os.environ["TEMP"]).resolve(strict=True)
        uninstall_run = _run_process(
            uninstaller,
            ["/S"],
            cwd=cleanup_cwd,
            timeout_seconds=1_200,
        )
        if uninstall_run["timed_out"] or uninstall_run["exit_code"] != 0:
            errors.append("isolated uninstaller failed")
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and (
        install["install_root"].exists() or _registry_state(gate.APPROVED_PRODUCT_NAME) is not None
    ):
        time.sleep(0.25)
    if install["install_root"].exists():
        errors.append("isolated install root remains")
    if _registry_state(gate.APPROVED_PRODUCT_NAME) is not None:
        errors.append("isolated uninstall registry key remains")
    for label in ("data_root", "webview_root"):
        path = install[label]
        if path.exists():
            if not _owner_matches(path, install["owner"]):
                errors.append(f"{label} owner marker mismatch; not removed")
                continue
            gate.reject_symlink(path, boundary=path.parent)
            shutil.rmtree(path)
        if path.exists():
            errors.append(f"{label} remains")
    if install["backup_root"].exists():
        errors.append("unexpected rollback backup root exists; not removed")
    return {
        "status": "PASS" if not errors else "FAIL",
        "completed_at": _utc_now(),
        "uninstall": uninstall_run,
        "errors": errors,
    }


def run_formal(args: argparse.Namespace) -> dict[str, Any]:
    repo = args.repo.resolve(strict=True)
    source_commit = gate._require_current_commit(repo, args.source_commit)
    node_value = shutil.which("node.exe") or shutil.which("node")
    if not node_value:
        raise gate.QualityGateError("Node.js runtime is unavailable")
    node = Path(node_value).resolve(strict=True)
    cdp_script = (repo / "scripts" / "qa" / "moss-functional-ft-cdp.mjs").resolve(strict=True)
    quality_script = (repo / "scripts" / "qa" / "moss_functional_fix_quality_gate.py").resolve(strict=True)
    formal_runner = Path(__file__).resolve(strict=True)
    manifest = _load_build_manifest(args.build_manifest, args.candidate_installer, source_commit)
    models = {"moss": args.moss_model, "whisper": args.whisper_model, "qwen_2b": args.qwen_model}
    for role, path in models.items():
        contract = gate.MODEL_CONTRACTS[role]
        gate._require_exact_file(
            path, size=int(contract["bytes"]), sha256=str(contract["sha256"]), label=f"formal {role} model"
        )
    gate._require_exact_file(
        args.moss_runtime_root / "contract.json",
        size=RUNTIME_CONTRACT_BYTES,
        sha256=RUNTIME_CONTRACT_SHA256,
        label="formal MOSS runtime contract",
    )
    private_root = args.private_root.resolve(strict=True)
    public_root = args.public_root.resolve(strict=True)
    session = gate.begin_formal_session(
        repo=repo,
        source_commit=source_commit,
        private_root=private_root,
        public_root=public_root,
        formal_runner_path=formal_runner,
        node_path=node,
        cdp_script_path=cdp_script,
    )
    run_id = session["run_id"]
    nonce = session["nonce"]
    run_directory = Path(session["run_directory"])
    session_path = Path(session["session_path"])
    truth = _copy_truth_package(
        repo=repo,
        run_directory=run_directory,
        truth_package_root=args.truth_package_root.resolve(strict=True),
        positive_truth_source=args.positive_truth_source.resolve(strict=True),
    )
    window_audio = run_directory / "q00-window.wav"
    window_manifest = run_directory / "q00-window-manifest.json"
    gate.prepare_window(
        source_wav=args.source_wav,
        output_wav=window_audio,
        manifest_path=window_manifest,
        source_commit=source_commit,
    )

    install = None
    app_process = None
    app_record = None
    runtime_cleanup = None
    final_error: Exception | None = None
    result: dict[str, Any] | None = None
    generated_reports: list[Path] = []
    try:
        install = _prepare_isolated_install(
            installer=args.candidate_installer,
            manifest=manifest,
            models=models,
            runtime_root=args.moss_runtime_root,
            run_id=run_id,
            source_commit=source_commit,
        )
        recording_root = run_directory / "recordings"
        recording_root.mkdir()
        raw_root = run_directory / "raw"
        raw_root.mkdir()
        state_path = run_directory / "product-state.private.json"
        gate._write_json_exclusive(
            state_path,
            {
                "schema_version": 1,
                "stage": "MOSS_FUNCTIONAL_FT_STATE",
                "run_id": run_id,
                "source_commit": source_commit,
                "candidate_sha256": str(manifest["installer"]["sha256"]).upper(),
                "created_at": _utc_now(),
            },
        )
        app_process, app_record = _start_app(install, args.cdp_port)
        state = gate.read_json(state_path, "Q00 product state")
        state["app_pid"] = app_record["pid"]
        state["cdp_port"] = args.cdp_port
        state["cdp_target_id"] = app_record["cdp_target_id"]
        _write_atomic(state_path, state)
        common = {
            "node": node,
            "cdp_script": cdp_script,
            "repo": repo,
            "state_path": state_path,
            "raw_root": raw_root,
            "port": args.cdp_port,
            "target_id": app_record["cdp_target_id"],
        }
        _invoke_cdp(
            **common,
            action="bootstrap",
            label="01-bootstrap",
            request={
                "whisper_model": Path(gate.MODEL_CONTRACTS["whisper"]["filename"]).stem.removeprefix("ggml-"),
                "qwen_model": gate.MODEL_CONTRACTS["qwen_2b"]["model_name"],
                "recording_root": str(recording_root),
            },
            timeout_seconds=300,
        )
        imported = _invoke_cdp(
            **common,
            action="import-audio",
            label="02-import-whisper",
            request={
                "audio_path": str(window_audio),
                "title": f"MOSS-Q00-{run_id}",
                "language": "zh-CN",
                "model": Path(gate.MODEL_CONTRACTS["whisper"]["filename"]).stem.removeprefix("ggml-"),
                "provider": "localWhisper",
                "timeout_ms": 3_600_000,
                "expected_duration_seconds": gate.WINDOW_DURATION_SECONDS,
            },
            timeout_seconds=3_700,
        )
        state = gate.read_json(state_path, "Q00 product state")
        meeting_folder = Path(gate.require_string(state.get("meeting_folder"), "Q00 meeting folder")).resolve(strict=True)
        gate._strict_child(meeting_folder, recording_root, "Q00 imported meeting folder")
        metadata_path = meeting_folder / "metadata.json"
        context = _attach_product_context(
            metadata_path=metadata_path,
            premeeting_context_path=truth["pre_meeting_context"],
            speaker_truth_path=truth["speaker_truth"],
            run_id=run_id,
        )
        marker = gate.mark_moss_start(session_path=session_path)
        moss = _invoke_cdp(
            **common,
            action="moss-complete",
            label="03-moss-complete",
            request={"timeout_ms": 1_700_000, "expected_duration_seconds": gate.WINDOW_DURATION_SECONDS},
            timeout_seconds=1_800,
        )
        candidate = gate.require_mapping(
            gate.require_mapping(
                gate.require_mapping(moss["document"].get("terminal"), "MOSS terminal").get("current"),
                "MOSS current workspace",
            ).get("review"),
            "MOSS current review",
        ).get("candidate")
        candidate = gate.require_mapping(candidate, "MOSS candidate")
        candidate_segments = gate.require_list(candidate.get("segments"), "MOSS candidate segments")
        (
            bindings,
            override_segment,
            wrong_override_person,
            correct_override_person,
        ) = _speaker_bindings(candidate_segments, truth["speaker_truth"])
        _invoke_cdp(
            **common,
            action="moss-review-q00",
            label="04a-moss-review-wrong-speaker",
            request={
                "speaker_bindings": bindings,
                "override_segment_id": override_segment,
                "override_person_id": wrong_override_person,
            },
            timeout_seconds=300,
        )
        _invoke_cdp(
            **common,
            action="moss-review-q00",
            label="04b-moss-review-correct-speaker",
            request={
                "speaker_bindings": bindings,
                "override_segment_id": override_segment,
                "override_person_id": correct_override_person,
            },
            timeout_seconds=300,
        )
        _invoke_cdp(
            **common,
            action="moss-activate",
            label="05-moss-activate",
            request={},
            timeout_seconds=180,
        )
        _invoke_cdp(
            **common,
            action="summary-generate",
            label="06-qwen-summary",
            request={"timeout_ms": 600_000},
            timeout_seconds=720,
        )
        runtime_cleanup = _stop_app(
            process=app_process,
            node=node,
            repo=repo,
            port=args.cdp_port,
            target_id=app_record["cdp_target_id"],
            install_root=install["install_root"],
            raw_root=raw_root,
        )
        app_process = None
        if runtime_cleanup["completed"] is not True:
            raise gate.QualityGateError("Q00 candidate process tree or CDP listener did not stop cleanly")
        snapshot_root = _snapshot_installed_files(install, run_directory, manifest)
        program_records = {
            role: gate._safe_file_record(snapshot_root / Path(*relative.split("/")))
            for role, relative in gate.PROGRAM_CONTRACTS.items()
        }
        database_path = _snapshot_database(install["data_root"], run_directory)
        native_decode_contract = gate.require_mapping(
            moss["document"].get("native_decode_contract"),
            "Q00 MOSS native decode contract from api_moss_get_workspace.runs",
        )
        whisper_request = gate.require_mapping(
            gate.read_json(imported["request_path"], "Q00 Whisper import request"),
            "Q00 Whisper import request",
        )
        moss_request = gate.require_mapping(
            gate.read_json(moss["request_path"], "Q00 MOSS run request"),
            "Q00 MOSS run request",
        )
        whisper_input = Path(
            gate.require_string(
                whisper_request.get("audio_path"), "Whisper input audio path"
            )
        ).resolve(strict=True)
        if not gate._same_path(whisper_input, window_audio):
            raise gate.QualityGateError("Whisper import did not use the prepared Q00 window")
        current_artifacts, identities = _export_product_evidence(
            database_path=database_path,
            state_path=state_path,
            run_directory=run_directory,
            run_id=run_id,
            source_commit=source_commit,
            session_nonce=nonce,
            program_records=program_records,
            speaker_truth_path=truth["speaker_truth"],
            transcription_run_evidence={
                "moss_native_decode_contract": native_decode_contract,
                "whisper_language_resolved": whisper_request.get("language"),
                "whisper_window": gate._safe_file_record(whisper_input),
                "whisper_model": gate._safe_file_record(models["whisper"]),
                "moss_window_duration_seconds": moss_request.get(
                    "expected_duration_seconds"
                ),
                "whisper_window_duration_seconds": whisper_request.get(
                    "expected_duration_seconds"
                ),
            },
        )
        receipt = gate.finish_formal_session(
            repo=repo,
            source_commit=source_commit,
            session_path=session_path,
            marker_path=Path(marker["marker_path"]),
            moss_completed_monotonic_ns=moss["completed_monotonic_ns"],
            runner_exit_code=moss["run"]["exit_code"],
            timed_out=moss["run"]["timed_out"],
            runner_arguments=moss["arguments"],
            runner_cwd=repo,
            app_process=app_record,
            cleanup=runtime_cleanup,
        )
        receipt_path = Path(receipt["receipt_path"])
        bindings_path = run_directory / "q00-bindings.json"
        all_artifacts = {
            "window_audio": window_audio,
            "window_manifest": window_manifest,
            "moss_raw": current_artifacts["moss_raw"],
            "whisper_same_window": current_artifacts["whisper_same_window"],
            "corrected": current_artifacts["corrected"],
            "human_verbatim": truth["human_verbatim"],
            "speaker_truth": truth["speaker_truth"],
            "positive_truth": truth["positive_truth"],
            "negative_truth": truth["negative_truth"],
            "activation_evidence": current_artifacts["activation_evidence"],
            "summary_evidence": current_artifacts["summary_evidence"],
        }
        producer_paths = {
            "window_audio": quality_script,
            "window_manifest": quality_script,
            "moss_raw": snapshot_root / gate.PROGRAM_CONTRACTS["moss_helper"],
            "whisper_same_window": snapshot_root / gate.PROGRAM_CONTRACTS["main_executable"],
            "corrected": snapshot_root / gate.PROGRAM_CONTRACTS["main_executable"],
            "human_verbatim": formal_runner,
            "speaker_truth": formal_runner,
            "positive_truth": formal_runner,
            "negative_truth": formal_runner,
            "activation_evidence": snapshot_root / gate.PROGRAM_CONTRACTS["main_executable"],
            "summary_evidence": snapshot_root / gate.PROGRAM_CONTRACTS["main_executable"],
        }
        completed_at = _utc_now()
        bindings_document = {
            "schema_version": 1,
            "stage": gate.BINDINGS_STAGE,
            "formal_short_gate": True,
            "current_run": True,
            "source_commit": source_commit,
            "run_id": run_id,
            "started_at": session["started_at"],
            "completed_at": completed_at,
            "window_audio_sha256": gate.WINDOW_SHA256,
            "artifacts": {
                role: _artifact_binding(
                    path=path,
                    origin=gate.ARTIFACT_ORIGINS[role],
                    source_commit=source_commit,
                    run_id=run_id,
                    producer=producer_paths[role],
                    exporter=formal_runner if role in gate.CURRENT_JSON_ROLES else None,
                )
                for role, path in all_artifacts.items()
            },
            "formal_evidence": {
                "session": _path_record(session_path),
                "execution_receipt": _path_record(receipt_path),
                "candidate": {
                    "execution_install_root": str(install["install_root"]),
                    "installed_snapshot_root": str(snapshot_root),
                    "build_manifest": _path_record(args.build_manifest),
                    "models": {role: _path_record(path) for role, path in models.items()},
                    "moss_runtime": install["runtime"],
                },
                "truth": {
                    "root": str(truth["root"]),
                    "manifest": _path_record(truth["manifest"]),
                    "human_review": _path_record(truth["human_review"]),
                    "pre_meeting_context": _path_record(truth["pre_meeting_context"]),
                    "truth_package_manifest": _path_record(truth["truth_package_manifest"]),
                    "derived_review_provenance": _path_record(truth["derived_review_provenance"]),
                },
                "product_state": {
                    "database": _path_record(database_path),
                    "snapshot_kind": "POST_CLEAN_EXIT_IMMUTABLE_MAIN_DATABASE",
                    "source_database_sha256": gate._safe_file_record(database_path)["sha256"],
                    "source_sidecars_absent": True,
                    **identities,
                    "product_context": context["metadata"],
                },
            },
            "bindings_path": str(bindings_path),
        }
        gate._write_json_exclusive(bindings_path, bindings_document)
        public_directory = public_root / "Q00"
        public_directory.mkdir(exist_ok=False)
        public_report = public_directory / "q00.public.json"
        private_report = run_directory / "q00.private.json"
        generated_reports = [private_report, public_report]
        public, _ = gate.score_files(
            artifact_paths=all_artifacts,
            bindings_path=bindings_path,
            public_report_path=public_report,
            private_report_path=private_report,
            source_commit=source_commit,
        )
        verified = gate.verify_files(
            artifact_paths=all_artifacts,
            bindings_path=bindings_path,
            public_report_path=public_report,
            private_report_path=private_report,
            source_commit=source_commit,
        )
        if public["status"] != gate.PASS or verified != gate.PASS:
            raise gate.QualityGateError(f"Formal Q00 hard gate did not pass: {public['failed_gates']}")
        result = {
            "schema_version": 1,
            "stage": FORMAL_RUNNER_STAGE,
            "status": gate.PASS,
            "run_id": run_id,
            "source_commit": source_commit,
            "bindings": _path_record(bindings_path),
            "public_report": _path_record(public_report),
            "private_report": _path_record(private_report),
            "completed_at": _utc_now(),
        }
    except Exception as exc:
        final_error = exc
    finally:
        if app_process is not None and app_process.poll() is None and install is not None and app_record is not None:
            try:
                _stop_app(
                    process=app_process,
                    node=node,
                    repo=repo,
                    port=args.cdp_port,
                    target_id=app_record["cdp_target_id"],
                    install_root=install["install_root"],
                    raw_root=run_directory / "raw",
                )
            except Exception as exc:
                if final_error is None:
                    final_error = exc
        cleanup = _cleanup_install(install)
        gate._write_json_exclusive(run_directory / "q00-isolated-cleanup.private.json", cleanup)
        if cleanup["status"] not in {"PASS", "NOT_INSTALLED"} and final_error is None:
            final_error = gate.QualityGateError(f"Formal Q00 isolated cleanup failed: {cleanup['errors']}")
        if final_error is not None:
            for report in generated_reports:
                try:
                    report.unlink()
                except FileNotFoundError:
                    pass
            if generated_reports:
                try:
                    generated_reports[-1].parent.rmdir()
                except OSError:
                    pass
    if final_error is not None:
        raise final_error
    if result is None:
        raise gate.QualityGateError("Formal Q00 completed without a result")
    return result


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--candidate-installer", type=Path, required=True)
    parser.add_argument("--build-manifest", type=Path, required=True)
    parser.add_argument("--source-wav", type=Path, required=True)
    parser.add_argument("--truth-package-root", type=Path, required=True)
    parser.add_argument("--positive-truth-source", type=Path, required=True)
    parser.add_argument("--moss-model", type=Path, required=True)
    parser.add_argument("--whisper-model", type=Path, required=True)
    parser.add_argument("--qwen-model", type=Path, required=True)
    parser.add_argument("--moss-runtime-root", type=Path, required=True)
    parser.add_argument("--private-root", type=Path, required=True)
    parser.add_argument("--public-root", type=Path, required=True)
    parser.add_argument("--cdp-port", type=int, required=True, choices=range(1024, 65536), metavar="PORT")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    try:
        result = run_formal(build_parser().parse_args(argv))
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except (gate.GateError, OSError, ValueError, subprocess.SubprocessError) as exc:
        print(json.dumps({"status": gate.FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
