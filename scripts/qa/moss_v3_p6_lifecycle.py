#!/usr/bin/env python3
"""Capture and compare P6 install/upgrade/uninstall/rollback state.

Snapshots contain paths relative to named roots, hashes, row counts, and row-set
hashes.  They never serialize meeting, transcript, setting, or template values.
"""

from __future__ import annotations

import argparse
import base64
from collections import Counter
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
from typing import Any, Iterable
from urllib.parse import quote

from moss_v3_p6_common import (
    FAIL,
    PASS,
    REQUIRED_LIFECYCLE_CHECKPOINTS,
    REQUIRED_LIFECYCLE_TRANSITIONS,
    GateError,
    atomic_write_json,
    canonical_json_bytes,
    file_record,
    normalize_relative_path,
    read_json,
    records_sha256,
    reject_symlink,
    require_git_commit,
    require_list,
    require_mapping,
    require_sha256,
    require_string,
    sha256_bytes,
    sha256_file,
    status_exit_code,
    tree_records,
    utc_now,
)


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


REQUIRED_TRANSITIONS = REQUIRED_LIFECYCLE_TRANSITIONS
REQUIRED_CHECKPOINTS = REQUIRED_LIFECYCLE_CHECKPOINTS
REQUIRED_TRANSITION_CHECKPOINTS = {
    "install": ("before_install", "after_install"),
    "upgrade": ("before_upgrade", "after_upgrade"),
    "uninstall": ("after_upgrade", "after_uninstall"),
    "rollback": ("before_rollback", "after_rollback"),
}
ROOT_KINDS = {"tree", "json", "sqlite"}
ROOT_CLASSIFICATIONS = {"protected", "application", "moss_owned"}
REQUIRED_ROOT_CONTRACT = {
    "application": ("tree", "application"),
    "recordings": ("tree", "protected"),
    "database": ("sqlite", "protected"),
    "settings": ("json", "protected"),
    "templates": ("tree", "protected"),
    "whisper_models": ("tree", "protected"),
    "parakeet_models": ("tree", "protected"),
    "qwen_models": ("tree", "protected"),
    "gemma_models": ("tree", "protected"),
    "moss_data": ("tree", "moss_owned"),
}
TREE_PROTECTION_MODES = {"exact", "preserve"}
JSON_PROTECTION_MODES = {"exact", "json_preserve"}
SQLITE_PROTECTION_MODES = {"sqlite_preserve"}
MOSS_POLICY_MODES = {
    "exact",
    "preserve",
    "manifest_present",
    "manifest_exact",
    "manifest_removed",
    "absent",
}


def git_head(repo: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise GateError(f"Cannot resolve Git HEAD: {completed.stderr.strip()}")
    return require_git_commit(completed.stdout.strip(), "Git HEAD")


def json_pointer_escape(value: str) -> str:
    return value.replace("~", "~0").replace("/", "~1")


def flatten_json_hashes(value: Any, pointer: str = "") -> list[dict[str, str]]:
    records: list[dict[str, str]] = []
    if isinstance(value, dict):
        if not value:
            records.append({"pointer": pointer or "/", "value_sha256": sha256_bytes(b"{}")})
        for key in sorted(value):
            child_pointer = f"{pointer}/{json_pointer_escape(str(key))}"
            records.extend(flatten_json_hashes(value[key], child_pointer))
    elif isinstance(value, list):
        if not value:
            records.append({"pointer": pointer or "/", "value_sha256": sha256_bytes(b"[]")})
        for index, child in enumerate(value):
            records.extend(flatten_json_hashes(child, f"{pointer}/{index}"))
    else:
        records.append(
            {
                "pointer": pointer or "/",
                "value_sha256": sha256_bytes(canonical_json_bytes(value)),
            }
        )
    return records


def sqlite_value(value: Any) -> Any:
    if value is None or isinstance(value, (str, int, float)):
        return value
    if isinstance(value, bytes):
        return {"blob_base64": base64.b64encode(value).decode("ascii")}
    raise GateError(f"Unsupported SQLite value type: {type(value).__name__}")


def quote_identifier(value: str) -> str:
    return '"' + value.replace('"', '""') + '"'


def sqlite_snapshot(path: Path, tables: list[str] | None) -> dict[str, Any]:
    reject_symlink(path)
    if not path.is_file():
        return {"exists": False, "integrity": "NOT RUN", "tables": []}
    uri = f"file:{quote(str(path.resolve()).replace('\\', '/'), safe='/:')}?mode=ro"
    connection = sqlite3.connect(uri, uri=True, timeout=5.0)
    try:
        # Hold one read transaction so every table is observed from the same
        # database state even if an unexpected background writer appears.
        connection.execute("BEGIN")
        integrity_rows = [str(row[0]) for row in connection.execute("PRAGMA integrity_check")]
        integrity = "ok" if integrity_rows == ["ok"] else sha256_bytes(
            canonical_json_bytes(integrity_rows)
        )
        user_version = int(connection.execute("PRAGMA user_version").fetchone()[0])
        actual_tables = {
            str(row[0]): str(row[1] or "")
            for row in connection.execute(
                "SELECT name, sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
            )
        }
        selected_tables = sorted(actual_tables) if tables is None else tables
        missing = sorted(set(selected_tables) - set(actual_tables))
        if missing:
            raise GateError(f"Configured SQLite tables are missing: {missing}")
        table_records: list[dict[str, Any]] = []
        for table in selected_tables:
            column_rows = list(connection.execute(f"PRAGMA table_info({quote_identifier(table)})"))
            columns = [str(row[1]) for row in column_rows]
            column_contract = [
                {
                    "name": str(row[1]),
                    "type": str(row[2] or ""),
                    "not_null": bool(row[3]),
                    "default_sha256": (
                        sha256_bytes(canonical_json_bytes(str(row[4]))) if row[4] is not None else None
                    ),
                    "primary_key_order": int(row[5]),
                }
                for row in column_rows
            ]
            row_hashes: list[str] = []
            projection_hashes: dict[str, list[str]] = {
                str(width): [] for width in range(1, len(columns) + 1)
            }
            cursor = connection.execute(f"SELECT * FROM {quote_identifier(table)}")
            for row in cursor:
                encoded = [sqlite_value(value) for value in row]
                full_hash = sha256_bytes(canonical_json_bytes(encoded))
                row_hashes.append(full_hash)
                for width in range(1, len(encoded) + 1):
                    projection_hashes[str(width)].append(
                        sha256_bytes(canonical_json_bytes(encoded[:width]))
                    )
            row_hashes.sort()
            for hashes in projection_hashes.values():
                hashes.sort()
            table_records.append(
                {
                    "name": table,
                    "columns": columns,
                    "column_contract": column_contract,
                    "schema_sha256": sha256_bytes(actual_tables[table].encode("utf-8")),
                    "row_count": len(row_hashes),
                    "rowset_sha256": sha256_bytes(canonical_json_bytes(row_hashes)),
                    "row_projection_hashes": projection_hashes,
                }
            )
        sidecars = []
        for suffix in ("-wal", "-shm"):
            sidecar = Path(str(path) + suffix)
            if sidecar.is_file():
                sidecars.append(file_record(sidecar, relative_path=path.name + suffix))
        return {
            "exists": True,
            "file": file_record(path, relative_path=path.name),
            "sidecars": sidecars,
            "integrity": integrity,
            "user_version": user_version,
            "tables": table_records,
        }
    finally:
        connection.close()


def snapshot_root(raw_root: Any) -> dict[str, Any]:
    root = require_mapping(raw_root, "snapshot root")
    root_id = require_string(root.get("id"), "snapshot root id")
    kind = require_string(root.get("kind"), f"root {root_id} kind")
    classification = require_string(root.get("classification"), f"root {root_id} classification")
    if kind not in ROOT_KINDS:
        raise GateError(f"root {root_id} has unsupported kind {kind!r}")
    if classification not in ROOT_CLASSIFICATIONS:
        raise GateError(f"root {root_id} has unsupported classification {classification!r}")
    required = root.get("required") is True
    path = Path(
        os.path.abspath(Path(require_string(root.get("path"), f"root {root_id} path")))
    )
    base = {
        "id": root_id,
        "kind": kind,
        "classification": classification,
        "required": required,
    }
    if kind == "tree":
        exists = path.is_dir()
        records = tree_records(path) if exists else []
        return {
            **base,
            "exists": exists,
            "file_count": len(records),
            "bytes": sum(int(item["bytes"]) for item in records),
            "records": records,
            "content_sha256": records_sha256(records),
        }
    if kind == "json":
        exists = path.is_file()
        if not exists:
            return {**base, "exists": False, "file": None, "values": [], "content_sha256": None}
        reject_symlink(path)
        value = read_json(path, f"JSON root {root_id}")
        values = flatten_json_hashes(value)
        return {
            **base,
            "exists": True,
            "file": file_record(path, relative_path=path.name),
            "values": values,
            "content_sha256": sha256_bytes(canonical_json_bytes(values)),
        }
    table_spec = root.get("tables")
    if table_spec == "*":
        tables = None
    else:
        table_values = require_list(table_spec, f"root {root_id} tables")
        tables = [require_string(value, f"root {root_id} table") for value in table_values]
        if not tables or len(set(tables)) != len(tables):
            raise GateError(f"root {root_id} must list unique SQLite tables or use '*' ")
    return {**base, **sqlite_snapshot(path, tables)}


def command_snapshot(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    config_path = args.config.resolve()
    config_sha256_before = sha256_file(config_path)
    config = require_mapping(read_json(config_path, "lifecycle snapshot config"), "snapshot config")
    config_sha256_after_read = sha256_file(config_path)
    if config_sha256_before != config_sha256_after_read:
        raise GateError("lifecycle snapshot config changed while it was being read")
    if config.get("template_only") is True:
        raise GateError("lifecycle snapshot template must be copied and fully resolved before capture")
    if config.get("schema_version") != 1:
        raise GateError("snapshot config schema_version must be 1")
    raw_roots = require_list(config.get("roots"), "snapshot roots")
    roots: list[dict[str, Any]] = []
    errors: list[dict[str, str]] = []
    ids: set[str] = set()
    for raw_root in raw_roots:
        root_id = str(raw_root.get("id", "<missing>")) if isinstance(raw_root, dict) else "<invalid>"
        try:
            captured = snapshot_root(raw_root)
            if captured["id"] in ids:
                raise GateError(f"duplicate root id {captured['id']}")
            ids.add(captured["id"])
            roots.append(captured)
        except GateError as exc:
            errors.append({"root": root_id, "error": str(exc)})
    missing_required = [
        root["id"] for root in roots if root["required"] and not root.get("exists", False)
    ]
    observed_contract = {
        str(root["id"]): (str(root["kind"]), str(root["classification"])) for root in roots
    }
    root_contract_failures = []
    for root_id, expected in REQUIRED_ROOT_CONTRACT.items():
        actual = observed_contract.get(root_id)
        if actual is None:
            root_contract_failures.append(f"missing required root {root_id}")
        elif actual != expected:
            root_contract_failures.append(
                f"root {root_id} must be kind={expected[0]} classification={expected[1]}"
            )
        else:
            captured = next(root for root in roots if root["id"] == root_id)
            if expected[1] == "protected" and captured.get("required") is not True:
                root_contract_failures.append(f"protected root {root_id} must be required")
    empty_required_tree_roots = [
        str(root["id"])
        for root in roots
        if root.get("required") is True
        and root.get("kind") == "tree"
        and root.get("exists") is True
        and int(root.get("file_count", 0)) == 0
    ]
    empty_required_sqlite_roots = [
        str(root["id"])
        for root in roots
        if root.get("required") is True
        and root.get("kind") == "sqlite"
        and root.get("exists") is True
        and (
            not root.get("tables")
            or sum(int(table.get("row_count", 0)) for table in root.get("tables", [])) == 0
        )
    ]
    database_failures = [
        root["id"]
        for root in roots
        if root["kind"] == "sqlite" and root.get("exists") and root.get("integrity") != "ok"
    ]
    config_unchanged = sha256_file(config_path) == config_sha256_after_read
    if not config_unchanged:
        errors.append({"root": "<config>", "error": "snapshot config changed during capture"})
    status = (
        PASS
        if not errors
        and not missing_required
        and not empty_required_tree_roots
        and not empty_required_sqlite_roots
        and not database_failures
        and not root_contract_failures
        else FAIL
    )
    root_contract = [
        {
            "id": root["id"],
            "kind": root["kind"],
            "classification": root["classification"],
            "required": root["required"],
        }
        for root in sorted(roots, key=lambda item: str(item["id"]))
    ]
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_LIFECYCLE_SNAPSHOT",
        "generated_at": utc_now(),
        "source_commit": git_head(repo),
        "checkpoint": require_string(args.checkpoint, "checkpoint"),
        "status": status,
        "config_sha256": config_sha256_after_read,
        "config_unchanged_during_capture": config_unchanged,
        "root_contract_sha256": sha256_bytes(canonical_json_bytes(root_contract)),
        "roots": sorted(roots, key=lambda item: str(item["id"])),
        "missing_required_roots": missing_required,
        "empty_required_tree_roots": empty_required_tree_roots,
        "empty_required_sqlite_roots": empty_required_sqlite_roots,
        "database_integrity_failures": database_failures,
        "root_contract_failures": root_contract_failures,
        "errors": errors,
    }
    atomic_write_json(args.output.resolve(), report)
    print(json.dumps({"status": status, "checkpoint": args.checkpoint, "output": str(args.output.resolve())}))
    return status_exit_code(status)


def index_records(root: dict[str, Any]) -> dict[str, tuple[int, str]]:
    return {
        str(item["path"]): (int(item["bytes"]), require_sha256(item["sha256"], "record sha256"))
        for item in root.get("records", [])
    }


def index_json_values(root: dict[str, Any]) -> dict[str, str]:
    return {
        str(item["pointer"]): require_sha256(item["value_sha256"], "JSON value sha256")
        for item in root.get("values", [])
    }


def index_sqlite_tables(root: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {str(item["name"]): item for item in root.get("tables", [])}


def assertion_result(
    assertion: dict[str, Any],
    before_root: dict[str, Any],
    after_root: dict[str, Any],
    release_manifest: dict[str, Any],
) -> tuple[bool, str]:
    mode = require_string(assertion.get("mode"), "lifecycle assertion mode")
    if mode == "exact":
        return before_root == after_root, "root snapshot changed"
    if mode == "preserve":
        before = index_records(before_root)
        after = index_records(after_root)
        lost = sorted(path for path, value in before.items() if after.get(path) != value)
        return not lost, f"lost or changed paths={lost}"
    if mode == "json_preserve":
        before = index_json_values(before_root)
        after = index_json_values(after_root)
        lost = sorted(pointer for pointer, value in before.items() if after.get(pointer) != value)
        return not lost, f"lost or changed JSON pointers={lost}"
    if mode == "sqlite_preserve":
        if before_root.get("integrity") != "ok" or after_root.get("integrity") != "ok":
            return False, "SQLite integrity_check is not ok"
        before = index_sqlite_tables(before_root)
        after = index_sqlite_tables(after_root)
        changed = []
        for name, table in before.items():
            candidate = after.get(name)
            before_columns = table.get("columns", [])
            candidate_columns = candidate.get("columns", []) if candidate is not None else []
            before_contract = table.get("column_contract", [])
            candidate_contract = candidate.get("column_contract", []) if candidate is not None else []
            width = str(len(before_columns))
            before_hashes = table.get("row_projection_hashes", {}).get(width, [])
            candidate_hashes = (
                candidate.get("row_projection_hashes", {}).get(width, [])
                if candidate is not None
                else []
            )
            missing_rows = Counter(before_hashes) - Counter(candidate_hashes)
            if (
                candidate is None
                or candidate_columns[: len(before_columns)] != before_columns
                or candidate_contract[: len(before_contract)] != before_contract
                or bool(missing_rows)
            ):
                changed.append(name)
        return not changed, f"missing, rewritten, or destructively changed SQLite tables={changed}"
    if mode == "absent":
        return after_root.get("exists") is False, "root still exists"
    if mode == "nonempty":
        if after_root.get("kind") == "tree":
            return after_root.get("exists") is True and int(after_root.get("file_count", 0)) > 0, "tree is empty"
        return after_root.get("exists") is True, "root is absent"
    if mode == "changed":
        before_digest = before_root.get("content_sha256") or before_root.get("file", {}).get("sha256")
        after_digest = after_root.get("content_sha256") or after_root.get("file", {}).get("sha256")
        return before_digest != after_digest, "root did not change"
    if mode in {"manifest_present", "manifest_exact", "manifest_removed"}:
        manifest_root = require_string(assertion.get("manifest_root"), "manifest_root")
        expected = {
            str(item["path"]): (int(item["bytes"]), require_sha256(item["sha256"], "manifest sha256"))
            for item in release_manifest.get("files", [])
            if item.get("root") == manifest_root
        }
        if not expected:
            return False, f"release manifest root {manifest_root!r} has no files"
        actual = index_records(after_root)
        if mode == "manifest_present":
            mismatches = sorted(path for path, value in expected.items() if actual.get(path) != value)
            return not mismatches, f"manifest files absent or changed={mismatches}"
        if mode == "manifest_exact":
            missing_or_changed = sorted(
                path for path, value in expected.items() if actual.get(path) != value
            )
            unexpected = sorted(set(actual) - set(expected))
            return (
                not missing_or_changed and not unexpected,
                f"manifest files absent or changed={missing_or_changed}, unexpected={unexpected}",
            )
        remaining = sorted(path for path in expected if path in actual)
        return not remaining, f"manifest-owned files remain={remaining}"
    if mode == "no_moss_assets":
        deployment_roots = require_mapping(
            release_manifest.get("deployment_roots", {}), "release deployment roots"
        )
        moss_root_ids = {
            str(root_id)
            for root_id, raw_root in deployment_roots.items()
            if isinstance(raw_root, dict) and raw_root.get("classification") == "moss_data"
        }
        moss_hashes = {
            require_sha256(item["sha256"], "MOSS asset sha256")
            for item in release_manifest.get("files", [])
            if str(item.get("root")) in moss_root_ids
        }
        if len(moss_root_ids) != 1 or not moss_hashes:
            return False, "release manifest does not identify one non-empty MOSS data root"
        collisions = [
            str(item["path"])
            for item in after_root.get("records", [])
            if require_sha256(item["sha256"], "application file sha256") in moss_hashes
            or str(item["path"]).lower().endswith(".gguf")
        ]
        return not collisions, f"MOSS model/runtime appeared in application root={collisions}"
    raise GateError(f"unsupported lifecycle assertion mode {mode!r}")


def parse_snapshot_arguments(values: list[str]) -> dict[str, Path]:
    snapshots: dict[str, Path] = {}
    for value in values:
        if "=" not in value:
            raise GateError("--snapshot must use CHECKPOINT=PATH")
        checkpoint, raw_path = value.split("=", 1)
        checkpoint = require_string(checkpoint, "snapshot checkpoint")
        if checkpoint in snapshots:
            raise GateError(f"duplicate snapshot checkpoint {checkpoint}")
        snapshots[checkpoint] = Path(raw_path).resolve()
    return snapshots


def stripped_root(root: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in root.items() if key not in {"id", "kind", "classification", "required"}}


def command_verify(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    head = git_head(repo)
    spec = require_mapping(read_json(args.spec.resolve(), "lifecycle spec"), "lifecycle spec")
    if spec.get("template_only") is True:
        raise GateError("lifecycle template is not executable evidence")
    if spec.get("schema_version") != 1:
        raise GateError("lifecycle spec schema_version must be 1")
    snapshot_paths = parse_snapshot_arguments(args.snapshot)
    if set(snapshot_paths) != set(REQUIRED_CHECKPOINTS) or len(snapshot_paths) != len(
        REQUIRED_CHECKPOINTS
    ):
        raise GateError(
            "lifecycle verification requires exactly the seven checkpoints: "
            f"{list(REQUIRED_CHECKPOINTS)}"
        )
    snapshots: dict[str, dict[str, Any]] = {}
    for checkpoint, path in snapshot_paths.items():
        document = require_mapping(read_json(path, f"snapshot {checkpoint}"), f"snapshot {checkpoint}")
        if (
            document.get("schema_version") != 1
            or document.get("stage") != "MOSS_V3_P6_LIFECYCLE_SNAPSHOT"
            or document.get("checkpoint") != checkpoint
            or document.get("status") != PASS
            or document.get("source_commit") != head
        ):
            raise GateError(f"snapshot {checkpoint} is not a PASS report bound to release HEAD")
        snapshots[checkpoint] = document
    if not snapshots:
        raise GateError("at least one lifecycle snapshot is required")
    contract_hashes = {str(document.get("root_contract_sha256")) for document in snapshots.values()}
    config_hashes = {str(document.get("config_sha256")) for document in snapshots.values()}
    if len(contract_hashes) != 1 or len(config_hashes) != 1:
        raise GateError("all lifecycle snapshots must use the same root contract and config")

    release_manifest = require_mapping(
        read_json(args.release_manifest.resolve(), "release manifest"), "release manifest"
    )
    if (
        release_manifest.get("stage") != "MOSS_V3_P6_RELEASE_MANIFEST"
        or release_manifest.get("status") != PASS
        or release_manifest.get("source_commit") != head
    ):
        raise GateError("release manifest is not PASS or is not bound to release HEAD")

    transitions = require_list(spec.get("transitions"), "lifecycle transitions")
    ids = [str(item.get("id")) for item in transitions if isinstance(item, dict)]
    if set(ids) != set(REQUIRED_TRANSITIONS) or len(ids) != len(REQUIRED_TRANSITIONS):
        raise GateError(f"lifecycle spec must contain exactly {list(REQUIRED_TRANSITIONS)}")
    for raw_transition in transitions:
        transition = require_mapping(raw_transition, "lifecycle transition")
        transition_id = require_string(transition.get("id"), "transition id")
        expected_before, expected_after = REQUIRED_TRANSITION_CHECKPOINTS[transition_id]
        if transition.get("before") != expected_before or transition.get("after") != expected_after:
            raise GateError(
                f"{transition_id} must compare {expected_before} to {expected_after}"
            )

    first_snapshot = next(iter(snapshots.values()))
    root_contract = {
        str(root["id"]): {
            "kind": root["kind"],
            "classification": root["classification"],
        }
        for root in first_snapshot.get("roots", [])
    }
    checks: dict[str, bool] = {}
    results: list[dict[str, Any]] = []
    coverage_failures: list[str] = []
    for raw_transition in transitions:
        transition = require_mapping(raw_transition, "lifecycle transition")
        transition_id = require_string(transition.get("id"), "transition id")
        before_name = require_string(transition.get("before"), f"{transition_id} before")
        after_name = require_string(transition.get("after"), f"{transition_id} after")
        if before_name not in snapshots or after_name not in snapshots:
            raise GateError(f"{transition_id} references a missing snapshot")
        before_roots = {str(item["id"]): item for item in snapshots[before_name]["roots"]}
        after_roots = {str(item["id"]): item for item in snapshots[after_name]["roots"]}
        assertions = require_list(transition.get("assertions"), f"{transition_id} assertions")
        by_root: dict[str, set[str]] = {}
        transition_results: list[dict[str, Any]] = []
        for index, raw_assertion in enumerate(assertions):
            assertion = require_mapping(raw_assertion, f"{transition_id} assertion {index}")
            root_id = require_string(assertion.get("root"), f"{transition_id} assertion root")
            mode = require_string(assertion.get("mode"), f"{transition_id} assertion mode")
            if root_id not in root_contract or root_id not in before_roots or root_id not in after_roots:
                raise GateError(f"{transition_id} assertion references unknown root {root_id}")
            by_root.setdefault(root_id, set()).add(mode)
            passed, detail = assertion_result(
                assertion,
                stripped_root(before_roots[root_id]),
                stripped_root(after_roots[root_id]),
                release_manifest,
            )
            check_id = f"{transition_id}:{root_id}:{mode}"
            checks[check_id] = passed
            transition_results.append(
                {"root": root_id, "mode": mode, "status": PASS if passed else FAIL, "detail": detail}
            )

        for root_id, contract in root_contract.items():
            modes = by_root.get(root_id, set())
            kind = contract["kind"]
            classification = contract["classification"]
            if classification == "protected":
                accepted = {
                    "tree": TREE_PROTECTION_MODES,
                    "json": JSON_PROTECTION_MODES,
                    "sqlite": SQLITE_PROTECTION_MODES,
                }[kind]
                if not modes.intersection(accepted):
                    coverage_failures.append(f"{transition_id}:{root_id}:protected root lacks strong assertion")
            elif classification == "moss_owned" and not modes.intersection(MOSS_POLICY_MODES):
                coverage_failures.append(f"{transition_id}:{root_id}:MOSS ownership policy is not explicit")
            elif classification == "application":
                if transition_id == "uninstall":
                    if "absent" not in modes:
                        coverage_failures.append(f"{transition_id}:{root_id}:application removal not asserted")
                else:
                    if not modes.intersection({"nonempty", "manifest_present", "manifest_exact"}):
                        coverage_failures.append(f"{transition_id}:{root_id}:installed payload not asserted")
                    if "no_moss_assets" not in modes:
                        coverage_failures.append(f"{transition_id}:{root_id}:D-drive asset placement not asserted")
        results.append(
            {
                "id": transition_id,
                "before": before_name,
                "after": after_name,
                "status": PASS if all(item["status"] == PASS for item in transition_results) else FAIL,
                "assertions": transition_results,
            }
        )

    checks["all_root_policies_covered"] = not coverage_failures
    status = PASS if all(checks.values()) else FAIL
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_LIFECYCLE",
        "generated_at": utc_now(),
        "source_commit": head,
        "status": status,
        "release_manifest_sha256": sha256_file(args.release_manifest.resolve()),
        "snapshot_config_sha256": next(iter(config_hashes)),
        "root_contract_sha256": next(iter(contract_hashes)),
        "snapshots": {
            checkpoint: {"sha256": sha256_file(snapshot_paths[checkpoint]), "status": document["status"]}
            for checkpoint, document in sorted(snapshots.items())
        },
        "transitions": results,
        "checks": checks,
        "coverage_failures": coverage_failures,
    }
    atomic_write_json(args.output.resolve(), report)
    print(json.dumps({"status": status, "output": str(args.output.resolve())}))
    return status_exit_code(status)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    snapshot = subparsers.add_parser("snapshot", help="capture one read-only lifecycle checkpoint")
    snapshot.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    snapshot.add_argument("--config", type=Path, required=True)
    snapshot.add_argument("--checkpoint", required=True)
    snapshot.add_argument("--output", type=Path, required=True)
    snapshot.set_defaults(handler=command_snapshot)

    verify = subparsers.add_parser("verify", help="compare the four required lifecycle transitions")
    verify.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    verify.add_argument("--spec", type=Path, required=True)
    verify.add_argument("--snapshot", action="append", default=[], metavar="CHECKPOINT=PATH")
    verify.add_argument("--release-manifest", type=Path, required=True)
    verify.add_argument("--output", type=Path, required=True)
    verify.set_defaults(handler=command_verify)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        return int(args.handler(args))
    except GateError as exc:
        print(json.dumps({"status": FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
