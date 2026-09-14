#!/usr/bin/env python3
"""Build and audit MOSS v3 P6 release evidence without inventing a release pass.

The commands in this file are deliberately read-only with respect to product and
user data.  They hash artifacts, validate manifests, and close evidence bundles.
Install/upgrade/uninstall execution belongs to ``moss_v3_p6_acceptance.py`` and
state comparison belongs to ``moss_v3_p6_lifecycle.py``.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import sys
from typing import Any, Callable, Iterable

from moss_v3_p6_common import (
    BLOCKED,
    FAIL,
    NOT_RUN,
    PASS,
    REQUIRED_LIFECYCLE_CHECKPOINTS,
    REQUIRED_LIFECYCLE_TRANSITIONS,
    REQUIRED_P6_SCENARIOS,
    GateError,
    atomic_write_json,
    canonical_json_bytes,
    checks_status,
    file_record,
    normalize_relative_path,
    read_json,
    records_sha256,
    reject_symlink,
    require_git_commit,
    require_list,
    require_mapping,
    require_sha256,
    require_status,
    require_string,
    resolve_under,
    sha256_bytes,
    sha256_file,
    status_exit_code,
    tree_records,
    utc_now,
)


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


P1_LOCK_RELATIVE = Path("scripts/qa/moss_v3_p1_lock.json")
REQUIRED_STAGES = ("P0", "P1", "P2", "P3", "P4", "P5")
PARALLEL_STAGE_BLOCKERS = ("P3", "P4", "P5")
RELEASE_DEPENDENCY_CHAIN = ("P2", "P3", "P4", "P5")
REQUIRED_EVIDENCE_FILES = (
    "TASKS.md",
    "00-source-state.json",
    "01-input-manifest.json",
    "02-environment.json",
    "03-run-results.json",
    "04-test-results.json",
    "05-data-integrity.json",
    "06-self-review.md",
)
REQUIRED_PACKAGE_ROLES = {
    "installer",
    "installer_signature",
    "application",
    "moss_helper",
    "llama_helper",
    "ffmpeg",
    "moss_model",
    "moss_runtime",
    "model_license",
    "runtime_license",
    "third_party_licenses",
    "database_migration",
    "release_notes",
    "authenticode_report",
}
MOSS_DATA_ROLES = {
    "moss_model",
    "moss_runtime",
    "model_license",
    "runtime_license",
    "third_party_licenses",
}
APPLICATION_ROLES = {"application", "moss_helper", "llama_helper", "ffmpeg"}
ROLE_CLASSIFICATIONS = {
    "installer": "package",
    "installer_signature": "package",
    "application": "application",
    "moss_helper": "application",
    "llama_helper": "application",
    "ffmpeg": "application",
    "moss_model": "moss_data",
    "moss_runtime": "moss_data",
    "model_license": "moss_data",
    "runtime_license": "moss_data",
    "third_party_licenses": "moss_data",
    "database_migration": "source",
    "release_notes": "evidence",
    "authenticode_report": "evidence",
}
COMPLETE_INVENTORY_CLASSIFICATIONS = {"package", "application", "moss_data"}
HUMAN_APPROVAL_ATTESTATION = (
    "I reviewed the complete reference against the full frozen business audio."
)
REQUIRED_METRIC_EVIDENCE_ROLES = {
    "moss_raw_transcript",
    "whisper_same_window_transcript",
    "corrected_transcript",
    "term_scoring",
    "speaker_scoring",
    "timestamp_scoring",
    "performance",
    "process_ordering",
}
WORKFLOW_PATHS = (
    ".github/workflows/build.yml",
    ".github/workflows/build-windows.yml",
    ".github/workflows/build-macos.yml",
    ".github/workflows/build-linux.yml",
    ".github/workflows/build-devtest.yml",
)
RELEASE_DRIVER_PATH = ".github/workflows/release.yml"


class CheckCollector:
    def __init__(self) -> None:
        self.checks: dict[str, bool] = {}
        self.failures: list[dict[str, str]] = []

    def add(self, name: str, passed: bool, detail: str = "") -> bool:
        self.checks[name] = bool(passed)
        if not passed:
            self.failures.append({"check": name, "detail": detail or "check failed"})
        return bool(passed)

    def attempt(self, name: str, operation: Callable[[], bool]) -> bool:
        try:
            return self.add(name, bool(operation()))
        except Exception as exc:  # Evidence output must preserve the exact failed gate.
            return self.add(name, False, f"{type(exc).__name__}: {exc}")


def git_output(repo: Path, arguments: Iterable[str], *, check: bool = True) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if check and completed.returncode != 0:
        raise GateError(
            f"Git command failed ({completed.returncode}): git {' '.join(arguments)}: "
            f"{completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def git_head(repo: Path) -> str:
    return require_git_commit(git_output(repo, ["rev-parse", "HEAD"]), "Git HEAD")


def git_is_ancestor(repo: Path, ancestor: str, descendant: str) -> bool:
    completed = subprocess.run(
        ["git", "-C", str(repo), "merge-base", "--is-ancestor", ancestor, descendant],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return completed.returncode == 0


def validate_release_dependency_chain(
    stages: dict[str, dict[str, Any]], repo: Path, head: str
) -> tuple[bool, list[str]]:
    """Require the release DAG to be P2 -> P3 -> P4 -> P5 -> P6 HEAD."""

    if any(stages.get(stage, {}).get("status") != PASS for stage in RELEASE_DEPENDENCY_CHAIN):
        return False, ["DEPENDENCY_CHAIN_REQUIRES_P2_P3_P4_P5_PASS"]
    errors: list[str] = []
    for parent, child in zip(RELEASE_DEPENDENCY_CHAIN, RELEASE_DEPENDENCY_CHAIN[1:]):
        parent_commit = str(stages[parent].get("source_commit", ""))
        child_commit = str(stages[child].get("source_commit", ""))
        if not git_is_ancestor(repo, parent_commit, child_commit):
            errors.append(f"{parent}_NOT_ANCESTOR_OF_{child}")
    p5_commit = str(stages["P5"].get("source_commit", ""))
    if not git_is_ancestor(repo, p5_commit, head):
        errors.append("P5_NOT_ANCESTOR_OF_P6_HEAD")
    return not errors, errors


def path_is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def windows_deployment_is_moss_data(path_value: Any, system_drive: str) -> bool:
    path = PureWindowsPath(require_string(path_value, "MOSS deployment root"))
    expected = PureWindowsPath(r"D:\MeetilyData")
    normalized_system_drive = system_drive.rstrip("\\/").casefold()
    return (
        path.is_absolute()
        and path.drive.casefold() == "d:"
        and path.drive.casefold() != normalized_system_drive
        and tuple(part.casefold() for part in path.parts[:2])
        == tuple(part.casefold() for part in expected.parts[:2])
    )


def command_assets(args: argparse.Namespace) -> int:
    output = args.output.resolve()
    collector = CheckCollector()
    repo = args.repo.resolve()
    source_commit = git_head(repo)
    lock = require_mapping(read_json(args.lock.resolve(), "P1 lock"), "P1 lock")
    model_lock = require_mapping(lock.get("model"), "P1 model lock")
    runtime_lock = require_mapping(lock.get("runtime"), "P1 runtime lock")
    runtime_hashes = require_mapping(runtime_lock.get("runtime_files"), "runtime file hashes")

    # Do not call resolve() before link checks: that would hide a symlink in an
    # artifact path and let evidence hash a different file than the one named.
    model = Path(os.path.abspath(args.model))
    model_license = Path(os.path.abspath(args.model_license))
    runtime = Path(os.path.abspath(args.runtime))
    runtime_license = Path(os.path.abspath(args.runtime_license))
    third_party_license = Path(os.path.abspath(args.runtime_third_party_license))
    data_root = Path(os.path.abspath(args.data_root))

    safe_paths: dict[str, bool] = {}
    for label, path in {
        "model": model,
        "model_license": model_license,
        "runtime": runtime,
        "runtime_license": runtime_license,
        "runtime_third_party_license": third_party_license,
    }.items():
        collector.add(
            f"{label}_under_data_root",
            path_is_under(path, data_root),
            "asset resolved outside the declared D-drive data root",
        )
        safe_paths[label] = collector.attempt(
            f"{label}_contains_no_symlink",
            lambda path=path: (reject_symlink(path, boundary=data_root) or True),
        )

    collector.attempt("deployment_root_is_D_MeetilyData", lambda: windows_deployment_is_moss_data(
        args.deployment_root, args.system_drive
    ))
    collector.add("p1_lock_schema", lock.get("schema_version") == 1)
    collector.add("p1_lock_stage", lock.get("stage") == "MOSS_V3_P1")
    collector.attempt(
        "model_revision_is_full_commit",
        lambda: bool(re.fullmatch(r"[0-9a-f]{40}", require_string(model_lock.get("revision"), "model revision"))),
    )
    collector.attempt(
        "runtime_source_commit_is_full_commit",
        lambda: bool(re.fullmatch(r"[0-9a-f]{40}", require_string(runtime_lock.get("source_commit"), "runtime source commit"))),
    )
    collector.add("runtime_backend_is_vulkan", runtime_lock.get("primary_backend") == "Intel Arc Vulkan")
    collector.add(
        "accuracy_truth_boundary_preserved",
        lock.get("accuracy_status") == "NOT_SCORABLE_UNTIL_P0_R_PASS",
    )

    model_record: dict[str, Any] | None = None
    if collector.add("model_file_exists", model.is_file(), "model file is missing") and safe_paths[
        "model"
    ]:
        model_record = file_record(model, relative_path=model.name)
        collector.add("model_name_matches_lock", model.name == model_lock.get("file_name"))
        collector.add("model_size_matches_lock", model_record["bytes"] == model_lock.get("bytes"))
        collector.add(
            "model_sha256_matches_lock",
            model_record["sha256"] == str(model_lock.get("sha256", "")).upper(),
        )

    model_license_record: dict[str, Any] | None = None
    if collector.add(
        "model_license_exists", model_license.is_file(), "model license is missing"
    ) and safe_paths["model_license"]:
        model_license_record = file_record(model_license, relative_path=model_license.name)
        collector.add(
            "model_license_size_matches_lock",
            model_license_record["bytes"] == model_lock.get("local_license_bytes"),
        )
        collector.add(
            "model_license_sha256_matches_lock",
            model_license_record["sha256"]
            == str(model_lock.get("local_license_sha256", "")).upper(),
        )

    runtime_records: list[dict[str, Any]] = []
    bundled_license_records: list[dict[str, Any]] = []
    actual_runtime_names: set[str] = set()
    if collector.add(
        "runtime_directory_exists", runtime.is_dir(), "runtime directory is missing"
    ) and safe_paths["runtime"]:
        runtime_symlinks = sorted(item.name for item in runtime.iterdir() if item.is_symlink())
        collector.add(
            "runtime_contains_no_symlinks",
            not runtime_symlinks,
            f"symlinks: {runtime_symlinks}",
        )
        unexpected_directories = sorted(
            item.name
            for item in runtime.iterdir()
            if not item.is_symlink() and item.is_dir() and item.name != "licenses"
        )
        collector.add(
            "runtime_contains_no_unapproved_directories",
            not unexpected_directories,
            f"unexpected directories: {unexpected_directories}",
        )
        for child in sorted(runtime.iterdir(), key=lambda item: item.name.casefold()):
            if not child.is_symlink() and child.is_file():
                actual_runtime_names.add(child.name)
                runtime_records.append(file_record(child, relative_path=child.name))
        expected_names = set(runtime_hashes)
        collector.add(
            "runtime_file_set_matches_lock",
            actual_runtime_names == expected_names,
            f"missing={sorted(expected_names - actual_runtime_names)}, "
            f"unexpected={sorted(actual_runtime_names - expected_names)}",
        )
        by_name = {str(item["path"]): item for item in runtime_records}
        mismatches = [
            name
            for name, expected_hash in runtime_hashes.items()
            if by_name.get(name, {}).get("sha256") != str(expected_hash).upper()
        ]
        collector.add("runtime_hashes_match_lock", not mismatches, f"mismatches={mismatches}")

        contract_path = runtime / "contract.json"
        contract: dict[str, Any] | None = None
        if collector.add("runtime_contract_exists", contract_path.is_file()):
            try:
                contract = require_mapping(
                    read_json(contract_path, "runtime contract"), "runtime contract"
                )
            except GateError as exc:
                collector.add("runtime_contract_valid_json_object", False, str(exc))
            else:
                collector.add("runtime_contract_valid_json_object", True)
        if contract is None:
            collector.add("runtime_contract_version", False, "contract is unavailable")
            collector.add("runtime_contract_backends", False, "contract is unavailable")
            collector.add("runtime_contract_lane", False, "contract is unavailable")
        else:
            collector.add(
                "runtime_contract_version", contract.get("version") == runtime_lock.get("version")
            )
            collector.attempt(
                "runtime_contract_backends",
                lambda: set(require_list(contract.get("backends"), "runtime contract backends"))
                == {"cpu", "vulkan"},
            )
            collector.add("runtime_contract_lane", contract.get("lane") == "cpu-vulkan")

        bundled_license_root = runtime / "licenses"
        if collector.add("bundled_runtime_license_directory_exists", bundled_license_root.is_dir()):
            bundled_license_records = tree_records(bundled_license_root)
            expected_bundled_sources = {
                "LICENSE": runtime_license,
                "ggml/LICENSE": runtime_license.parent / "ggml" / "LICENSE",
                "src/third_party/miniz/LICENSE": runtime_license.parent
                / "src"
                / "third_party"
                / "miniz"
                / "LICENSE",
            }
            collector.add(
                "bundled_runtime_license_file_set",
                {str(item["path"]) for item in bundled_license_records}
                == set(expected_bundled_sources),
            )
            bundled_by_path = {str(item["path"]): item for item in bundled_license_records}
            source_license_mismatches: list[str] = []
            for relative, source_path in expected_bundled_sources.items():
                try:
                    reject_symlink(source_path, boundary=data_root)
                    source_matches = source_path.is_file() and bundled_by_path.get(
                        relative, {}
                    ).get("sha256") == sha256_file(source_path)
                except GateError:
                    source_matches = False
                if not source_matches:
                    source_license_mismatches.append(relative)
            collector.add(
                "bundled_runtime_licenses_match_pinned_source",
                not source_license_mismatches,
                f"mismatches={source_license_mismatches}",
            )

    runtime_license_record: dict[str, Any] | None = None
    if collector.add("runtime_license_exists", runtime_license.is_file()) and safe_paths[
        "runtime_license"
    ]:
        runtime_license_record = file_record(runtime_license, relative_path=runtime_license.name)
        collector.add(
            "runtime_license_sha256_matches_lock",
            runtime_license_record["sha256"]
            == str(runtime_lock.get("license_sha256", "")).upper(),
        )
    third_party_record: dict[str, Any] | None = None
    if collector.add("third_party_license_exists", third_party_license.is_file()) and safe_paths[
        "runtime_third_party_license"
    ]:
        third_party_record = file_record(
            third_party_license, relative_path=third_party_license.name
        )
        collector.add(
            "third_party_license_sha256_matches_lock",
            third_party_record["sha256"]
            == str(runtime_lock.get("third_party_licenses_sha256", "")).upper(),
        )

    status = checks_status(collector.checks)
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_ASSET_INTEGRITY",
        "generated_at": utc_now(),
        "source_commit": source_commit,
        "status": status,
        "accuracy_status": "NOT_SCORABLE_UNTIL_HUMAN_APPROVED_FULL_REFERENCE",
        "deployment_root": str(PureWindowsPath(args.deployment_root)),
        "model": model_record,
        "model_license": model_license_record,
        "runtime": {
            "project": runtime_lock.get("project"),
            "version": runtime_lock.get("version"),
            "source_commit": runtime_lock.get("source_commit"),
            "file_count": len(runtime_records),
            "files": runtime_records,
            "manifest_sha256": records_sha256(runtime_records),
            "bundled_licenses": bundled_license_records,
            "bundled_licenses_manifest_sha256": records_sha256(bundled_license_records),
        },
        "runtime_license": runtime_license_record,
        "third_party_licenses": third_party_record,
        "checks": collector.checks,
        "failures": collector.failures,
    }
    atomic_write_json(output, report)
    print(json.dumps({"status": status, "output": str(output)}, ensure_ascii=False))
    return status_exit_code(status)


def root_configurations(spec: dict[str, Any]) -> dict[str, dict[str, Any]]:
    raw_roots = require_mapping(spec.get("roots"), "package roots")
    roots: dict[str, dict[str, Any]] = {}
    for root_id, raw_value in raw_roots.items():
        root_name = require_string(root_id, "root id")
        value = require_mapping(raw_value, f"root {root_name}")
        classification = require_string(value.get("classification"), f"root {root_name} classification")
        if classification not in {"package", "application", "moss_data", "source", "evidence"}:
            raise GateError(f"root {root_name} has unsupported classification {classification!r}")
        roots[root_name] = {
            "classification": classification,
            "source": Path(
                os.path.abspath(
                    Path(require_string(value.get("source"), f"root {root_name} source"))
                )
            ),
            "deployment_root": require_string(
                value.get("deployment_root"), f"root {root_name} deployment_root"
            ),
        }
    return roots


def command_package(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    output = args.output.resolve()
    source_commit = git_head(repo)
    spec = require_mapping(read_json(args.spec.resolve(), "release package spec"), "release package spec")
    if spec.get("template_only") is True:
        raise GateError("release package template must be copied and fully resolved before execution")
    collector = CheckCollector()
    collector.add("spec_schema", spec.get("schema_version") == 1)
    release = require_mapping(spec.get("release"), "release metadata")
    policy = require_mapping(spec.get("policy"), "release policy")
    roots = root_configurations(spec)
    files = require_list(spec.get("files"), "release files")

    declared_commit = require_git_commit(release.get("git_commit"), "release git_commit")
    collector.add("release_commit_matches_head", declared_commit == source_commit)
    collector.add("windows_vulkan_platform", release.get("platform") == "windows-x86_64-vulkan")
    collector.add("windows_x86_64_architecture", release.get("architecture") == "x86_64")
    collector.add("vulkan_acceleration", release.get("acceleration") == "vulkan")
    require_string(release.get("version"), "release version")
    build_command = require_list(release.get("build_command"), "release build_command")
    collector.add(
        "build_command_is_argv",
        bool(build_command) and all(isinstance(item, str) and item for item in build_command),
    )
    if policy.get("require_clean_tracked_tree") is True:
        dirty = git_output(repo, ["status", "--porcelain=v1", "--untracked-files=all"])
        collector.add("release_worktree_clean", not dirty, dirty)
    else:
        collector.add("tracked_tree_policy_explicit", policy.get("require_clean_tracked_tree") is False)

    system_drive = require_string(policy.get("system_drive", "C:"), "system_drive")
    inventory_classifications = {
        require_string(value, "complete inventory classification")
        for value in require_list(
            policy.get("complete_inventory_classifications"),
            "complete_inventory_classifications",
        )
    }
    collector.add(
        "complete_inventory_policy",
        inventory_classifications == COMPLETE_INVENTORY_CLASSIFICATIONS,
        f"required={sorted(COMPLETE_INVENTORY_CLASSIFICATIONS)}, "
        f"actual={sorted(inventory_classifications)}",
    )
    moss_roots = [value for value in roots.values() if value["classification"] == "moss_data"]
    collector.add("one_moss_data_root_declared", len(moss_roots) == 1)
    if moss_roots:
        collector.attempt(
            "moss_data_deploys_to_D_MeetilyData",
            lambda: windows_deployment_is_moss_data(
                moss_roots[0]["deployment_root"], system_drive
            ),
        )

    records: list[dict[str, Any]] = []
    ids: set[str] = set()
    targets: set[tuple[str, str]] = set()
    duplicate_targets: list[str] = []
    present_roles: set[str] = set()
    for index, raw_entry in enumerate(files):
        entry = require_mapping(raw_entry, f"release file {index}")
        file_id = require_string(entry.get("id"), f"release file {index} id")
        role = require_string(entry.get("role"), f"release file {file_id} role")
        root_id = require_string(entry.get("root"), f"release file {file_id} root")
        if file_id in ids:
            collector.add(f"unique_id_{file_id}", False, "duplicate file id")
            continue
        ids.add(file_id)
        if root_id not in roots:
            collector.add(f"known_root_{file_id}", False, root_id)
            continue
        root = roots[root_id]
        try:
            path, relative = resolve_under(
                root["source"], entry.get("path"), f"release file {file_id} path"
            )
            target = (root_id, relative.casefold())
            if target in targets:
                duplicate_targets.append(f"{root_id}:{relative}")
                collector.add(f"unique_target_{file_id}", False, f"duplicate target {root_id}:{relative}")
                continue
            targets.add(target)
            reject_symlink(path, boundary=root["source"])
            record = file_record(path, relative_path=relative)
        except GateError as exc:
            collector.add(f"file_{file_id}_readable", False, str(exc))
            continue
        record.update({"id": file_id, "role": role, "root": root_id})
        records.append(record)
        if entry.get("required", True) is True:
            present_roles.add(role)
        expected_hash = entry.get("expected_sha256")
        if expected_hash is not None:
            collector.add(
                f"file_{file_id}_expected_sha256",
                record["sha256"] == require_sha256(expected_hash, f"{file_id} expected_sha256"),
            )
        expected_bytes = entry.get("expected_bytes")
        if expected_bytes is not None:
            collector.add(
                f"file_{file_id}_expected_bytes",
                isinstance(expected_bytes, int)
                and not isinstance(expected_bytes, bool)
                and expected_bytes >= 0
                and record["bytes"] == expected_bytes,
            )
        classification = root["classification"]
        expected_classification = ROLE_CLASSIFICATIONS.get(role)
        if expected_classification is not None:
            collector.add(
                f"file_{file_id}_role_placement",
                classification == expected_classification,
                f"{role} was declared under {classification}, expected {expected_classification}",
            )
        if role in MOSS_DATA_ROLES:
            collector.add(
                f"file_{file_id}_moss_data_placement",
                classification == "moss_data",
                f"{role} was declared under {classification}",
            )
        if role in APPLICATION_ROLES:
            collector.add(
                f"file_{file_id}_application_placement",
                classification == "application",
                f"{role} was declared under {classification}",
            )

    missing_roles = sorted(REQUIRED_PACKAGE_ROLES - present_roles)
    collector.add("all_required_roles_present", not missing_roles, f"missing roles={missing_roles}")
    collector.add("file_ids_unique", len(ids) == len(files))
    collector.add(
        "file_targets_unique",
        not duplicate_targets,
        f"duplicate targets={duplicate_targets}",
    )
    collector.add("all_declared_files_readable", len(targets) == len(records))

    inventory_records_by_root: dict[str, list[dict[str, Any]]] = {}
    for root_id, root in roots.items():
        if root["classification"] not in COMPLETE_INVENTORY_CLASSIFICATIONS:
            continue
        try:
            actual_records = tree_records(root["source"])
            inventory_records_by_root[root_id] = actual_records
        except GateError as exc:
            collector.add(f"complete_inventory_{root_id}", False, str(exc))
            continue
        actual_by_path = {
            str(item["path"]): (int(item["bytes"]), str(item["sha256"]))
            for item in actual_records
        }
        declared_records = {
            str(item["path"]): (int(item["bytes"]), str(item["sha256"]))
            for item in records
            if item["root"] == root_id
        }
        collector.add(
            f"complete_inventory_{root_id}",
            actual_by_path == declared_records,
            f"undeclared={sorted(set(actual_by_path) - set(declared_records))}, "
            f"missing={sorted(set(declared_records) - set(actual_by_path))}, "
            f"changed={sorted(path for path in set(actual_by_path) & set(declared_records) if actual_by_path[path] != declared_records[path])}",
        )

    moss_root_ids = {
        root_id for root_id, root in roots.items() if root["classification"] == "moss_data"
    }
    moss_hashes = {
        record["sha256"] for record in records if str(record["root"]) in moss_root_ids
    }
    application_hash_collisions: list[str] = []
    application_gguf: list[str] = []
    for root_id, root in roots.items():
        if root["classification"] not in {"application", "package"} or not root["source"].is_dir():
            continue
        scan_records = inventory_records_by_root.get(root_id)
        if scan_records is None:
            scan_records = tree_records(root["source"])
        for record in scan_records:
            if record["sha256"] in moss_hashes:
                application_hash_collisions.append(f"{root_id}:{record['path']}")
            if str(record["path"]).lower().endswith(".gguf"):
                application_gguf.append(f"{root_id}:{record['path']}")
    collector.add(
        "moss_model_runtime_not_copied_into_application_or_package",
        not application_hash_collisions and not application_gguf,
        f"hash collisions={application_hash_collisions}, gguf={application_gguf}",
    )

    status = checks_status(collector.checks)
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_RELEASE_MANIFEST",
        "generated_at": utc_now(),
        "source_commit": source_commit,
        "status": status,
        "release": {
            "version": release["version"],
            "git_commit": declared_commit,
            "platform": release["platform"],
            "architecture": release["architecture"],
            "acceleration": release["acceleration"],
            "build_command": build_command,
        },
        "deployment_roots": {
            root_id: {
                "classification": root["classification"],
                "deployment_root": root["deployment_root"],
            }
            for root_id, root in sorted(roots.items())
        },
        "files": sorted(records, key=lambda item: str(item["id"])),
        "files_manifest_sha256": sha256_bytes(
            canonical_json_bytes(
                [
                    {
                        "id": item["id"],
                        "role": item["role"],
                        "root": item["root"],
                        "path": item["path"],
                        "bytes": item["bytes"],
                        "sha256": item["sha256"],
                    }
                    for item in sorted(records, key=lambda value: str(value["id"]))
                ]
            )
        ),
        "required_roles": sorted(REQUIRED_PACKAGE_ROLES),
        "checks": collector.checks,
        "failures": collector.failures,
    }
    atomic_write_json(output, report)
    print(json.dumps({"status": status, "output": str(output)}, ensure_ascii=False))
    return status_exit_code(status)


def command_build_wiring(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    output = args.output.resolve()
    collector = CheckCollector()
    config_path = repo / "frontend/src-tauri/tauri.conf.json"
    package_path = repo / "frontend/package.json"
    prepare_path = repo / "scripts/prepare-tauri-sidecars.ps1"
    config = require_mapping(read_json(config_path, "Tauri config"), "Tauri config")
    package = require_mapping(read_json(package_path, "frontend package"), "frontend package")
    external_bin = config.get("bundle", {}).get("externalBin", [])
    collector.add("tauri_external_bin_has_moss_helper", "binaries/moss-helper" in external_bin)
    collector.add("tauri_external_bin_has_llama_helper", "binaries/llama-helper" in external_bin)
    targets = set(config.get("bundle", {}).get("targets", []))
    collector.add("tauri_windows_nsis_target", "nsis" in targets)
    collector.add("tauri_windows_msi_target", "msi" in targets)
    windows = require_mapping(
        config.get("bundle", {}).get("windows"),
        "Tauri Windows bundle config",
    )
    webview_install_mode = require_mapping(
        windows.get("webviewInstallMode"),
        "Tauri WebView2 install mode",
    )
    collector.add(
        "tauri_windows_webview2_uses_fixed_runtime",
        webview_install_mode.get("type") == "fixedRuntime",
    )
    collector.add(
        "tauri_windows_webview2_fixed_runtime_path_is_pinned",
        webview_install_mode.get("path") == "runtime/webview2-fixed",
    )
    nsis = require_mapping(windows.get("nsis"), "Tauri NSIS config")
    collector.add(
        "tauri_windows_nsis_uses_fast_offline_zlib_compression",
        nsis.get("compression") == "zlib",
    )
    fixed_runtime_lock = repo / "frontend/src-tauri/runtime/webview2-fixed.lock.json"
    fixed_runtime_prepare = repo / "scripts/prepare-webview2-fixed.ps1"
    collector.add("webview2_fixed_runtime_lock_exists", fixed_runtime_lock.is_file())
    collector.add("webview2_fixed_runtime_prepare_exists", fixed_runtime_prepare.is_file())
    if fixed_runtime_lock.is_file():
        fixed_runtime_lock_data = read_json(fixed_runtime_lock)
        collector.add(
            "webview2_fixed_runtime_excludes_wix_incompatible_copilot_payload",
            fixed_runtime_lock_data.get("excluded_paths") == ["undocked_copilot"],
        )
    scripts = require_mapping(package.get("scripts"), "frontend scripts")
    collector.add(
        "frontend_sidecar_prepare_script",
        "prepare-tauri-sidecars.ps1" in str(scripts.get("sidecars:prepare", "")),
    )
    prepare_text = prepare_path.read_text(encoding="utf-8-sig")
    collector.add(
        "local_prepare_builds_both_helpers",
        "--package llama-helper --package moss-helper" in prepare_text,
    )
    collector.add(
        "local_prepare_copies_both_helpers",
        'foreach ($name in @("llama-helper", "moss-helper"))' in prepare_text,
    )
    collector.add(
        "local_prepare_prepares_fixed_webview2_runtime",
        "prepare-webview2-fixed.ps1" in prepare_text,
    )
    collector.add(
        "fixed_webview2_prepare_applies_locked_exclusions",
        "excluded_paths" in fixed_runtime_prepare.read_text(encoding="utf-8-sig"),
    )

    workflow_records: list[dict[str, Any]] = []
    for relative in WORKFLOW_PATHS:
        workflow = repo / relative
        exists = workflow.is_file()
        collector.add(f"workflow_exists_{workflow.name}", exists)
        if not exists:
            continue
        text = workflow.read_text(encoding="utf-8-sig")
        collector.add(
            f"workflow_builds_moss_{workflow.name}",
            "cargo build" in text and "moss-helper" in text,
        )
        collector.add(
            f"workflow_stages_moss_{workflow.name}",
            re.search(r"(?:Copy-Item|cp).*moss-helper", text, re.IGNORECASE) is not None,
        )
        collector.add(
            f"workflow_builds_llama_{workflow.name}",
            "cargo build" in text and "llama-helper" in text,
        )
        collector.add(
            f"workflow_uses_locked_cargo_{workflow.name}",
            re.search(r"cargo\s+build[^\r\n]*--locked", text, re.IGNORECASE) is not None,
        )
        workflow_records.append(file_record(workflow, relative_path=relative))
    release_workflow = (repo / ".github/workflows/build.yml").read_text(encoding="utf-8-sig")
    collector.add("release_workflow_selects_windows_vulkan", 'FEATURES="--features vulkan"' in release_workflow)
    collector.add(
        "release_workflow_prepares_fixed_webview2_runtime",
        "prepare-webview2-fixed.ps1" in release_workflow,
    )
    release_driver = repo / RELEASE_DRIVER_PATH
    collector.add("release_driver_exists", release_driver.is_file())
    if release_driver.is_file():
        release_driver_text = release_driver.read_text(encoding="utf-8-sig")
        collector.add(
            "release_driver_uses_audited_build_workflow",
            "uses: ./.github/workflows/build.yml" in release_driver_text,
        )
        collector.add(
            "release_driver_requires_binary_signing",
            re.search(r"^\s*sign-binaries:\s*true\s*$", release_driver_text, re.MULTILINE)
            is not None,
        )
        workflow_records.append(file_record(release_driver, relative_path=RELEASE_DRIVER_PATH))

    status = checks_status(collector.checks)
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_BUILD_WIRING",
        "generated_at": utc_now(),
        "source_commit": git_head(repo),
        "status": status,
        "files": [
            file_record(config_path, relative_path=config_path.relative_to(repo).as_posix()),
            file_record(package_path, relative_path=package_path.relative_to(repo).as_posix()),
            file_record(prepare_path, relative_path=prepare_path.relative_to(repo).as_posix()),
            file_record(
                fixed_runtime_lock,
                relative_path=fixed_runtime_lock.relative_to(repo).as_posix(),
            ),
            file_record(
                fixed_runtime_prepare,
                relative_path=fixed_runtime_prepare.relative_to(repo).as_posix(),
            ),
            *workflow_records,
        ],
        "checks": collector.checks,
        "failures": collector.failures,
    }
    atomic_write_json(output, report)
    print(json.dumps({"status": status, "output": str(output)}, ensure_ascii=False))
    return status_exit_code(status)


def validate_final_audit_for_pass(path: Path, repo: Path) -> dict[str, Any]:
    report = require_mapping(read_json(path, "P6 final audit"), "P6 final audit")
    if report.get("schema_version") != 1 or report.get("stage") != "MOSS_V3_P6_FINAL_AUDIT":
        raise GateError("P6 PASS requires a schema-valid final audit")
    if report.get("source_commit") != git_head(repo):
        raise GateError("P6 final audit source_commit does not match release HEAD")
    if report.get("p6_status") != PASS or report.get("l2_release_status") != PASS:
        raise GateError("P6 PASS requires both final P6 and L2 release status to be PASS")
    if report.get("worktree_clean") is not True or report.get("p3_p5_merged_and_passed") is not True:
        raise GateError("P6 PASS requires a clean tree and merged PASS P3-P5 prerequisites")
    if report.get("release_dependency_chain_valid") is not True:
        raise GateError("P6 PASS requires the P2 -> P3 -> P4 -> P5 -> P6 dependency chain")
    if report.get("open_gates") != []:
        raise GateError("P6 PASS final audit must have no open gates")
    stages = require_mapping(report.get("stages"), "P6 final audit stages")
    for stage in REQUIRED_STAGES:
        stage_report = require_mapping(stages.get(stage), f"P6 final audit {stage}")
        if stage_report.get("status") != PASS:
            raise GateError(f"P6 PASS requires {stage}=PASS")
    reports = require_mapping(report.get("reports"), "P6 final audit reports")
    for name in (
        "release_manifest",
        "asset_integrity",
        "release_asset_binding",
        "build_wiring",
        "lifecycle",
        "acceptance",
        "metrics",
    ):
        item = require_mapping(reports.get(name), f"P6 final audit report {name}")
        if item.get("status") != PASS:
            raise GateError(f"P6 PASS requires report {name}=PASS")
    return report


def command_evidence_manifest(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    evidence = args.evidence_dir.resolve()
    output = (args.output or (evidence / "07-evidence-manifest.json")).resolve()
    try:
        output.relative_to(evidence)
    except ValueError as exc:
        raise GateError("07 evidence manifest must be written inside the evidence directory") from exc
    status = args.p6_status
    final_audit_record: dict[str, Any] | None = None
    if status == PASS:
        if args.final_audit is None:
            raise GateError("--final-audit is mandatory when --p6-status PASS is requested")
        final_audit = args.final_audit.resolve()
        try:
            final_relative = final_audit.relative_to(evidence).as_posix()
        except ValueError as exc:
            raise GateError("P6 PASS final audit must be inside the evidence directory") from exc
        validate_final_audit_for_pass(final_audit, repo)
        final_audit_record = file_record(final_audit, relative_path=final_relative)

    missing = [name for name in REQUIRED_EVIDENCE_FILES if not (evidence / name).is_file()]
    files: list[dict[str, Any]] = []
    if evidence.is_dir():
        for record in tree_records(evidence):
            if record["path"] != output.relative_to(evidence).as_posix():
                files.append(record)
    manifest_status = PASS if not missing else FAIL
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_EVIDENCE_MANIFEST",
        "generated_at": utc_now(),
        "manifest_status": manifest_status,
        "p6_status": status,
        "required_files": list(REQUIRED_EVIDENCE_FILES),
        "missing_files": missing,
        "files": files,
        "files_manifest_sha256": records_sha256(files),
        "final_audit": final_audit_record,
    }
    atomic_write_json(output, report)
    print(
        json.dumps(
            {"manifest_status": manifest_status, "p6_status": status, "output": str(output)},
            ensure_ascii=False,
        )
    )
    return status_exit_code(manifest_status)


def validate_stage_gate(path: Path, expected_stage: str, repo: Path, head: str) -> dict[str, Any]:
    document = require_mapping(read_json(path, f"{expected_stage} stage gate"), f"{expected_stage} stage gate")
    if document.get("schema_version") != 1:
        raise GateError(f"{expected_stage} gate schema_version must be 1")
    if document.get("stage") != f"MOSS_V3_{expected_stage}":
        raise GateError(f"{expected_stage} gate stage identifier does not match")
    status = require_status(document.get("status"), f"{expected_stage} status")
    commit = require_git_commit(document.get("source_commit"), f"{expected_stage} source_commit")
    if not git_is_ancestor(repo, commit, head):
        raise GateError(f"{expected_stage} source commit is not merged into release HEAD")
    raw_files = require_list(document.get("files"), f"{expected_stage} evidence files")
    if not raw_files:
        raise GateError(f"{expected_stage} stage gate contains no evidence files")
    verified_files: list[dict[str, Any]] = []
    for index, raw_record in enumerate(raw_files):
        record = require_mapping(raw_record, f"{expected_stage} evidence file {index}")
        relative = normalize_relative_path(record.get("path"), f"{expected_stage} evidence path")
        evidence_file, _ = resolve_under(path.parent, relative, f"{expected_stage} evidence path")
        actual = file_record(evidence_file, relative_path=relative)
        expected_bytes = record.get("bytes")
        expected_hash = require_sha256(record.get("sha256"), f"{expected_stage} evidence sha256")
        if actual["bytes"] != expected_bytes or actual["sha256"] != expected_hash:
            raise GateError(f"{expected_stage} evidence file changed: {relative}")
        verified_files.append(actual)
    return {
        "stage": expected_stage,
        "status": status,
        "source_commit": commit,
        "gate_sha256": sha256_file(path),
        "evidence_file_count": len(verified_files),
        "evidence_manifest_sha256": records_sha256(verified_files),
    }


def finite_number(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise GateError(f"{label} must be a number")
    number = float(value)
    if number != number or number in {float("inf"), float("-inf")}:
        raise GateError(f"{label} must be finite")
    return number


def bounded_number(
    value: Any, label: str, *, minimum: float | None = None, maximum: float | None = None
) -> float:
    number = finite_number(value, label)
    if minimum is not None and number < minimum:
        raise GateError(f"{label} must be at least {minimum}")
    if maximum is not None and number > maximum:
        raise GateError(f"{label} must be at most {maximum}")
    return number


def nonnegative_integer(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise GateError(f"{label} must be a non-negative integer")
    return value


def find_locked_sample(lock: dict[str, Any], name: str) -> dict[str, Any]:
    for raw_sample in require_list(lock.get("samples"), "P1 locked samples"):
        sample = require_mapping(raw_sample, "P1 locked sample")
        if sample.get("name") == name:
            return sample
    raise GateError(f"P1 lock does not contain sample {name}")


def evaluate_metrics(path: Path, p1_lock_path: Path, expected_commit: str) -> dict[str, Any]:
    document = require_mapping(read_json(path, "P6 metrics"), "P6 metrics")
    if document.get("template_only") is True:
        raise GateError("metrics template is not measured evidence")
    if document.get("schema_version") != 1 or document.get("stage") != "MOSS_V3_P6_METRICS":
        raise GateError("P6 metrics schema or stage identifier is invalid")
    source_commit = require_git_commit(document.get("source_commit"), "metrics source_commit")
    if source_commit != expected_commit:
        raise GateError("metrics source_commit does not match release HEAD")
    lock = require_mapping(read_json(p1_lock_path, "P1 lock"), "P1 lock")
    business_lock = find_locked_sample(lock, "business_737s")
    long_lock = find_locked_sample(lock, "long_3096s")
    audio = require_mapping(document.get("audio"), "metrics audio")
    business_audio = require_mapping(audio.get("business_737s"), "business audio")
    long_audio = require_mapping(audio.get("long_3096s"), "long audio")
    metrics = require_mapping(document.get("metrics"), "P6 metrics values")
    truth = require_mapping(document.get("ground_truth"), "P6 ground truth")

    raw_metric_evidence = require_list(
        document.get("measurement_evidence"), "P6 measurement evidence"
    )
    metric_evidence_records: list[dict[str, Any]] = []
    metric_evidence_roles: set[str] = set()
    metric_evidence_paths: set[str] = set()
    for index, raw_record in enumerate(raw_metric_evidence):
        record = require_mapping(raw_record, f"measurement evidence {index}")
        role = require_string(record.get("role"), f"measurement evidence {index} role")
        relative = normalize_relative_path(
            record.get("path"), f"measurement evidence {role} path"
        )
        if role in metric_evidence_roles or relative.casefold() in metric_evidence_paths:
            raise GateError("measurement evidence roles and paths must be unique")
        metric_evidence_roles.add(role)
        metric_evidence_paths.add(relative.casefold())
        evidence_path, _ = resolve_under(path.parent, relative, f"measurement evidence {role}")
        actual = file_record(evidence_path, relative_path=relative)
        expected_bytes = record.get("bytes")
        if (
            isinstance(expected_bytes, bool)
            or not isinstance(expected_bytes, int)
            or expected_bytes < 0
            or actual["bytes"] != expected_bytes
            or actual["sha256"]
            != require_sha256(record.get("sha256"), f"measurement evidence {role} sha256")
        ):
            raise GateError(f"measurement evidence changed: {role}")
        metric_evidence_records.append({**actual, "role": role})
    if metric_evidence_roles != REQUIRED_METRIC_EVIDENCE_ROLES:
        raise GateError(
            "measurement evidence role set mismatch: "
            f"missing={sorted(REQUIRED_METRIC_EVIDENCE_ROLES - metric_evidence_roles)}, "
            f"unexpected={sorted(metric_evidence_roles - REQUIRED_METRIC_EVIDENCE_ROLES)}"
        )

    truth_approved = truth.get("status") == "HUMAN_APPROVED_FULL_REFERENCE"
    truth_file_valid = False
    approval_valid = False
    if truth_approved:
        relative = normalize_relative_path(truth.get("path"), "ground truth path")
        truth_path, _ = resolve_under(path.parent, relative, "ground truth path")
        actual_truth = file_record(truth_path, relative_path=relative)
        truth_file_valid = (
            actual_truth["bytes"] == truth.get("bytes")
            and actual_truth["sha256"] == require_sha256(truth.get("sha256"), "ground truth sha256")
        )
        approval_relative = normalize_relative_path(
            truth.get("approval_path"), "ground truth approval path"
        )
        approval_path, _ = resolve_under(
            path.parent, approval_relative, "ground truth approval path"
        )
        actual_approval = file_record(approval_path, relative_path=approval_relative)
        approval = require_mapping(
            read_json(approval_path, "human reference approval"), "human reference approval"
        )
        approval_valid = (
            actual_approval["bytes"] == truth.get("approval_bytes")
            and actual_approval["sha256"]
            == require_sha256(truth.get("approval_sha256"), "ground truth approval sha256")
            and approval.get("schema_version") == 1
            and approval.get("template_only") is not True
            and approval.get("stage") == "MOSS_V3_P6_HUMAN_REFERENCE_APPROVAL"
            and approval.get("status") == "APPROVED"
            and approval.get("reference_bytes") == actual_truth["bytes"]
            and str(approval.get("reference_sha256", "")).upper() == actual_truth["sha256"]
            and str(approval.get("business_audio_sha256", "")).upper()
            == str(business_lock.get("sha256", "")).upper()
            and abs(
                finite_number(
                    approval.get("reviewed_duration_seconds"), "approved reviewed duration"
                )
                - finite_number(
                    business_lock.get("source_timeline_duration_seconds"),
                    "locked business duration",
                )
            )
            <= 0.1
            and bool(require_string(approval.get("reviewer"), "human approval reviewer"))
            and bool(
                re.fullmatch(
                    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z",
                    require_string(approval.get("approved_at"), "human approval timestamp"),
                )
            )
            and approval.get("attestation") == HUMAN_APPROVAL_ATTESTATION
        )

    checks: dict[str, bool] = {
        "human_full_reference_approved": truth_approved and truth_file_valid and approval_valid,
        "business_audio_hash": str(business_audio.get("sha256", "")).upper()
        == str(business_lock.get("sha256", "")).upper(),
        "business_audio_duration": abs(
            finite_number(business_audio.get("duration_seconds"), "business duration")
            - finite_number(business_lock.get("source_timeline_duration_seconds"), "locked business duration")
        )
        <= 0.1,
        "long_audio_hash": str(long_audio.get("sha256", "")).upper()
        == str(long_lock.get("sha256", "")).upper(),
        "long_audio_duration": abs(
            finite_number(long_audio.get("duration_seconds"), "long duration")
            - finite_number(long_lock.get("source_timeline_duration_seconds"), "locked long duration")
        )
        <= 0.1,
    }
    raw_cer = bounded_number(metrics.get("moss_raw_cer"), "moss_raw_cer", minimum=0.0)
    whisper_cer = bounded_number(
        metrics.get("whisper_same_window_cer"), "whisper_same_window_cer", minimum=0.0
    )
    corrected_cer = bounded_number(
        metrics.get("corrected_cer"), "corrected_cer", minimum=0.0
    )
    spoken_term_accuracy = bounded_number(
        metrics.get("spoken_term_accuracy"),
        "spoken_term_accuracy",
        minimum=0.0,
        maximum=1.0,
    )
    unspoken_term_insertions = nonnegative_integer(
        metrics.get("unspoken_term_insertions"), "unspoken_term_insertions"
    )
    raw_segment_error = bounded_number(
        metrics.get("raw_speaker_segment_error_rate"),
        "raw_speaker_segment_error_rate",
        minimum=0.0,
        maximum=1.0,
    )
    raw_duration_error = bounded_number(
        metrics.get("raw_speaker_duration_error_rate"),
        "raw_speaker_duration_error_rate",
        minimum=0.0,
        maximum=1.0,
    )
    corrected_speaker_errors = nonnegative_integer(
        metrics.get("corrected_speaker_error_count"), "corrected_speaker_error_count"
    )
    timestamp_parse_rate = bounded_number(
        metrics.get("timestamp_parse_rate"),
        "timestamp_parse_rate",
        minimum=0.0,
        maximum=1.0,
    )
    business_rtf = bounded_number(
        metrics.get("business_rtf"), "business_rtf", minimum=0.0
    )
    long_last_timestamp = bounded_number(
        metrics.get("long_audio_last_timestamp_seconds"),
        "long_audio_last_timestamp_seconds",
        minimum=0.0,
    )
    residual_processes = nonnegative_integer(
        metrics.get("moss_residual_process_count"), "moss_residual_process_count"
    )
    checks.update(
        {
            "moss_raw_cer_at_most_20_percent": raw_cer <= 0.20,
            "moss_not_worse_than_whisper": raw_cer <= whisper_cer,
            "corrected_cer_at_most_15_percent": corrected_cer <= 0.15,
            "spoken_term_accuracy_at_least_95_percent": spoken_term_accuracy >= 0.95,
            "unspoken_term_insertions_zero": unspoken_term_insertions == 0,
            "raw_speaker_segment_error_recorded": True,
            "raw_speaker_duration_error_recorded": True,
            "marketing_claim_safe": (
                max(raw_segment_error, raw_duration_error) <= 0.20
                or metrics.get("claims_automatic_speaker_accuracy") is False
            ),
            "corrected_speaker_errors_zero": corrected_speaker_errors == 0,
            "timestamp_parse_rate_100_percent": timestamp_parse_rate == 1.0,
            "timestamps_monotonic": metrics.get("timestamps_monotonic") is True,
            "timestamps_in_bounds": metrics.get("timestamps_in_bounds") is True,
            "business_rtf_at_most_one": business_rtf <= 1.0,
            "long_audio_complete": metrics.get("long_audio_complete") is True,
            "long_audio_tail_reached": abs(
                long_last_timestamp
                - finite_number(long_lock.get("source_timeline_duration_seconds"), "locked long duration")
            )
            <= 0.5,
            "moss_process_released": residual_processes == 0,
            "qwen_started_after_moss_exit": metrics.get("qwen_started_after_moss_exit") is True,
        }
    )
    if not checks["human_full_reference_approved"]:
        status = BLOCKED
    elif all(checks.values()):
        status = PASS
    else:
        status = FAIL
    return {
        "status": status,
        "source_commit": source_commit,
        "metrics_sha256": sha256_file(path),
        "measurement_evidence_count": len(metric_evidence_records),
        "measurement_evidence_manifest_sha256": sha256_bytes(
            canonical_json_bytes(
                sorted(metric_evidence_records, key=lambda item: str(item["role"]))
            )
        ),
        "checks": checks,
        "failed_checks": sorted(name for name, passed in checks.items() if not passed),
    }


def validate_pass_report_semantics(document: dict[str, Any], label: str, stage: str) -> None:
    """Reject a hand-written superficial PASS that omits the tool's hard checks."""

    if document.get("template_only") is True:
        raise GateError(f"{label} is still marked as a template")

    if stage in {
        "MOSS_V3_P6_RELEASE_MANIFEST",
        "MOSS_V3_P6_ASSET_INTEGRITY",
        "MOSS_V3_P6_BUILD_WIRING",
    }:
        checks = require_mapping(document.get("checks"), f"{label} checks")
        if not checks or any(value is not True for value in checks.values()):
            raise GateError(f"{label} PASS requires a non-empty all-true checks map")
        failures = require_list(document.get("failures"), f"{label} failures")
        if failures:
            raise GateError(f"{label} PASS cannot contain failures")
        return

    if stage == "MOSS_V3_P6_LIFECYCLE":
        checks = require_mapping(document.get("checks"), "lifecycle checks")
        if not checks or any(value is not True for value in checks.values()):
            raise GateError("lifecycle PASS requires a non-empty all-true checks map")
        if require_list(document.get("coverage_failures"), "lifecycle coverage failures"):
            raise GateError("lifecycle PASS contains coverage failures")
        snapshots = require_mapping(document.get("snapshots"), "lifecycle snapshots")
        if set(snapshots) != set(REQUIRED_LIFECYCLE_CHECKPOINTS):
            raise GateError("lifecycle PASS requires all seven named checkpoints")
        for checkpoint, raw_snapshot in snapshots.items():
            snapshot = require_mapping(raw_snapshot, f"lifecycle snapshot {checkpoint}")
            if snapshot.get("status") != PASS:
                raise GateError(f"lifecycle snapshot {checkpoint} is not PASS")
            require_sha256(snapshot.get("sha256"), f"lifecycle snapshot {checkpoint} sha256")
        transitions = [
            require_mapping(item, "lifecycle transition")
            for item in require_list(document.get("transitions"), "lifecycle transitions")
        ]
        ids = [str(item.get("id")) for item in transitions]
        if set(ids) != set(REQUIRED_LIFECYCLE_TRANSITIONS) or len(ids) != len(
            REQUIRED_LIFECYCLE_TRANSITIONS
        ):
            raise GateError("lifecycle PASS requires all four transitions exactly once")
        for transition in transitions:
            transition_id = str(transition.get("id"))
            assertions = [
                require_mapping(item, f"lifecycle {transition_id} assertion")
                for item in require_list(
                    transition.get("assertions"), f"lifecycle {transition_id} assertions"
                )
            ]
            if transition.get("status") != PASS or not assertions or any(
                item.get("status") != PASS for item in assertions
            ):
                raise GateError(f"lifecycle transition {transition_id} is not a complete PASS")
        return

    if stage == "MOSS_V3_P6_ACCEPTANCE":
        if require_list(document.get("blockers"), "acceptance blockers"):
            raise GateError("acceptance PASS cannot contain blockers")
        if document.get("release_dependency_chain_valid") is not True or require_list(
            document.get("dependency_chain_errors", []), "acceptance dependency chain errors"
        ):
            raise GateError("acceptance PASS requires the P2 -> P3 -> P4 -> P5 -> P6 chain")
        required = require_list(document.get("required_scenarios"), "required scenarios")
        if set(required) != set(REQUIRED_P6_SCENARIOS) or len(required) != len(
            REQUIRED_P6_SCENARIOS
        ):
            raise GateError("acceptance PASS required scenario contract is incomplete")
        stage_gates = require_mapping(document.get("stage_gates"), "acceptance stage gates")
        if set(stage_gates) != set(REQUIRED_STAGES) or any(
            not isinstance(stage_gates.get(stage), dict)
            or stage_gates[stage].get("status") != PASS
            for stage in REQUIRED_STAGES
        ):
            raise GateError("acceptance PASS requires P0-P5 stage gates to be PASS")
        scenarios = [
            require_mapping(item, "acceptance scenario")
            for item in require_list(document.get("scenarios"), "acceptance scenarios")
        ]
        ids = [str(item.get("id")) for item in scenarios]
        if set(ids) != set(REQUIRED_P6_SCENARIOS) or len(ids) != len(REQUIRED_P6_SCENARIOS):
            raise GateError("acceptance PASS requires all 28 scenarios exactly once")
        for scenario in scenarios:
            scenario_id = str(scenario.get("id"))
            commands = [
                require_mapping(item, f"acceptance {scenario_id} command")
                for item in require_list(
                    scenario.get("commands"), f"acceptance {scenario_id} commands"
                )
            ]
            evidence_files = require_list(
                scenario.get("evidence_files"), f"acceptance {scenario_id} evidence files"
            )
            missing = require_list(
                scenario.get("missing_evidence_files"),
                f"acceptance {scenario_id} missing evidence files",
            )
            validation_errors = require_list(
                scenario.get("evidence_validation_errors", []),
                f"acceptance {scenario_id} evidence validation errors",
            )
            if (
                scenario.get("status") != PASS
                or not commands
                or any(item.get("status") != PASS for item in commands)
                or not evidence_files
                or missing
                or validation_errors
            ):
                raise GateError(f"acceptance scenario {scenario_id} is not a complete PASS")
        return

    raise GateError(f"No PASS semantic validator exists for {stage}")


def load_bound_report(
    path: Path | None,
    *,
    label: str,
    expected_stage: str,
    expected_commit: str,
) -> dict[str, Any]:
    if path is None:
        return {"status": NOT_RUN, "reason": f"{label} was not supplied"}
    document = require_mapping(read_json(path, label), label)
    if document.get("schema_version") != 1 or document.get("stage") != expected_stage:
        raise GateError(f"{label} schema/stage identifier is invalid")
    status = require_status(document.get("status"), f"{label} status")
    commit = require_git_commit(document.get("source_commit"), f"{label} source_commit")
    if commit != expected_commit:
        raise GateError(f"{label} source_commit does not match release HEAD")
    if status == PASS:
        validate_pass_report_semantics(document, label, expected_stage)
    return {"status": status, "source_commit": commit, "sha256": sha256_file(path)}


def evaluate_release_asset_binding(
    release_path: Path, asset_path: Path, expected_commit: str
) -> dict[str, Any]:
    release = require_mapping(read_json(release_path, "release manifest"), "release manifest")
    asset = require_mapping(read_json(asset_path, "asset integrity report"), "asset integrity report")
    if (
        release.get("stage") != "MOSS_V3_P6_RELEASE_MANIFEST"
        or release.get("status") != PASS
        or release.get("source_commit") != expected_commit
    ):
        raise GateError("release manifest is not a PASS report bound to release HEAD")
    if (
        asset.get("stage") != "MOSS_V3_P6_ASSET_INTEGRITY"
        or asset.get("status") != PASS
        or asset.get("source_commit") != expected_commit
    ):
        raise GateError("asset integrity is not a PASS report bound to release HEAD")

    deployments = require_mapping(release.get("deployment_roots"), "release deployment roots")
    moss_root_ids = {
        str(root_id)
        for root_id, raw_root in deployments.items()
        if isinstance(raw_root, dict) and raw_root.get("classification") == "moss_data"
    }
    all_records = [
        require_mapping(item, "release file")
        for item in require_list(release.get("files"), "release files")
    ]
    records = [item for item in all_records if str(item.get("root")) in moss_root_ids]
    inventory_hashes = {
        require_sha256(item.get("sha256"), "release MOSS file sha256") for item in records
    }
    by_role: dict[str, list[dict[str, Any]]] = {}
    for item in records:
        by_role.setdefault(str(item.get("role")), []).append(item)

    model = require_mapping(asset.get("model"), "asset model")
    model_license = require_mapping(asset.get("model_license"), "asset model license")
    runtime = require_mapping(asset.get("runtime"), "asset runtime")
    runtime_license = require_mapping(asset.get("runtime_license"), "asset runtime license")
    third_party = require_mapping(asset.get("third_party_licenses"), "asset third-party licenses")
    runtime_files = [
        require_mapping(item, "asset runtime file")
        for item in require_list(runtime.get("files"), "asset runtime files")
    ]
    bundled_licenses = [
        require_mapping(item, "asset bundled license")
        for item in require_list(runtime.get("bundled_licenses"), "asset bundled licenses")
    ]

    def role_has_hash(role: str, expected: Any) -> bool:
        expected_hash = require_sha256(expected, f"{role} expected sha256")
        return any(
            require_sha256(item.get("sha256"), f"{role} release sha256") == expected_hash
            for item in by_role.get(role, [])
        )

    missing_runtime_files = []
    for item in runtime_files:
        expected_name = PureWindowsPath(require_string(item.get("path"), "runtime file path")).name
        expected_hash = require_sha256(item.get("sha256"), f"runtime {expected_name} sha256")
        if not any(
            PureWindowsPath(require_string(record.get("path"), "release MOSS path")).name
            == expected_name
            and require_sha256(record.get("sha256"), "release MOSS sha256") == expected_hash
            for record in records
        ):
            missing_runtime_files.append(expected_name)

    required_hashes = {
        require_sha256(model.get("sha256"), "asset model sha256"),
        require_sha256(model_license.get("sha256"), "asset model license sha256"),
        require_sha256(runtime_license.get("sha256"), "asset runtime license sha256"),
        require_sha256(third_party.get("sha256"), "asset third-party sha256"),
        *[
            require_sha256(item.get("sha256"), "asset runtime component sha256")
            for item in runtime_files
        ],
        *[
            require_sha256(item.get("sha256"), "asset bundled license sha256")
            for item in bundled_licenses
        ],
    }
    checks = {
        "one_moss_data_root": len(moss_root_ids) == 1,
        "model_role_matches_frozen_asset": role_has_hash("moss_model", model.get("sha256")),
        "model_license_role_matches_frozen_asset": role_has_hash(
            "model_license", model_license.get("sha256")
        ),
        "runtime_license_role_matches_frozen_asset": role_has_hash(
            "runtime_license", runtime_license.get("sha256")
        ),
        "third_party_role_matches_frozen_asset": role_has_hash(
            "third_party_licenses", third_party.get("sha256")
        ),
        "every_locked_runtime_file_in_release": not missing_runtime_files,
        "every_frozen_asset_hash_in_moss_inventory": required_hashes.issubset(inventory_hashes),
    }
    status = checks_status(checks)
    return {
        "status": status,
        "source_commit": expected_commit,
        "release_manifest_sha256": sha256_file(release_path),
        "asset_report_sha256": sha256_file(asset_path),
        "checks": checks,
        "missing_runtime_files": sorted(missing_runtime_files),
        "missing_asset_hashes": sorted(required_hashes - inventory_hashes),
    }


def parse_stage_gate_arguments(values: list[str]) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for value in values:
        if "=" not in value:
            raise GateError("--stage-gate must use STAGE=PATH")
        stage, raw_path = value.split("=", 1)
        stage = stage.upper()
        if stage not in REQUIRED_STAGES:
            raise GateError(f"unsupported stage gate {stage!r}")
        if stage in result:
            raise GateError(f"duplicate stage gate {stage}")
        result[stage] = Path(raw_path).resolve()
    return result


def command_final_audit(args: argparse.Namespace) -> int:
    repo = args.repo.resolve()
    output = args.output.resolve()
    head = git_head(repo)
    stage_paths = parse_stage_gate_arguments(args.stage_gate)
    stages: dict[str, dict[str, Any]] = {}
    stage_errors: list[dict[str, str]] = []
    for stage in REQUIRED_STAGES:
        path = stage_paths.get(stage)
        if path is None:
            stages[stage] = {"stage": stage, "status": NOT_RUN, "reason": "gate not supplied"}
            continue
        try:
            stages[stage] = validate_stage_gate(path, stage, repo, head)
        except GateError as exc:
            stages[stage] = {"stage": stage, "status": FAIL, "reason": str(exc)}
            stage_errors.append({"stage": stage, "error": str(exc)})

    all_prerequisites_pass = all(stages[stage]["status"] == PASS for stage in REQUIRED_STAGES)
    p3_p5_merged_and_passed = all(stages[stage]["status"] == PASS for stage in PARALLEL_STAGE_BLOCKERS)
    release_dependency_chain_valid, dependency_chain_errors = validate_release_dependency_chain(
        stages, repo, head
    )
    reports: dict[str, dict[str, Any]] = {}
    report_errors: list[dict[str, str]] = []
    report_specs = (
        ("release_manifest", args.release_manifest, "MOSS_V3_P6_RELEASE_MANIFEST"),
        ("asset_integrity", args.asset_report, "MOSS_V3_P6_ASSET_INTEGRITY"),
        ("build_wiring", args.build_wiring_report, "MOSS_V3_P6_BUILD_WIRING"),
        ("lifecycle", args.lifecycle_report, "MOSS_V3_P6_LIFECYCLE"),
        ("acceptance", args.acceptance_report, "MOSS_V3_P6_ACCEPTANCE"),
    )
    for label, path, expected_stage in report_specs:
        try:
            reports[label] = load_bound_report(
                path.resolve() if path else None,
                label=label,
                expected_stage=expected_stage,
                expected_commit=head,
            )
        except GateError as exc:
            reports[label] = {"status": FAIL, "reason": str(exc)}
            report_errors.append({"report": label, "error": str(exc)})

    if args.release_manifest is None or args.asset_report is None:
        reports["release_asset_binding"] = {
            "status": NOT_RUN,
            "reason": "release manifest and asset report were not both supplied",
        }
    else:
        try:
            reports["release_asset_binding"] = evaluate_release_asset_binding(
                args.release_manifest.resolve(), args.asset_report.resolve(), head
            )
        except GateError as exc:
            reports["release_asset_binding"] = {"status": FAIL, "reason": str(exc)}
            report_errors.append({"report": "release_asset_binding", "error": str(exc)})

    if args.metrics_report is None:
        reports["metrics"] = {"status": NOT_RUN, "reason": "metrics report was not supplied"}
    else:
        try:
            reports["metrics"] = evaluate_metrics(
                args.metrics_report.resolve(), repo / P1_LOCK_RELATIVE, head
            )
        except GateError as exc:
            reports["metrics"] = {"status": FAIL, "reason": str(exc)}
            report_errors.append({"report": "metrics", "error": str(exc)})

    any_failure = any(item["status"] == FAIL for item in stages.values()) or any(
        item["status"] == FAIL for item in reports.values()
    )
    all_reports_pass = all(item["status"] == PASS for item in reports.values())
    if not all_prerequisites_pass or not release_dependency_chain_valid:
        p6_status = NOT_RUN
    elif any_failure:
        p6_status = FAIL
    elif all_reports_pass:
        p6_status = PASS
    else:
        p6_status = NOT_RUN

    if p6_status == PASS and all_prerequisites_pass:
        release_status = PASS
    elif any_failure:
        release_status = FAIL
    else:
        release_status = BLOCKED

    open_gates: list[str] = []
    for stage, result in stages.items():
        if result["status"] != PASS:
            open_gates.append(f"{stage}_STATUS_{result['status'].replace(' ', '_')}")
    for label, result in reports.items():
        if result["status"] != PASS:
            open_gates.append(f"{label.upper()}_STATUS_{result['status'].replace(' ', '_')}")
    if not p3_p5_merged_and_passed:
        open_gates.append("P3_P4_P5_NOT_ALL_MERGED_AND_PASS")
    open_gates.extend(dependency_chain_errors)
    tracked_dirty = git_output(repo, ["status", "--porcelain=v1", "--untracked-files=all"])
    if tracked_dirty:
        open_gates.append("RELEASE_WORKTREE_NOT_CLEAN")
        if p6_status == PASS:
            p6_status = FAIL
            release_status = FAIL

    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_FINAL_AUDIT",
        "generated_at": utc_now(),
        "source_commit": head,
        "p6_status": p6_status,
        "l2_release_status": release_status,
        "p3_p5_merged_and_passed": p3_p5_merged_and_passed,
        "release_dependency_chain": list(RELEASE_DEPENDENCY_CHAIN) + ["P6_HEAD"],
        "release_dependency_chain_valid": release_dependency_chain_valid,
        "dependency_chain_errors": dependency_chain_errors,
        "tracked_tree_clean": not bool(tracked_dirty),
        "worktree_clean": not bool(tracked_dirty),
        "stages": stages,
        "reports": reports,
        "open_gates": sorted(set(open_gates)),
        "stage_errors": stage_errors,
        "report_errors": report_errors,
        "truth_rule": (
            "Accuracy is PASS only when a human-approved full reference file is present, hashed, "
            "and bound to the frozen 737.728-second audio."
        ),
    }
    atomic_write_json(output, report)
    print(
        json.dumps(
            {
                "p6_status": p6_status,
                "l2_release_status": release_status,
                "open_gate_count": len(report["open_gates"]),
                "output": str(output),
            },
            ensure_ascii=False,
        )
    )
    return status_exit_code(release_status)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    assets = subparsers.add_parser("assets", help="verify frozen MOSS model/runtime/licenses")
    assets.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    assets.add_argument("--lock", type=Path, default=Path(__file__).with_name("moss_v3_p1_lock.json"))
    assets.add_argument("--data-root", type=Path, required=True)
    assets.add_argument("--deployment-root", default=r"D:\MeetilyData")
    assets.add_argument("--system-drive", default=os.environ.get("SystemDrive", "C:"))
    assets.add_argument("--model", type=Path, required=True)
    assets.add_argument("--model-license", type=Path, required=True)
    assets.add_argument("--runtime", type=Path, required=True)
    assets.add_argument("--runtime-license", type=Path, required=True)
    assets.add_argument("--runtime-third-party-license", type=Path, required=True)
    assets.add_argument("--output", type=Path, required=True)
    assets.set_defaults(handler=command_assets)

    package = subparsers.add_parser("package", help="build a hash-bound release package manifest")
    package.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    package.add_argument("--spec", type=Path, required=True)
    package.add_argument("--output", type=Path, required=True)
    package.set_defaults(handler=command_package)

    build = subparsers.add_parser("build-wiring", help="audit helper wiring in build/package scripts")
    build.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    build.add_argument("--output", type=Path, required=True)
    build.set_defaults(handler=command_build_wiring)

    evidence = subparsers.add_parser("evidence-manifest", help="close the fixed P6 evidence list")
    evidence.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    evidence.add_argument("--evidence-dir", type=Path, required=True)
    evidence.add_argument("--output", type=Path)
    evidence.add_argument("--p6-status", choices=[NOT_RUN, BLOCKED, FAIL, PASS], default=NOT_RUN)
    evidence.add_argument("--final-audit", type=Path)
    evidence.set_defaults(handler=command_evidence_manifest)

    final = subparsers.add_parser("final-audit", help="apply the non-bypassable P6/L2 release gate")
    final.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    final.add_argument("--stage-gate", action="append", default=[], metavar="STAGE=PATH")
    final.add_argument("--release-manifest", type=Path)
    final.add_argument("--asset-report", type=Path)
    final.add_argument("--build-wiring-report", type=Path)
    final.add_argument("--lifecycle-report", type=Path)
    final.add_argument("--acceptance-report", type=Path)
    final.add_argument("--metrics-report", type=Path)
    final.add_argument("--output", type=Path, required=True)
    final.set_defaults(handler=command_final_audit)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    try:
        return int(args.handler(args))
    except GateError as exc:
        print(json.dumps({"status": FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
