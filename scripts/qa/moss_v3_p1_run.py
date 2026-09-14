#!/usr/bin/env python3
r"""Run one controlled MOSS v3 P1 Intel Arc/Vulkan acceptance sample.

This is a gate runner, not a product helper.  It uses the frozen
transcribe.cpp v0.2.2 ctypes binding and native DLL bundle, selects the Intel
Arc Vulkan device by exact identity, and rejects a change of primary device.
The native scheduler still contains CPU fallback support for unsupported
operators, so this runner does not claim GPU-only execution.

The runner accepts one already-frozen source sample per invocation.  AAC
containers are converted to deterministic 16 kHz mono PCM WAV files under
``D:\MeetilyData\staging\moss-p1\inputs``.  Existing converted files are
reused only when their source/conversion manifest and output hash still match;
they are never overwritten.
"""

from __future__ import annotations

import argparse
import array
import ctypes
import dataclasses
import hashlib
import importlib
import importlib.metadata
import json
import math
import os
import re
import subprocess
import sys
import threading
import time
import traceback
import wave
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


EXPECTED_API_VERSION = "0.2.2"
EXPECTED_NATIVE_COMMIT = "c6a9257"
EXPECTED_SOURCE_COMMIT = "c6a9257cdf8e9c6918c0f8f876246db048a22103"
EXPECTED_MODEL_NAME = "MOSS-Transcribe-Diarize-Q8_0.gguf"
EXPECTED_MODEL_BYTES = 986_899_616
EXPECTED_MODEL_SHA256 = "64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039"
EXPECTED_UPSTREAM_MODEL_REVISION = "cb765f2b0fe6f7a298aa2002e2281ae693d1f3c3"
EXPECTED_MODEL_LICENSE = Path(
    r"D:\MeetilyData\staging\moss-p1\LICENSE-MOSS-Transcribe-Diarize-Apache-2.0.txt"
)
EXPECTED_MODEL_LICENSE_BYTES = 11_357
EXPECTED_MODEL_LICENSE_SHA256 = "C71D239DF91726FC519C6EB72D318EC65820627232B2F796219E87DCF35D0AB4"
EXPECTED_DEVICE_KIND = "vulkan"
EXPECTED_DEVICE_DESCRIPTION = "Intel(R) Arc(TM) Graphics"
EXPECTED_DEVICE_ID = None
EXPECTED_DEVICE_TYPE = "igpu"
EXPECTED_PYTHON = Path(__file__).resolve().parents[2] / ".tools" / "moss-poc" / ".venv" / "Scripts" / "python.exe"
EXPECTED_SITE_PACKAGES = Path(r"D:\MeetilyData\staging\moss-p1\python-deps")
EXPECTED_PYTHON_SHA256 = "560B9EF7D856608AB8DA02DED2DC8A1951AD1F424C382C0EC6A698874165A18E"
EXPECTED_PYTHON_VERSION = (3, 12, 13)
EXPECTED_BASE_PREFIX = Path(
    r"C:\Users\liuxin\.cache\codex-runtimes\codex-primary-runtime\dependencies\python"
)
EXPECTED_PYVENV_CFG_SHA256 = "FB31B3399FD193478B24AD00535EF433CCB14DEA9A568D8F1B10F88414385D1C"
EXPECTED_VENV_SCRIPTS_MANIFEST_SHA256 = "EE7632E5BC2A7B54CD1ACA14EE72954D65C559F29D74E76E22B7187B92BB03A8"
EXPECTED_VENV_SCRIPTS_FILE_COUNT = 34
EXPECTED_VENV_SCRIPTS_BYTES = 3_452_085
GIT_EXECUTABLE = (
    Path(sys.base_prefix).resolve().parent / "native" / "git" / "cmd" / "git.exe"
)
EXPECTED_PSUTIL_VERSION = "7.1.3"
EXPECTED_PSUTIL_INIT_SHA256 = "DD1397ECC2A8E10C375B2093419389BB4DAA8BD12AC0B26DDE2CFA991D595C26"
EXPECTED_PSUTIL_NATIVE_SHA256 = "039E4D17D9B8399162CEB7A40177E66A7BAC46FCD0F722813670AF11EEAE5C22"
EXPECTED_PSUTIL_PACKAGE_MANIFEST_SHA256 = "8E349CBB107CBA6C599D0A233957D21F91B9319FF601C00D6CD8FCBC8689C077"
EXPECTED_PSUTIL_PACKAGE_FILE_COUNT = 10
EXPECTED_BASE_EXECUTABLE_SHA256 = "D8E3F0ADF246DB00358C0C4ED349CF714898178F9558FB0E944F79F5C07F8EAA"
EXPECTED_PYTHON_DLL_SHA256 = "64A1DAD031E97F13B1A0BAC26C689D8E14A18D7DD1EAB06E17F70E22373F4EEC"
EXPECTED_BASE_RUNTIME_MANIFEST_SHA256 = "9E4EB4AB85B578067E8AD0F21C2D6E246DCABFA66373612A352DF29D4228AA99"
EXPECTED_BASE_RUNTIME_FILE_COUNT = 2294
EXPECTED_BASE_RUNTIME_BYTES = 48_590_231
EXPECTED_HOST_FINGERPRINT_SHA256 = "F7C48478EF6F5E314715087BA94BBC2F689BA3C60CCA5BDB7E820F4A5B19C1EB"
EXPECTED_VULKAN_LOADER = Path(r"C:\Windows\System32\vulkan-1.dll")
EXPECTED_VULKAN_LOADER_SHA256 = "591D4C316A0A39F96792979CE5EBB1BB380F0015C30707BA7AC611B35AE01CFC"
EXPECTED_INTEL_VULKAN_MODULES = {
    Path(r"C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_15cb41d17b4923f1\igc-default64.dll"): "E90D7BBEE270E57A80669AC18B92ACB7B57C78DD5771B9D4E090874D8D39D837",
    Path(r"C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_15cb41d17b4923f1\igvk64.dll"): "3673A93E939F100C327E2FE80663C21C2088EB1A3DF0956575A21A4D92BFD183",
    Path(r"C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_15cb41d17b4923f1\igvkMedia64.dll"): "A31F8B14FCEE878801A269CADF399E732477D76F48FAAE539F0DDFDFE1E0B4FB",
    Path(r"C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_15cb41d17b4923f1\IntelControlLib.dll"): "71D7E0AC912287BFE270FA8D1E93B2E51655817EE141142B4723A3916D0F51B7",
    Path(r"C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_15cb41d17b4923f1\igc64.dll"): "80D327048FC5E48122E133B9BC96B007AD0EF553FCCDBCA93ADB9CD6438E3DAB",
}
EXPECTED_INTEL_VULKAN_INFERENCE_ONLY_MODULES = {
    Path(r"C:\Windows\System32\DriverStore\FileRepository\iigd_dch.inf_amd64_15cb41d17b4923f1\igc-default64.dll")
}
EXPECTED_POWERSHELL = Path(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
EXPECTED_POWERSHELL_SHA256 = "7600FFE12DA441FE89D035B13801E8E91D064BC544A27B19A5CF49F6AB8B18F5"
EXPECTED_ICACLS = Path(r"C:\Windows\System32\icacls.exe")
EXPECTED_ICACLS_SHA256 = "CB9E55D4C02F4E55100724D2DA9A1267F49E196BD39FCDF36CEC6EA6314FAD2A"
EXPECTED_GIT_SHA256 = "C53279919FDEA03474BB23B465B3A82287157491F1BD69A5EB82DD9831582333"
SILENCE_THRESHOLD_DB = -40
# Do not make the silence probe coarser than the final timestamp tolerance.
# Otherwise a real short silence is mislabeled as speech and
# produces a deterministic false failure at the end of an otherwise complete run.
SILENCE_MIN_DURATION_SECONDS = 0.2
MIN_ACTIVITY_INTERVAL_SECONDS = 0.25
OUTPUT_END_TOLERANCE_SECONDS = 0.5
OUTPUT_START_TOLERANCE_SECONDS = 2.0
SOURCE_TIMELINE_TOLERANCE_SECONDS = 0.1
MIN_ACTIVITY_COVERAGE_RATIO = 0.87
MAX_UNCOVERED_ACTIVITY_GAP_SECONDS = 1.0
MAX_SAME_SPEAKER_OVERLAP_SECONDS = 0.5
MAX_ACTIVE_SECONDS_PER_SEGMENT = 120.0
MIN_TEXT_CHARS_PER_TRANSCRIPT_SECOND = 0.5

SOURCE_ROOT = Path(r"D:\MeetilyData\staging\v0\source\transcribe.cpp-v0.2.2")
BINDING_ROOT = SOURCE_ROOT / "bindings" / "python" / "src"
RUNTIME_DIR = Path(
    r"D:\MeetilyData\staging\v0\runtime\transcribe-native-windows-x86_64-cpu-vulkan"
)
MODEL_ROOT = Path(r"D:\MeetilyData\staging\moss-p1")
INPUT_ROOT = MODEL_ROOT / "inputs"
REPO_ROOT = Path(__file__).resolve().parents[2]
FFMPEG = REPO_ROOT / "target" / "release" / "ffmpeg.exe"
RUNNER_PATH = Path(__file__).resolve()
TEST_PATH = RUNNER_PATH.with_name("test_moss_v3_p1_run.py")
LOCK_PATH = RUNNER_PATH.with_name("moss_v3_p1_lock.json")
SUPERVISOR_PATH = RUNNER_PATH.with_name("moss_v3_p1_supervise.py")
SUPERVISOR_TEST_PATH = RUNNER_PATH.with_name("test_moss_v3_p1_supervise.py")
CHUNKED_PATH = RUNNER_PATH.with_name("moss_v3_p1_chunked.py")
CHUNKED_TEST_PATH = RUNNER_PATH.with_name("test_moss_v3_p1_chunked.py")
PUBLIC_EVIDENCE_ROOT = (
    REPO_ROOT / "target" / "release" / "docs" / "方案" / "证据"
).resolve()
PRIVATE_EVIDENCE_ROOT = Path(r"D:\MeetilyData\private-evidence\moss-v3-p1").resolve()
PYTHON_CACHE_ROOT = Path(r"D:\MeetilyData\staging\moss-p1\empty-pycache").resolve()
APPROVED_TOOLCHAIN_MANIFEST = Path(
    r"D:\MeetilyData\locks\moss-v3-p1-approved-toolchain.json"
).resolve()

EXPECTED_SAMPLES = {
    "short_12s": {
        "bytes": 385272,
        "sha256": "E9376B7F00C3B553211E27E65BE397A09CEEB2ACDF28337AB951B7C7CDBBE9DD",
        "source_timeline_duration_seconds": 12.0373125,
        "prepared": {"bytes": 385272, "sha256": "E9376B7F00C3B553211E27E65BE397A09CEEB2ACDF28337AB951B7C7CDBBE9DD", "frames": 192597, "pcm_s16le_sha256": "775BA1A6148BAA18A034F929B0CD09535395CCFA2D3C01C5F44B1CE6FAE68072", "float32_sha256": "2A1997D757808E64BED66B774DB0D1F4219ECCB13E77C2D7585E712458509A02"},
        "known_audible_anchor_probes": ["核心功能验收"],
    },
    "real_107s": {
        "bytes": 909038,
        "sha256": "0361C5203587BFBF65ADC2F31DF157C8F14C44F2D42CEADF88C114A8EF1789F6",
        "source_timeline_duration_seconds": 107.42,
        "prepared": {"bytes": 3437782, "sha256": "A818172478951F9C941307FBCDE5B15990F02D7EC0F220FF8D3ACC7CCC73B4F7", "frames": 1718869, "pcm_s16le_sha256": "E6DBFFE31C48C336645343580312BC293AB0CAC8AF67F097DF5700D8CA3D9333", "float32_sha256": "A182A967EF956B9470AD99D3B3275D59E7F5A9A75CCE3A1E61F7B8855BA32D9B"},
        # The planned script contains a second paragraph, but the frozen MP4
        # is objectively silent from about 81.10 s to EOF.  Only the final
        # phrase that is actually present in the waveform is a gate anchor.
        "known_audible_anchor_probes": ["下面暂停录音十秒", "恢复后继续"],
    },
    "review_244s": {
        "bytes": 7838158,
        "sha256": "DE0B7237880441606AD614CF8508EB34CA76ABE64A875DAFD0B7E030705FB881",
        "source_timeline_duration_seconds": 244.94,
        "prepared": {"bytes": 7838158, "sha256": "DE0B7237880441606AD614CF8508EB34CA76ABE64A875DAFD0B7E030705FB881", "frames": 3919040, "pcm_s16le_sha256": "08788B9B3F713360069229530B995D462DE80E0059CA81E188D7FFE15757E769", "float32_sha256": "630E29BDB305BE903ED59E2D3E300E202C7922741760F71D33376431013F154C"},
    },
    "business_737s": {
        "bytes": 23607374,
        "sha256": "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB",
        "source_timeline_duration_seconds": 737.728,
        "prepared": {"bytes": 23607374, "sha256": "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB", "frames": 11803648, "pcm_s16le_sha256": "4E2FC2387B173CA7C6F0D42E3796CF6FB8D4BED630527AC67833AFEEBBF287C1", "float32_sha256": "673F8BC361FA9008CA4CD14A853BD49B5D7433E20124420E59000FADF20FB8C5"},
        "max_rtf": 1.0,
    },
    "monthly_900s": {
        "bytes": 18148250,
        "sha256": "C6815E3C81CA022B12EA48BB2102EFCFA625B3F760FFC796F5D0F02002A0DD71",
        "source_timeline_duration_seconds": 900.02,
        "prepared": {"bytes": 28802946, "sha256": "FAA196E7E3978942E9DBCD8E646BB8835A075B9A3717A549A60E793535C3C2CB", "frames": 14401451, "pcm_s16le_sha256": "4F6DCD0BD3C9413DA7842F719D7DDACF1CC1BA985E14D4DF9565AD6A2DDF4A58", "float32_sha256": "ECD33570A8CB2F65D526FEB96540B8C9C5CD3851CA5531DE89FE9C04F465D6EE"},
    },
    "long_3096s": {
        "bytes": 61402760,
        "sha256": "A1B8A502E5C9A3A88D7F1AC425229DEFDE085A389FE4CE78D3C665B57A373C04",
        "source_timeline_duration_seconds": 3096.62,
        "prepared": {"bytes": 99093548, "sha256": "FE6BAB06F9B64DD4A665FC0536971DF572EC5A24999C075BEE6ACB6E623B99E7", "frames": 49546752, "pcm_s16le_sha256": "8CD9A7A1C67A1520ED5CE58478AB004EB12FC979DA8FD458922AD7F37E169FCD", "float32_sha256": "F45C07E0ED4325948F23A1E0337A130E8601433434450894E3CA69C450F6AFE1"},
    },
}

EXPECTED_HASHES = {
    RUNTIME_DIR / "contract.json": "C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73",
    RUNTIME_DIR / "transcribe.dll": "3C19F92A9ACB8480377DAEBD415909887AD770E3A58DBC4BF50FDD2432071E63",
    RUNTIME_DIR / "ggml.dll": "CEA8F409E2E94E9A847BFA962499C384334A31A28EF14B3BF73CA930DB77AE1F",
    RUNTIME_DIR / "ggml-base.dll": "951BD8BC93B9F5327D81BCDA7627144265655ECFE8749D3B7BFCDE9B8AA6D781",
    RUNTIME_DIR / "ggml-vulkan.dll": "52FEA282E87471078A2BD795CD5F31E8CF345B2CCDCAC94F4133FBE9CF7A20B6",
    RUNTIME_DIR / "ggml-cpu-alderlake.dll": "C03FDAF376FA1B10EF79DF1FDA9ECBB1C75858D7AEC8E4D6C79C1AE883F51D3D",
    RUNTIME_DIR / "ggml-cpu-cannonlake.dll": "6A6EF755425DE2A1BAC71B8CD5003882D9B7FA10807676948F050012F3F9026C",
    RUNTIME_DIR / "ggml-cpu-cascadelake.dll": "765D97EC6785D82AF76B023484A8F39C662D2C75BCB45649DE375A251B94F003",
    RUNTIME_DIR / "ggml-cpu-haswell.dll": "CDE7120183E5A83C1EB8D234916E087635CD4B809695B535BE9E1355C902A8A2",
    RUNTIME_DIR / "ggml-cpu-icelake.dll": "61FB173672BFE5AA7A8458F478DDF614D4F6BBA57434CF4D45D7724DC3C07F48",
    RUNTIME_DIR / "ggml-cpu-sandybridge.dll": "31F767685E1D41844EC5F83B09A7A80E2F1861EB23677EC848FB7C458188FB57",
    RUNTIME_DIR / "ggml-cpu-skylakex.dll": "FC63A42A473D3DA61512191A20B61D8A0D046F03A8FE537BDAFCC01D14D43DC4",
    RUNTIME_DIR / "ggml-cpu-sse42.dll": "8F2B1314B69103F488FE765BD3EF578FA1C7F4B874FE49BEEC55667BE8246BEE",
    RUNTIME_DIR / "ggml-cpu-x64.dll": "5D06E975855E72E0CFD8E2C140B9DA4C433422B98CFD40468BD41761EA202E5F",
    BINDING_ROOT / "transcribe_cpp" / "__init__.py": "DDAEA2436C8B17F0E45DA7808A923824AF0848CA6A6F665EABE9C1BEDEAC8E12",
    BINDING_ROOT / "transcribe_cpp" / "_generated.py": "08D5963072D490A5F4E4A1D38EEAC755920D1926687C5684F5E0618FC1348633",
    BINDING_ROOT / "transcribe_cpp" / "_library.py": "DDAE8A4A3E7544F168D691CEEE3F9977C9694E97B0CCC189D993375E117AB0E5",
    BINDING_ROOT / "transcribe_cpp" / "_abi.py": "F9A539B7F3DDA8A85900C68BF6D0007F35E8562C2BDABD439677D93078F2A9A6",
    BINDING_ROOT / "transcribe_cpp" / "errors.py": "A38813B403672788C6A3AB3DB303BEBD183D1D3BC3F02F9326369683284E85AB",
    BINDING_ROOT / "transcribe_cpp" / "py.typed": "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855",
    SOURCE_ROOT / "LICENSE": "ACEF7D1711847656CDEB0007784DBE19BD7FD069767B0B38BD62714C6A8F3159",
    SOURCE_ROOT / "THIRD-PARTY-LICENSES.md": "5ABF5CCE044B53CF14822CDB0E6992A4159EA1B184907E98020C8D3D2EF66EA2",
    FFMPEG: "5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D",
}
EXPECTED_RUNTIME_FILE_NAMES = {
    "contract.json",
    "ggml-base.dll",
    "ggml-cpu-alderlake.dll",
    "ggml-cpu-cannonlake.dll",
    "ggml-cpu-cascadelake.dll",
    "ggml-cpu-haswell.dll",
    "ggml-cpu-icelake.dll",
    "ggml-cpu-sandybridge.dll",
    "ggml-cpu-skylakex.dll",
    "ggml-cpu-sse42.dll",
    "ggml-cpu-x64.dll",
    "ggml-vulkan.dll",
    "ggml.dll",
    "transcribe.dll",
}
EXPECTED_BINDING_FILE_NAMES = {
    "__init__.py",
    "_abi.py",
    "_generated.py",
    "_library.py",
    "errors.py",
    "py.typed",
}

SHA256_RE = re.compile(r"^[0-9A-Fa-f]{64}$")
RAW_TIMESTAMP_RE = re.compile(r"\[(\d+(?:\.\d+)?)\]")
RAW_TURN_START_RE = re.compile(r"\[(\d+(?:\.\d+)?)\]\[S(\d{2})\]")
RAW_TURN_END_RE = re.compile(
    r"\[(\d+(?:\.\d+)?)\](?=(?:\[\d+(?:\.\d+)?\]\[S\d{2}\])|\s*$)"
)
SAFE_NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,79}$")
INFERENCE_ENV_PREFIXES = ("TRANSCRIBE_", "GGML_")
INFERENCE_ENV_NAMES = {
    "PYTHONHOME",
    "PYTHONPATH",
    "VK_ADD_LAYER_PATH",
    "VK_DRIVER_FILES",
    "VK_ICD_FILENAMES",
    "VK_INSTANCE_LAYERS",
    "VK_LAYER_PATH",
    "VULKAN_SDK",
}


class GateError(RuntimeError):
    """A controlled gate failure with an actionable message."""


def now_iso() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat()


def validate_inference_environment() -> dict[str, Any]:
    found = sorted(
        key
        for key in os.environ
        if key.upper().startswith(INFERENCE_ENV_PREFIXES)
        or key.upper() in INFERENCE_ENV_NAMES
    )
    if found:
        raise GateError(
            "P1 inference environment contains behavior-changing variables: "
            + ", ".join(found)
        )
    return {
        "state": "PASS",
        "policy": "reject_behavior_changing_environment_variables",
        "checked_prefixes": list(INFERENCE_ENV_PREFIXES),
        "checked_names": sorted(INFERENCE_ENV_NAMES),
        "present_names": [],
    }


def sha256(path: Path, chunk_size: int = 8 * 1024 * 1024) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(chunk_size):
            digest.update(chunk)
    return digest.hexdigest().upper()


def frozen_git_run(arguments: list[str], *, check: bool = False) -> subprocess.CompletedProcess[str]:
    """Run the frozen Git executable without inheriting repository override variables."""

    environment = {
        "SystemRoot": r"C:\Windows",
        "WINDIR": r"C:\Windows",
        "COMSPEC": r"C:\Windows\System32\cmd.exe",
        "PATH": os.pathsep.join(
            [str(GIT_EXECUTABLE.parent), r"C:\Windows\System32", r"C:\Windows"]
        ),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "NUL",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_TERMINAL_PROMPT": "0",
        "LC_ALL": "C",
    }
    return subprocess.run(
        [
            str(GIT_EXECUTABLE),
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=NUL",
            *arguments,
        ],
        check=check,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="strict",
        env=environment,
    )


def require_file(path: Path, label: str) -> Path:
    resolved = path.resolve()
    if not resolved.is_file():
        raise GateError(f"{label} does not exist or is not a file: {resolved}")
    return resolved


def require_sha256(value: str, label: str) -> str:
    if not SHA256_RE.fullmatch(value):
        raise GateError(f"{label} must be exactly 64 hexadecimal characters")
    return value.upper()


def require_under(path: Path, root: Path, label: str) -> None:
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError as exc:
        raise GateError(f"{label} must stay under {root}: {path}") from exc


def file_record(path: Path, *, with_hash: bool = True) -> dict[str, Any]:
    resolved = require_file(path, "file")
    record: dict[str, Any] = {
        "path": str(resolved),
        "bytes": resolved.stat().st_size,
    }
    if with_hash:
        record["sha256"] = sha256(resolved)
    return record


def flat_directory_manifest(path: Path) -> dict[str, Any]:
    resolved = path.resolve()
    entries = []
    for item in sorted((entry for entry in resolved.iterdir() if entry.is_file()), key=lambda entry: entry.name):
        entries.append((item.name, item.stat().st_size, sha256(item)))
    digest = hashlib.sha256()
    for name, size, file_hash in entries:
        digest.update(f"{name}\0{size}\0{file_hash}\n".encode("utf-8"))
    return {
        "path": str(resolved),
        "file_count": len(entries),
        "bytes": sum(size for _, size, _ in entries),
        "manifest_sha256": digest.hexdigest().upper(),
        "file_names": [name for name, _, _ in entries],
    }


def base_runtime_manifest(path: Path) -> dict[str, Any]:
    resolved = path.resolve()
    entries = []
    for item in resolved.rglob("*"):
        if not item.is_file():
            continue
        relative = item.relative_to(resolved).as_posix()
        if (
            "/__pycache__/" in f"/{relative}"
            or relative.startswith("Lib/site-packages/")
            or relative.startswith("Scripts/")
        ):
            continue
        entries.append((relative, item.stat().st_size, sha256(item)))
    entries.sort()
    digest = hashlib.sha256()
    for relative, size, file_hash in entries:
        digest.update(f"{relative}\0{size}\0{file_hash}\n".encode("utf-8"))
    return {
        "path": str(resolved),
        "file_count": len(entries),
        "bytes": sum(size for _, size, _ in entries),
        "manifest_sha256": digest.hexdigest().upper(),
    }


def expected_lock_document() -> dict[str, Any]:
    samples = []
    for name, sample in EXPECTED_SAMPLES.items():
        row: dict[str, Any] = {
            "name": name,
            "bytes": sample["bytes"],
            "sha256": sample["sha256"],
            "source_timeline_duration_seconds": sample[
                "source_timeline_duration_seconds"
            ],
            "prepared": sample["prepared"],
        }
        if sample.get("max_rtf") is not None:
            row["max_controlled_runner_rtf"] = float(sample["max_rtf"])
        samples.append(row)
    runtime_hashes = {
        name: EXPECTED_HASHES[RUNTIME_DIR / name]
        for name in sorted(EXPECTED_RUNTIME_FILE_NAMES)
    }
    binding_hashes = {
        name: EXPECTED_HASHES[BINDING_ROOT / "transcribe_cpp" / name]
        for name in sorted(EXPECTED_BINDING_FILE_NAMES)
    }
    return {
        "schema_version": 1,
        "stage": "MOSS_V3_P1",
        "model": {
            "repository": "handy-computer/moss-transcribe-diarize-gguf",
            "revision": "6fdfa33aed776bbb0ac11a1a9835634fe6d75dd7",
            "file_name": EXPECTED_MODEL_NAME,
            "bytes": EXPECTED_MODEL_BYTES,
            "sha256": EXPECTED_MODEL_SHA256,
            "license_spdx": "Apache-2.0",
            "license_source": "https://huggingface.co/OpenMOSS-Team/MOSS-Transcribe-Diarize",
            "upstream_project_repository": "https://github.com/OpenMOSS/MOSS-Transcribe-Diarize",
            "upstream_project_revision": EXPECTED_UPSTREAM_MODEL_REVISION,
            "local_license_bytes": EXPECTED_MODEL_LICENSE_BYTES,
            "local_license_sha256": EXPECTED_MODEL_LICENSE_SHA256,
        },
        "runtime": {
            "project": "handy-computer/transcribe.cpp",
            "version": EXPECTED_API_VERSION,
            "source_commit": EXPECTED_SOURCE_COMMIT,
            "primary_backend": "Intel Arc Vulkan",
            "scheduler_cpu_fallback_present": True,
            "cpu_preprocessing_present": True,
            "gpu_only_execution_proven": False,
            "license_sha256": EXPECTED_HASHES[SOURCE_ROOT / "LICENSE"],
            "third_party_licenses_sha256": EXPECTED_HASHES[
                SOURCE_ROOT / "THIRD-PARTY-LICENSES.md"
            ],
            "runtime_files": runtime_hashes,
            "selected_device": {
                "kind": EXPECTED_DEVICE_KIND,
                "description": EXPECTED_DEVICE_DESCRIPTION,
                "device_id": EXPECTED_DEVICE_ID,
                "device_type": EXPECTED_DEVICE_TYPE,
            },
        },
        "python": {
            "relative_role": "P1 controlled runner",
            "version": ".".join(str(value) for value in EXPECTED_PYTHON_VERSION),
            "executable_sha256": EXPECTED_PYTHON_SHA256,
            "psutil_version": EXPECTED_PSUTIL_VERSION,
            "psutil_init_sha256": EXPECTED_PSUTIL_INIT_SHA256,
            "psutil_native_sha256": EXPECTED_PSUTIL_NATIVE_SHA256,
            "psutil_package_manifest_sha256": EXPECTED_PSUTIL_PACKAGE_MANIFEST_SHA256,
            "base_executable_sha256": EXPECTED_BASE_EXECUTABLE_SHA256,
            "python_dll_sha256": EXPECTED_PYTHON_DLL_SHA256,
            "base_runtime_manifest_sha256": EXPECTED_BASE_RUNTIME_MANIFEST_SHA256,
            "base_runtime_scope": "C_DRIVE_CODEX_CACHE_TEST_ONLY_NOT_PRODUCT_RUNTIME",
            "pyvenv_cfg_sha256": EXPECTED_PYVENV_CFG_SHA256,
            "venv_scripts_manifest_sha256": EXPECTED_VENV_SCRIPTS_MANIFEST_SHA256,
        },
        "host": {
            "scope": "current test host only",
            "fingerprint_sha256": EXPECTED_HOST_FINGERPRINT_SHA256,
            "windows_version": "10.0.26100",
            "cpu": "Intel(R) Core(TM) Ultra 7 155H",
            "gpu": EXPECTED_DEVICE_DESCRIPTION,
            "gpu_driver": "32.0.101.6127",
            "vulkan_loader_version": "1.3.300.0",
            "vulkan_loader_sha256": EXPECTED_VULKAN_LOADER_SHA256,
            "intel_vulkan_modules": {
                path.name: digest
                for path, digest in sorted(
                    EXPECTED_INTEL_VULKAN_MODULES.items(), key=lambda item: item[0].name.lower()
                )
            },
            "powershell_sha256": EXPECTED_POWERSHELL_SHA256,
            "icacls_sha256": EXPECTED_ICACLS_SHA256,
            "git_sha256": EXPECTED_GIT_SHA256,
        },
        "binding_files": binding_hashes,
        "thresholds": {
            "silence_detection_minimum_seconds": SILENCE_MIN_DURATION_SECONDS,
            "output_end_tolerance_seconds": OUTPUT_END_TOLERANCE_SECONDS,
            "minimum_activity_coverage_ratio": MIN_ACTIVITY_COVERAGE_RATIO,
            "maximum_uncovered_activity_gap_seconds": MAX_UNCOVERED_ACTIVITY_GAP_SECONDS,
            "maximum_same_speaker_overlap_seconds": MAX_SAME_SPEAKER_OVERLAP_SECONDS,
            "maximum_active_seconds_per_segment": MAX_ACTIVE_SECONDS_PER_SEGMENT,
            "minimum_text_characters_per_transcript_second": MIN_TEXT_CHARS_PER_TRANSCRIPT_SECOND,
            "business_737s_maximum_controlled_runner_rtf": 1.0,
        },
        "samples": samples,
        "accuracy_status": "NOT_SCORABLE_UNTIL_P0_R_PASS",
    }


def validate_lock_document(lock: dict[str, Any]) -> None:
    if lock != expected_lock_document():
        raise GateError("P1 lock document differs from the exact frozen schema/content")


def validate_approved_toolchain_manifest(
    commit: str, locked_paths: list[Path]
) -> dict[str, Any]:
    manifest = read_json(
        APPROVED_TOOLCHAIN_MANIFEST, "external approved P1 toolchain manifest"
    )
    expected_relatives = {
        path.relative_to(REPO_ROOT).as_posix(): sha256(path) for path in locked_paths
    }
    expected_document = {
        "schema_version": 1,
        "stage": "MOSS_V3_P1_APPROVED_TOOLCHAIN",
        "git_commit": commit,
        "files": expected_relatives,
    }
    if manifest != expected_document:
        raise GateError(
            "external approved P1 toolchain manifest does not match this commit/file set"
        )
    return file_record(APPROVED_TOOLCHAIN_MANIFEST)


def validate_test_toolchain() -> dict[str, Any]:
    if not sys.flags.isolated or not sys.flags.no_site or not sys.flags.ignore_environment:
        raise GateError("P1 must start Python with -I -S before any site startup hooks execute")
    if not sys.dont_write_bytecode:
        raise GateError("P1 must start Python with -B so the frozen cache root stays empty")
    if sys.pycache_prefix is None or Path(sys.pycache_prefix).resolve() != PYTHON_CACHE_ROOT:
        raise GateError(
            "P1 must start with -X pycache_prefix pointing at the frozen empty cache root"
        )
    if not PYTHON_CACHE_ROOT.is_dir() or any(PYTHON_CACHE_ROOT.rglob("*")):
        raise GateError("P1 frozen Python cache root is missing or not empty")
    executable = Path(sys.executable).resolve()
    if executable != EXPECTED_PYTHON.resolve():
        raise GateError(
            f"P1 must use the frozen Python executable {EXPECTED_PYTHON}, got {executable}"
        )
    if sys.version_info[:3] != EXPECTED_PYTHON_VERSION:
        raise GateError(
            f"unexpected Python version {sys.version_info[:3]}, expected {EXPECTED_PYTHON_VERSION}"
        )
    if sha256(executable) != EXPECTED_PYTHON_SHA256:
        raise GateError("frozen Python executable hash mismatch")
    require_file(EXPECTED_POWERSHELL, "frozen Windows PowerShell executable")
    if sha256(EXPECTED_POWERSHELL) != EXPECTED_POWERSHELL_SHA256:
        raise GateError("frozen Windows PowerShell executable hash mismatch")
    require_file(EXPECTED_ICACLS, "frozen Windows icacls executable")
    if sha256(EXPECTED_ICACLS) != EXPECTED_ICACLS_SHA256:
        raise GateError("frozen Windows icacls executable hash mismatch")
    if Path(sys.base_prefix).resolve() != EXPECTED_BASE_PREFIX.resolve():
        raise GateError("frozen Python base_prefix mismatch")
    pyvenv_cfg = EXPECTED_PYTHON.resolve().parents[1] / "pyvenv.cfg"
    if sha256(require_file(pyvenv_cfg, "frozen pyvenv.cfg")) != EXPECTED_PYVENV_CFG_SHA256:
        raise GateError("frozen pyvenv.cfg hash mismatch")
    scripts_manifest = flat_directory_manifest(EXPECTED_PYTHON.resolve().parent)
    if (
        scripts_manifest["file_count"] != EXPECTED_VENV_SCRIPTS_FILE_COUNT
        or scripts_manifest["bytes"] != EXPECTED_VENV_SCRIPTS_BYTES
        or scripts_manifest["manifest_sha256"] != EXPECTED_VENV_SCRIPTS_MANIFEST_SHA256
    ):
        raise GateError("frozen venv Scripts directory manifest mismatch")
    unexpected_pth = []
    for directory in (EXPECTED_PYTHON.resolve().parent, EXPECTED_BASE_PREFIX.resolve()):
        unexpected_pth.extend(directory.glob("*._pth"))
    if unexpected_pth:
        raise GateError("unexpected Python _pth startup override file is present")
    if not EXPECTED_SITE_PACKAGES.is_dir():
        raise GateError("protected minimal dependency root is missing")
    audit_restricted_acl(
        EXPECTED_SITE_PACKAGES,
        "minimal Python dependency root",
        EXPECTED_SITE_PACKAGES,
    )
    dependency_children = {entry.name for entry in EXPECTED_SITE_PACKAGES.iterdir()}
    if dependency_children != {"psutil"}:
        raise GateError("minimal Python dependency root contains an unexpected entry")
    psutil_root = EXPECTED_SITE_PACKAGES / "psutil"
    audit_restricted_acl(
        psutil_root, "frozen psutil package", EXPECTED_SITE_PACKAGES
    )
    psutil_manifest = flat_directory_manifest(psutil_root)
    if (
        psutil_manifest["file_count"] != EXPECTED_PSUTIL_PACKAGE_FILE_COUNT
        or psutil_manifest["manifest_sha256"] != EXPECTED_PSUTIL_PACKAGE_MANIFEST_SHA256
    ):
        raise GateError("psutil package file set/hash mismatch before import")
    stdlib_root = EXPECTED_BASE_PREFIX.resolve() / "Lib"
    for module in (ctypes, importlib.metadata):
        module_path = Path(str(module.__file__)).resolve()
        require_under(module_path, stdlib_root, f"stdlib module {module.__name__}")
    sys.path.insert(0, str(EXPECTED_SITE_PACKAGES))
    try:
        import psutil  # type: ignore
    except ImportError as exc:
        raise GateError("frozen P1 psutil dependency is missing") from exc
    finally:
        try:
            sys.path.remove(str(EXPECTED_SITE_PACKAGES))
        except ValueError:
            pass
    psutil_init = Path(psutil.__file__).resolve()
    psutil_native = psutil_init.with_name("_psutil_windows.pyd")
    if str(psutil.__version__) != EXPECTED_PSUTIL_VERSION:
        raise GateError(
            f"unexpected psutil version {psutil.__version__}, expected {EXPECTED_PSUTIL_VERSION}"
        )
    if sha256(psutil_init) != EXPECTED_PSUTIL_INIT_SHA256:
        raise GateError("psutil Python file hash mismatch")
    if sha256(psutil_native) != EXPECTED_PSUTIL_NATIVE_SHA256:
        raise GateError("psutil native extension hash mismatch")
    if psutil_init.parent != psutil_root.resolve():
        raise GateError("psutil imported from an unexpected directory")

    base_executable = Path(getattr(sys, "_base_executable", "")).resolve()
    python_dll = Path(sys.base_prefix).resolve() / "python312.dll"
    if sha256(base_executable) != EXPECTED_BASE_EXECUTABLE_SHA256:
        raise GateError("base Python executable hash mismatch")
    if sha256(python_dll) != EXPECTED_PYTHON_DLL_SHA256:
        raise GateError("base Python DLL hash mismatch")
    base_manifest = base_runtime_manifest(Path(sys.base_prefix))
    if (
        base_manifest["file_count"] != EXPECTED_BASE_RUNTIME_FILE_COUNT
        or base_manifest["bytes"] != EXPECTED_BASE_RUNTIME_BYTES
        or base_manifest["manifest_sha256"] != EXPECTED_BASE_RUNTIME_MANIFEST_SHA256
    ):
        raise GateError("base Python runtime manifest mismatch")

    binding_manifest = flat_directory_manifest(BINDING_ROOT / "transcribe_cpp")
    if set(binding_manifest["file_names"]) != EXPECTED_BINDING_FILE_NAMES:
        raise GateError("transcribe_cpp binding file set mismatch")
    for binding_name in EXPECTED_BINDING_FILE_NAMES:
        binding_path = BINDING_ROOT / "transcribe_cpp" / binding_name
        if sha256(binding_path) != EXPECTED_HASHES[binding_path]:
            raise GateError(f"transcribe_cpp binding hash mismatch: {binding_name}")

    require_file(GIT_EXECUTABLE, "frozen Git executable")
    if sha256(GIT_EXECUTABLE) != EXPECTED_GIT_SHA256:
        raise GateError("frozen Git executable hash mismatch")

    locked_paths = [
        RUNNER_PATH,
        TEST_PATH,
        LOCK_PATH,
        SUPERVISOR_PATH,
        SUPERVISOR_TEST_PATH,
        CHUNKED_PATH,
        CHUNKED_TEST_PATH,
    ]
    for path in locked_paths:
        require_file(path, "P1 test source")
        tracked = frozen_git_run(
            ["-C", str(REPO_ROOT), "ls-files", "--error-unmatch", str(path.relative_to(REPO_ROOT))]
        )
        if tracked.returncode != 0:
            raise GateError(f"P1 test source is not committed: {path}")
    status = frozen_git_run(
        [
            "-C",
            str(REPO_ROOT),
            "status",
            "--porcelain=v1",
            "--",
            *(str(path.relative_to(REPO_ROOT)) for path in locked_paths),
        ],
        check=True,
    ).stdout.strip()
    if status:
        raise GateError("P1 runner/test/lock files have uncommitted changes")
    commit = frozen_git_run(
        ["-C", str(REPO_ROOT), "rev-parse", "HEAD"], check=True
    ).stdout.strip()
    approved_manifest = validate_approved_toolchain_manifest(commit, locked_paths)
    lock = read_json(LOCK_PATH, "P1 lock file")
    validate_lock_document(lock)
    # Mutation is permitted only after every runner/test/lock/manifest identity
    # above has been authenticated.  Re-check the dependency bytes after ACL
    # enforcement so the write does not create a trust gap.
    enforce_and_audit_restricted_acl(
        EXPECTED_SITE_PACKAGES,
        "minimal Python dependency root",
        EXPECTED_SITE_PACKAGES,
    )
    enforce_and_audit_restricted_acl(
        psutil_root, "frozen psutil package", EXPECTED_SITE_PACKAGES
    )
    post_acl_psutil_manifest = flat_directory_manifest(psutil_root)
    if post_acl_psutil_manifest != psutil_manifest:
        raise GateError("psutil package changed during toolchain ACL enforcement")
    return {
        "git_commit": commit,
        "git_status": "clean_for_all_p1_gate_sources",
        "runner": file_record(RUNNER_PATH),
        "tests": file_record(TEST_PATH),
        "lock": file_record(LOCK_PATH),
        "supervisor": file_record(SUPERVISOR_PATH),
        "supervisor_tests": file_record(SUPERVISOR_TEST_PATH),
        "chunked_runner": file_record(CHUNKED_PATH),
        "chunked_tests": file_record(CHUNKED_TEST_PATH),
        "git": file_record(GIT_EXECUTABLE),
        "powershell": file_record(EXPECTED_POWERSHELL),
        "icacls": file_record(EXPECTED_ICACLS),
        "approved_toolchain_manifest": approved_manifest,
        "python": {
            **file_record(executable),
            "version": ".".join(str(value) for value in sys.version_info[:3]),
            "base_executable": file_record(base_executable),
            "python_dll": file_record(python_dll),
            "base_runtime": base_manifest,
            "storage_scope": "C_DRIVE_CODEX_CACHE_TEST_ONLY_NOT_PRODUCT_RUNTIME",
            "pyvenv_cfg": file_record(pyvenv_cfg),
            "venv_scripts": scripts_manifest,
        },
        "psutil": {
            "version": str(psutil.__version__),
            "python_file": file_record(psutil_init),
            "native_file": file_record(psutil_native),
            "package_manifest": psutil_manifest,
        },
        "binding_manifest": binding_manifest,
    }


def capture_and_validate_host() -> dict[str, Any]:
    command = r"""
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new()
$os=Get-CimInstance Win32_OperatingSystem
$sys=Get-CimInstance Win32_ComputerSystemProduct
$cpu=Get-CimInstance Win32_Processor | Select-Object -First 1
$gpu=Get-CimInstance Win32_VideoController | Where-Object {$_.Name -eq 'Intel(R) Arc(TM) Graphics'} | Select-Object -First 1
$battery=Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue | Select-Object -First 1
$canonical=('os={0}|version={1}|build={2}|arch={3}|vendor={4}|model={5}|uuid={6}|cpu={7}|gpu={8}|driver={9}' -f $os.Caption,$os.Version,$os.BuildNumber,$os.OSArchitecture,$sys.Vendor,$sys.Name,$sys.UUID,$cpu.Name,$gpu.Name,$gpu.DriverVersion)
$sha=[Security.Cryptography.SHA256]::Create()
try {
  $fingerprint=(($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($canonical))) | ForEach-Object {$_.ToString('X2')}) -join ''
} finally {
  $sha.Dispose()
}
[pscustomobject]@{os=$os.Caption;version=$os.Version;build=$os.BuildNumber;arch=$os.OSArchitecture;vendor=$sys.Vendor;model=$sys.Name;cpu=$cpu.Name;gpu=$gpu.Name;driver=$gpu.DriverVersion;fingerprint_sha256=$fingerprint;battery_status=$battery.BatteryStatus;battery_percent=$battery.EstimatedChargeRemaining} | ConvertTo-Json -Compress
""".strip()
    completed = subprocess.run(
        [str(EXPECTED_POWERSHELL), "-NoProfile", "-Command", command],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="strict",
        env={
            "SystemRoot": r"C:\Windows",
            "WINDIR": r"C:\Windows",
            "COMSPEC": r"C:\Windows\System32\cmd.exe",
            "PATH": r"C:\Windows\System32;C:\Windows",
            "PSModulePath": r"C:\Windows\System32\WindowsPowerShell\v1.0\Modules",
        },
    )
    host = json.loads(completed.stdout)
    if host.get("fingerprint_sha256") != EXPECTED_HOST_FINGERPRINT_SHA256:
        raise GateError("current machine/OS/CPU/GPU driver does not match the frozen P1 host")
    loader = file_record(EXPECTED_VULKAN_LOADER)
    if loader["sha256"] != EXPECTED_VULKAN_LOADER_SHA256:
        raise GateError("Vulkan loader hash mismatch on the frozen P1 host")
    host["vulkan_loader"] = loader
    return host


def validate_frozen_files() -> dict[str, Any]:
    actual_runtime_names = {
        path.name for path in RUNTIME_DIR.iterdir() if path.is_file()
    }
    if actual_runtime_names != EXPECTED_RUNTIME_FILE_NAMES:
        raise GateError(
            "runtime directory file set mismatch: "
            f"missing={sorted(EXPECTED_RUNTIME_FILE_NAMES - actual_runtime_names)}, "
            f"unexpected={sorted(actual_runtime_names - EXPECTED_RUNTIME_FILE_NAMES)}"
        )
    license_record = validate_locked_file(
        EXPECTED_MODEL_LICENSE,
        EXPECTED_MODEL_LICENSE_BYTES,
        EXPECTED_MODEL_LICENSE_SHA256,
        "frozen upstream MOSS model license",
    )
    records = []
    for path, expected_hash in EXPECTED_HASHES.items():
        resolved = require_file(path, "frozen P1 dependency")
        actual_hash = sha256(resolved)
        if actual_hash != expected_hash:
            raise GateError(
                f"frozen dependency hash mismatch for {resolved}: "
                f"expected {expected_hash}, got {actual_hash}"
            )
        records.append(
            {
                "path": str(resolved),
                "bytes": resolved.stat().st_size,
                "sha256": actual_hash,
            }
        )

    contract_path = RUNTIME_DIR / "contract.json"
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    if contract.get("version") != EXPECTED_API_VERSION:
        raise GateError(f"unexpected native contract version: {contract!r}")
    if contract.get("lane") != "cpu-vulkan":
        raise GateError(f"unexpected native contract lane: {contract!r}")
    if contract.get("header_hash") != "7df72bf9e667b8c2":
        raise GateError(f"unexpected native header hash: {contract!r}")
    if contract.get("backends") != ["vulkan", "cpu"]:
        raise GateError(f"unexpected native backend list: {contract!r}")

    source_commit = frozen_git_run(
        ["-C", str(SOURCE_ROOT), "rev-parse", "HEAD"], check=True
    ).stdout.strip()
    if source_commit != EXPECTED_SOURCE_COMMIT:
        raise GateError(
            f"unexpected transcribe.cpp source commit: {source_commit}"
        )
    source_status = frozen_git_run(
        [
            "-C",
            str(SOURCE_ROOT),
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
        check=True,
    ).stdout.strip()
    if source_status:
        raise GateError(
            "frozen transcribe.cpp source has local changes; refusing to run: "
            + source_status.replace("\n", " | ")
        )

    return {
        "source_root": str(SOURCE_ROOT.resolve()),
        "source_commit": source_commit,
        "source_status": "clean",
        "binding_root": str(BINDING_ROOT.resolve()),
        "runtime_dir": str(RUNTIME_DIR.resolve()),
        "contract": contract,
        "files": records,
        "model_license": license_record,
    }


def load_transcribe_cpp() -> tuple[Any, dict[str, Any]]:
    runtime = validate_frozen_files()
    library = RUNTIME_DIR / "transcribe.dll"
    expected_module_file = (BINDING_ROOT / "transcribe_cpp" / "__init__.py").resolve()
    if "transcribe_cpp" in sys.modules:
        raise GateError(
            "transcribe_cpp was already imported before the frozen binding gate"
        )

    os.environ["TRANSCRIBE_LIBRARY"] = str(library.resolve())
    os.environ.pop("TRANSCRIBE_BACKEND", None)
    os.environ.pop("TRANSCRIBE_NATIVE_PROVIDER", None)
    # Do not pass an attacker-controlled inherited PATH into native dependency
    # lookup.  All subprocess tools used by this gate have absolute paths.
    os.environ["PATH"] = os.pathsep.join(
        [str(RUNTIME_DIR.resolve()), r"C:\Windows\System32", r"C:\Windows"]
    )
    binding_string = str(BINDING_ROOT.resolve())
    if binding_string not in sys.path:
        sys.path.insert(0, binding_string)
    if hasattr(os, "add_dll_directory"):
        # Keep the handle alive for the process lifetime.  It controls dependent
        # DLL lookup for the dynamically loaded native bundle on Windows.
        load_transcribe_cpp._dll_directory = os.add_dll_directory(  # type: ignore[attr-defined]
            str(RUNTIME_DIR.resolve())
        )

    module = importlib.import_module("transcribe_cpp")
    loaded_module_file = Path(module.__file__).resolve()
    loaded_library_file = Path(module.library_path()).resolve()
    if loaded_module_file != expected_module_file:
        raise GateError(
            f"wrong transcribe_cpp module loaded: expected {expected_module_file}, got {loaded_module_file}"
        )
    if loaded_library_file != library.resolve():
        raise GateError(
            f"wrong transcribe native library loaded: expected {library.resolve()}, got {loaded_library_file}"
        )
    api_version = str(module.__version__)
    native_version = str(module.native_version())
    native_commit = str(module.native_commit())
    if api_version != EXPECTED_API_VERSION or native_version != EXPECTED_API_VERSION:
        raise GateError(
            f"binding/native version mismatch: api={api_version}, native={native_version}"
        )
    if not native_commit.startswith(EXPECTED_NATIVE_COMMIT):
        raise GateError(f"unexpected native commit: {native_commit}")

    runtime["loaded"] = {
        "api_version": api_version,
        "native_version": native_version,
        "native_commit": native_commit,
        "module_path": str(loaded_module_file),
        "module_sha256": sha256(loaded_module_file),
        "library_path": str(loaded_library_file),
        "library_sha256": sha256(loaded_library_file),
        "native_provider": module.native_provider(),
    }
    return module, runtime


def device_record(device: Any) -> dict[str, Any]:
    return {
        "index": device.index,
        "name": device.name,
        "description": device.description,
        "kind": device.kind,
        "device_id": device.device_id,
        "device_type": device.device_type,
        "memory_total": device.memory_total,
        "memory_free": device.memory_free,
    }


def select_exact_vulkan_device(module: Any) -> tuple[Any, dict[str, Any]]:
    devices = list(module.backends())
    records = [device_record(device) for device in devices]
    matches = [
        device
        for device in devices
        if device.kind == EXPECTED_DEVICE_KIND
        and device.description == EXPECTED_DEVICE_DESCRIPTION
        and device.device_id == EXPECTED_DEVICE_ID
        and device.device_type == EXPECTED_DEVICE_TYPE
    ]
    if not module.backend_available(EXPECTED_DEVICE_KIND):
        raise GateError("the frozen runtime reports that Vulkan is unavailable")
    if len(matches) != 1:
        raise GateError(
            "expected exactly one Intel Arc Vulkan device; "
            f"found {len(matches)} exact matches in {records!r}"
        )
    selected = matches[0]
    return selected, {
        "selection_rule": {
            "kind": EXPECTED_DEVICE_KIND,
            "description": EXPECTED_DEVICE_DESCRIPTION,
            "device_id": EXPECTED_DEVICE_ID,
            "device_type": EXPECTED_DEVICE_TYPE,
            "exact_match_count": len(matches),
            "primary_device_fallback_allowed": False,
            "scheduler_cpu_fallback_present": True,
            "cpu_preprocessing_present": True,
            "gpu_only_execution_proven": False,
        },
        "registered_devices": records,
        "selected": device_record(selected),
    }


def native_module_profile(
    phase: str,
) -> tuple[dict[Path, str], dict[Path, str]]:
    """Return every locked module and the subset required in this phase."""

    expected = {
        EXPECTED_VULKAN_LOADER.resolve(): EXPECTED_VULKAN_LOADER_SHA256,
        (RUNTIME_DIR / "transcribe.dll").resolve(): EXPECTED_HASHES[
            RUNTIME_DIR / "transcribe.dll"
        ],
        (RUNTIME_DIR / "ggml.dll").resolve(): EXPECTED_HASHES[RUNTIME_DIR / "ggml.dll"],
        (RUNTIME_DIR / "ggml-base.dll").resolve(): EXPECTED_HASHES[
            RUNTIME_DIR / "ggml-base.dll"
        ],
        (RUNTIME_DIR / "ggml-vulkan.dll").resolve(): EXPECTED_HASHES[
            RUNTIME_DIR / "ggml-vulkan.dll"
        ],
        (RUNTIME_DIR / "ggml-cpu-alderlake.dll").resolve(): EXPECTED_HASHES[
            RUNTIME_DIR / "ggml-cpu-alderlake.dll"
        ],
        **{
            path.resolve(): digest
            for path, digest in EXPECTED_INTEL_VULKAN_MODULES.items()
        },
    }
    if phase == "inference":
        return expected, dict(expected)
    if phase == "device_only":
        inference_only = {
            path.resolve() for path in EXPECTED_INTEL_VULKAN_INFERENCE_ONLY_MODULES
        }
        return expected, {
            path: digest
            for path, digest in expected.items()
            if path not in inference_only
        }
    raise GateError(f"unknown native module capture phase: {phase}")


def validate_required_native_module_presence(
    required: dict[Path, str], mapped_by_identity: dict[str, Path]
) -> None:
    for expected_path in required:
        if os.path.normcase(str(expected_path)) not in mapped_by_identity:
            raise GateError(
                f"required native module is not mapped from the frozen path: {expected_path.name}"
            )


def capture_loaded_native_modules(*, phase: str) -> dict[str, Any]:
    """Prove which native DLL files the current process actually mapped."""

    try:
        import psutil  # type: ignore
    except ImportError as exc:  # pragma: no cover - frozen P1 toolchain includes it
        raise GateError("psutil is required to prove loaded native modules") from exc

    expected, required = native_module_profile(phase)
    mapped_by_identity: dict[str, Path] = {}
    try:
        for mapping in psutil.Process(os.getpid()).memory_maps(grouped=False):
            raw_path = str(getattr(mapping, "path", "") or "")
            if not raw_path or raw_path.startswith("["):
                continue
            candidate = Path(raw_path)
            try:
                resolved = candidate.resolve()
            except OSError:
                continue
            mapped_by_identity[os.path.normcase(str(resolved))] = resolved
    except (psutil.AccessDenied, psutil.NoSuchProcess, OSError) as exc:
        raise GateError("cannot enumerate current-process native modules") from exc

    validate_required_native_module_presence(required, mapped_by_identity)
    records: list[dict[str, Any]] = []
    allowed_not_loaded: list[str] = []
    for expected_path, expected_hash in expected.items():
        actual_path = mapped_by_identity.get(os.path.normcase(str(expected_path)))
        if actual_path is None:
            allowed_not_loaded.append(expected_path.name)
            continue
        actual_hash = sha256(actual_path)
        if actual_hash != expected_hash:
            raise GateError(
                f"mapped native module hash mismatch: {expected_path.name}"
            )
        records.append(
            {
                "path": str(actual_path),
                "file_name": actual_path.name,
                "bytes": actual_path.stat().st_size,
                "sha256": actual_hash,
            }
        )
    expected_identities = {os.path.normcase(str(path)) for path in expected}
    relevant_prefixes = ("ggml", "igvk", "igc", "intelcontrol")
    unexpected_relevant = sorted(
        path.name
        for identity, path in mapped_by_identity.items()
        if identity not in expected_identities
        and (
            path.name.lower() in {"vulkan-1.dll", "transcribe.dll"}
            or path.name.lower().startswith(relevant_prefixes)
        )
    )
    if unexpected_relevant:
        raise GateError(
            "unexpected mapped Vulkan/ggml module(s): "
            + ", ".join(unexpected_relevant)
        )
    return {
        "state": "PASS",
        "method": "psutil_process_memory_maps",
        "phase": phase,
        "required_module_count": len(required),
        "mapped_locked_module_count": len(records),
        "allowed_not_loaded": sorted(allowed_not_loaded),
        "unexpected_relevant_module_count": 0,
        "gpu_only_execution_proven": False,
        "scope": "primary Vulkan selection plus exact mapped runtime/Intel ICD identity",
        "modules": records,
    }


def validate_locked_file(
    path: Path,
    expected_bytes: int,
    expected_hash: str,
    label: str,
) -> dict[str, Any]:
    resolved = require_file(path, label)
    if expected_bytes <= 0:
        raise GateError(f"{label} expected byte count must be positive")
    normalized_hash = require_sha256(expected_hash, f"{label} SHA-256")
    actual_bytes = resolved.stat().st_size
    if actual_bytes != expected_bytes:
        raise GateError(
            f"{label} byte mismatch for {resolved}: "
            f"expected {expected_bytes}, got {actual_bytes}"
        )
    actual_hash = sha256(resolved)
    if actual_hash != normalized_hash:
        raise GateError(
            f"{label} SHA-256 mismatch for {resolved}: "
            f"expected {normalized_hash}, got {actual_hash}"
        )
    return {
        "path": str(resolved),
        "bytes": actual_bytes,
        "sha256": actual_hash,
    }


def validate_model(path: Path, expected_bytes: int, expected_hash: str) -> dict[str, Any]:
    resolved = path.resolve()
    require_under(resolved, MODEL_ROOT, "model")
    if resolved.name != EXPECTED_MODEL_NAME:
        raise GateError(
            f"P1 requires {EXPECTED_MODEL_NAME}, got {resolved.name}"
        )
    normalized_hash = require_sha256(expected_hash, "model SHA-256")
    if expected_bytes != EXPECTED_MODEL_BYTES or normalized_hash != EXPECTED_MODEL_SHA256:
        raise GateError(
            "command-line model identity does not match the frozen P1 Q8_0 model: "
            f"expected bytes={EXPECTED_MODEL_BYTES}, sha256={EXPECTED_MODEL_SHA256}"
        )
    record = validate_locked_file(
        resolved, EXPECTED_MODEL_BYTES, EXPECTED_MODEL_SHA256, "Q8_0 GGUF model"
    )
    record["quantization"] = "Q8_0"
    return record


def validate_sample_lock(args: argparse.Namespace) -> None:
    expected = EXPECTED_SAMPLES.get(args.sample_name)
    if expected is None:
        raise GateError(
            f"sample name is not in the frozen P1 set: {args.sample_name!r}"
        )
    if args.sample_bytes != expected["bytes"]:
        raise GateError(
            f"sample byte argument does not match the frozen {args.sample_name} value: "
            f"expected {expected['bytes']}, got {args.sample_bytes}"
        )
    if require_sha256(args.sample_sha256, "sample SHA-256") != expected["sha256"]:
        raise GateError(
            f"sample hash argument does not match the frozen {args.sample_name} value"
        )


def validate_prepared_lock(sample_name: str, record: dict[str, Any]) -> None:
    expected = EXPECTED_SAMPLES[sample_name]["prepared"]
    for key in ("bytes", "sha256", "frames"):
        if record.get(key) != expected[key]:
            raise GateError(
                f"prepared WAV {key} does not match the frozen {sample_name} value"
            )


def validate_pcm_lock(sample_name: str, record: dict[str, Any]) -> None:
    expected = EXPECTED_SAMPLES[sample_name]["prepared"]
    checks = {
        "samples": expected["frames"],
        "source_pcm_s16le_sha256": expected["pcm_s16le_sha256"],
        "float32_sha256": expected["float32_sha256"],
    }
    for key, expected_value in checks.items():
        if record.get(key) != expected_value:
            raise GateError(
                f"loaded PCM {key} does not match the frozen {sample_name} value"
            )


def wav_record(path: Path) -> dict[str, Any]:
    resolved = require_file(path, "WAV input")
    try:
        with wave.open(str(resolved), "rb") as handle:
            channels = handle.getnchannels()
            sample_width = handle.getsampwidth()
            sample_rate = handle.getframerate()
            frames = handle.getnframes()
            compression = handle.getcomptype()
    except (wave.Error, EOFError) as exc:
        raise GateError(f"invalid WAV file {resolved}: {exc}") from exc

    if channels != 1 or sample_width != 2 or sample_rate != 16000:
        raise GateError(
            f"WAV must be PCM s16le, 16 kHz, mono; got channels={channels}, "
            f"sample_width={sample_width}, sample_rate={sample_rate}: {resolved}"
        )
    if compression != "NONE":
        raise GateError(f"WAV must be uncompressed PCM, got {compression}: {resolved}")
    if frames <= 0:
        raise GateError(f"WAV has no audio frames: {resolved}")
    return {
        "path": str(resolved),
        "bytes": resolved.stat().st_size,
        "sha256": sha256(resolved),
        "format": "pcm_s16le",
        "channels": channels,
        "sample_width_bytes": sample_width,
        "sample_rate_hz": sample_rate,
        "frames": frames,
        "duration_seconds": frames / sample_rate,
    }


def read_json(path: Path, label: str) -> dict[str, Any]:
    def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise GateError(f"{label} contains a duplicate JSON key")
            result[key] = value
        return result

    try:
        payload = json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=reject_duplicate_keys
        )
    except (OSError, json.JSONDecodeError) as exc:
        raise GateError(f"cannot read {label} {path}: {exc}") from exc
    if not isinstance(payload, dict):
        raise GateError(f"{label} must contain one JSON object: {path}")
    return payload


FFMPEG_DURATION_RE = re.compile(
    r"Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)"
)


def probe_source_timeline(path: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [str(FFMPEG), "-nostdin", "-hide_banner", "-i", str(path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    match = FFMPEG_DURATION_RE.search(completed.stderr)
    if match is None:
        raise GateError("FFmpeg could not report the frozen source timeline duration")
    hours, minutes, seconds = match.groups()
    duration = int(hours) * 3600 + int(minutes) * 60 + float(seconds)
    return {
        "method": "ffmpeg_container_duration",
        "duration_seconds": duration,
        "diagnostic_line_count": len(completed.stderr.splitlines()),
    }


def conversion_identity(
    source: dict[str, Any], output: Path, source_timeline: dict[str, Any]
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "source": {
            "bytes": int(source["bytes"]),
            "sha256": str(source["sha256"]),
            "suffix": Path(str(source["path"])).suffix.lower(),
            "timeline_duration_seconds": source_timeline["duration_seconds"],
        },
        "output": {
            "file_role": "locked_16k_mono_pcm",
            "suffix": output.suffix.lower(),
        },
        "ffmpeg": {
            "bytes": FFMPEG.stat().st_size,
            "sha256": sha256(FFMPEG),
        },
        "conversion": {
            "channels": 1,
            "sample_rate_hz": 16000,
            "codec": "pcm_s16le",
            "metadata_removed": True,
            "bitexact_flags": True,
            "timeline_normalization": "aresample_async_first_pts_zero",
        },
    }


def validate_existing_conversion(
    output: Path,
    manifest_path: Path,
    identity: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any]]:
    if not output.is_file() or not manifest_path.is_file():
        raise GateError(
            "converted input and its manifest must either both exist or both be absent; "
            f"refusing to overwrite: output={output}, manifest={manifest_path}"
        )
    manifest = read_json(manifest_path, "conversion manifest")
    for key in ("schema_version", "source", "output", "ffmpeg", "conversion"):
        if manifest.get(key) != identity.get(key):
            raise GateError(
                f"existing converted input manifest does not match current {key}; "
                f"refusing to overwrite {output}"
            )
    output_record = wav_record(output)
    sanitized_output_record = {
        key: value for key, value in output_record.items() if key != "path"
    }
    recorded_output = manifest.get("prepared_audio")
    if recorded_output != sanitized_output_record:
        raise GateError(
            f"existing converted input hash/format no longer matches its manifest: {output}"
        )
    return output_record, {
        **identity,
        "reuse": True,
        "verified_at": now_iso(),
        "prepared_audio": sanitized_output_record,
        "historical_manifest_sha256": sha256(manifest_path),
    }


def prepare_audio(
    sample_name: str,
    source: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any] | None]:
    source_path = Path(source["path"])
    if source_path.suffix.lower() == ".wav":
        prepared = wav_record(source_path)
        if prepared["sha256"] != source["sha256"]:
            raise GateError("source WAV changed between input validation and WAV validation")
        validate_prepared_lock(sample_name, prepared)
        expected_duration = float(
            EXPECTED_SAMPLES[sample_name]["source_timeline_duration_seconds"]
        )
        if abs(float(prepared["duration_seconds"]) - expected_duration) > 1e-6:
            raise GateError("source WAV duration differs from the frozen timeline")
        return prepared, None

    require_file(FFMPEG, "frozen FFmpeg")
    INPUT_ROOT.mkdir(parents=True, exist_ok=True)
    output = INPUT_ROOT / f"{sample_name}-{source['sha256'][:16].lower()}-v2-16k-mono-pcm.wav"
    manifest_path = output.with_suffix(output.suffix + ".manifest.json")
    source_timeline = probe_source_timeline(source_path)
    expected_timeline = float(
        EXPECTED_SAMPLES[sample_name]["source_timeline_duration_seconds"]
    )
    if abs(float(source_timeline["duration_seconds"]) - expected_timeline) > 0.011:
        raise GateError("source container duration differs from the frozen timeline")
    identity = conversion_identity(source, output, source_timeline)

    if output.exists() or manifest_path.exists():
        converted = validate_existing_conversion(output, manifest_path, identity)
        validate_prepared_lock(sample_name, converted[0])
        timeline_delta = (
            float(converted[0]["duration_seconds"]) - expected_timeline
        )
        if abs(timeline_delta) > SOURCE_TIMELINE_TOLERANCE_SECONDS:
            raise GateError("reused prepared WAV timeline drift exceeds the frozen limit")
        converted[1]["prepared_minus_source_timeline_seconds"] = timeline_delta
        validate_locked_file(
            source_path, int(source["bytes"]), str(source["sha256"]), "source after conversion reuse"
        )
        return converted

    nonce = f"{os.getpid()}-{time.time_ns()}"
    temporary_output = INPUT_ROOT / f".{output.stem}.{nonce}.partial.wav"
    command = [
        str(FFMPEG.resolve()),
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-n",
        "-i",
        str(source_path),
        "-map",
        "0:a:0",
        "-map_metadata",
        "-1",
        "-vn",
        "-af",
        "aresample=16000:async=1:first_pts=0",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-c:a",
        "pcm_s16le",
        "-fflags",
        "+bitexact",
        "-flags:a",
        "+bitexact",
        str(temporary_output),
    ]
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    conversion_elapsed = time.perf_counter() - started
    if completed.returncode != 0:
        temporary_output.unlink(missing_ok=True)
        raise GateError(
            f"FFmpeg conversion failed with exit code {completed.returncode}: "
            f"{completed.stderr.strip()}"
        )

    try:
        output_record = wav_record(temporary_output)
        temporary_output.rename(output)  # Windows refuses to replace an existing target.
    except FileExistsError:
        temporary_output.unlink(missing_ok=True)
        return validate_existing_conversion(output, manifest_path, identity)
    except Exception:
        temporary_output.unlink(missing_ok=True)
        raise

    # Re-read under the final name so the recorded path and hash describe the
    # file that the model will actually consume.
    output_record = wav_record(output)
    validate_prepared_lock(sample_name, output_record)
    timeline_delta = float(output_record["duration_seconds"]) - expected_timeline
    if abs(timeline_delta) > SOURCE_TIMELINE_TOLERANCE_SECONDS:
        raise GateError(
            "prepared WAV timeline drift exceeds the frozen limit: "
            f"delta={timeline_delta:.6f}s"
        )
    manifest = {
        **identity,
        "created_at": now_iso(),
        "command_argument_count": len(command),
        "command_sha256": hashlib.sha256(
            json.dumps(command, ensure_ascii=False).encode("utf-8")
        ).hexdigest().upper(),
        "elapsed_seconds": conversion_elapsed,
        "return_code": completed.returncode,
        "source_timeline": source_timeline,
        "prepared_minus_source_timeline_seconds": timeline_delta,
        "prepared_audio": {
            key: value for key, value in output_record.items() if key != "path"
        },
    }
    try:
        atomic_write_text_exclusive(
            manifest_path,
            json.dumps(manifest, ensure_ascii=False, indent=2, allow_nan=False) + "\n",
        )
    except Exception as exc:
        raise GateError(
            f"converted WAV was created but its manifest could not be written; "
            f"do not reuse or overwrite it until audited: {output}: {exc}"
        ) from exc
    validate_locked_file(
        source_path, int(source["bytes"]), str(source["sha256"]), "source after conversion"
    )
    return output_record, manifest


def validate_rtf(sample_name: str, rtf: float) -> float | None:
    max_rtf = EXPECTED_SAMPLES[sample_name].get("max_rtf")
    if max_rtf is not None and rtf > float(max_rtf):
        raise GateError(
            f"RTF gate failed for {sample_name}: "
            f"measured {rtf:.6f}, maximum {float(max_rtf):.6f}"
        )
    return float(max_rtf) if max_rtf is not None else None


def load_pcm_float32(path: Path) -> tuple[array.array[float], dict[str, Any]]:
    started = time.perf_counter()
    with wave.open(str(path), "rb") as handle:
        frames = handle.readframes(handle.getnframes())
    pcm16 = array.array("h")
    pcm16.frombytes(frames)
    if sys.byteorder == "big":
        pcm16.byteswap()
    pcm = array.array("f", (sample / 32768.0 for sample in pcm16))
    return pcm, {
        "samples": len(pcm),
        "source_pcm_s16le_sha256": hashlib.sha256(frames).hexdigest().upper(),
        "float32_sha256": hashlib.sha256(pcm.tobytes()).hexdigest().upper(),
        "float32_bytes": len(pcm) * pcm.itemsize,
        "elapsed_seconds": time.perf_counter() - started,
    }


def finite_or_none(value: float) -> float | None:
    return value if math.isfinite(value) else None


def normalize_transcript_text(value: str) -> str:
    """Normalize only whitespace for cross-layer text identity checks."""

    return re.sub(r"\s+", "", value)


def parse_raw_turns(raw_text: str) -> list[dict[str, Any]]:
    """Strictly parse complete ``[start][Sxx]text[end]`` MOSS turns.

    Parsed timestamps from the native convenience layer are not accepted as
    completeness truth because it may synthesize missing boundaries.  This
    parser only accepts timestamps that the model emitted in ``raw_text`` and
    requires the entire raw string to be consumed.
    """

    turns: list[dict[str, Any]] = []
    speaker_last_end: dict[int, float] = {}
    cursor = 0
    length = len(raw_text)
    while cursor < length:
        while cursor < length and raw_text[cursor].isspace():
            cursor += 1
        if cursor >= length:
            break
        start_match = RAW_TURN_START_RE.match(raw_text, cursor)
        if start_match is None:
            raise GateError(
                f"raw MOSS output is not a complete turn at character offset {cursor}"
            )
        start_seconds = float(start_match.group(1))
        speaker_token = start_match.group(2)
        speaker_id = int(speaker_token)
        if speaker_id <= 0 or speaker_id > 99:
            raise GateError(f"raw MOSS output has illegal speaker label S{speaker_token}")
        text_start = start_match.end()
        end_match = RAW_TURN_END_RE.search(raw_text, text_start)
        if end_match is None:
            raise GateError(
                f"raw MOSS turn S{speaker_token} at {start_seconds:.3f}s lacks an explicit end timestamp"
            )
        text = raw_text[text_start:end_match.start()]
        if not text.strip():
            raise GateError(
                f"raw MOSS turn S{speaker_token} at {start_seconds:.3f}s has empty text"
            )
        end_seconds = float(end_match.group(1))
        if end_seconds < start_seconds:
            raise GateError(
                f"raw MOSS turn S{speaker_token} has reversed time {start_seconds:.3f}..{end_seconds:.3f}s"
            )
        if turns and start_seconds < turns[-1]["start_seconds"]:
            raise GateError("raw MOSS start timestamps are not monotonic")
        previous_speaker_end = speaker_last_end.get(speaker_id, -1.0)
        if previous_speaker_end - start_seconds > MAX_SAME_SPEAKER_OVERLAP_SECONDS:
            raise GateError(
                f"raw MOSS speaker S{speaker_token} overlap exceeds "
                f"{MAX_SAME_SPEAKER_OVERLAP_SECONDS:.3f}s"
            )
        if end_seconds - start_seconds > MAX_ACTIVE_SECONDS_PER_SEGMENT:
            raise GateError(
                f"raw MOSS turn S{speaker_token} spans more than "
                f"{MAX_ACTIVE_SECONDS_PER_SEGMENT:.0f}s"
            )
        interval = (start_seconds, end_seconds)
        if any(
            interval == (turn["start_seconds"], turn["end_seconds"])
            for turn in turns
        ):
            raise GateError("raw MOSS output repeats an identical timestamp interval")
        turns.append(
            {
                "start_seconds": start_seconds,
                "end_seconds": end_seconds,
                "speaker_id": speaker_id,
                "speaker_label": f"S{speaker_token}",
                "text": text,
            }
        )
        speaker_last_end[speaker_id] = max(previous_speaker_end, end_seconds)
        cursor = end_match.end()
    if not turns:
        raise GateError("raw MOSS output contains no complete timestamped speaker turns")
    stripped_raw_text = raw_text.rstrip()
    final_end_match = next(iter(reversed(list(RAW_TURN_END_RE.finditer(stripped_raw_text)))), None)
    if final_end_match is None or final_end_match.end() != len(stripped_raw_text):
        raise GateError("raw MOSS output does not end with an explicit timestamp")
    return turns


def result_record(result: Any) -> dict[str, Any]:
    segments = [dataclasses.asdict(segment) for segment in result.segments]
    speaker_segments = []
    for segment in result.speaker_segments:
        row = dataclasses.asdict(segment)
        row["p"] = finite_or_none(float(row["p"]))
        speaker_segments.append(row)

    try:
        raw_turns = parse_raw_turns(result.raw_text)
    except BaseException as exc:
        # Preserve the native result only on the exception object.  The normal
        # evidence path remains unchanged; the restricted failure serializer
        # can then retain the malformed raw timestamp stream for diagnosis.
        exc.partial_result = result  # type: ignore[attr-defined]
        raise
    last_timestamp = raw_turns[-1]["end_seconds"]

    return {
        "text": result.text,
        "raw_text": result.raw_text,
        "language": result.language,
        "timestamp_kind": result.timestamp_kind,
        "segments": segments,
        "speaker_segments": speaker_segments,
        "segment_count": len(segments),
        "speaker_segment_count": len(speaker_segments),
        "speaker_ids": sorted(
            {
                row["speaker_id"]
                for row in segments + speaker_segments
                if row["speaker_id"] > 0
            }
        ),
        "raw_turns": raw_turns,
        "raw_turn_count": len(raw_turns),
        "last_timestamp_seconds": last_timestamp,
        "last_timestamp_source": "strict_raw_turn_end",
        "timings": dataclasses.asdict(result.timings),
    }


SILENCE_START_RE = re.compile(r"silence_start:\s*([0-9]+(?:\.[0-9]+)?)")
SILENCE_END_RE = re.compile(
    r"silence_end:\s*([0-9]+(?:\.[0-9]+)?)\s*\|\s*silence_duration:\s*([0-9]+(?:\.[0-9]+)?)"
)


def parse_silence_intervals(log_text: str, duration_seconds: float) -> list[dict[str, float]]:
    """Parse ffmpeg silencedetect events into closed intervals.

    A final ``silence_start`` without a matching ``silence_end`` is closed at
    the known WAV duration.  This is important for the 107 s control sample,
    whose planned second paragraph was never captured and is silent to EOF.
    """

    intervals: list[dict[str, float]] = []
    pending_start: float | None = None
    for line in log_text.splitlines():
        start_match = SILENCE_START_RE.search(line)
        if start_match:
            pending_start = float(start_match.group(1))
            continue
        end_match = SILENCE_END_RE.search(line)
        if end_match:
            end = min(duration_seconds, float(end_match.group(1)))
            duration = float(end_match.group(2))
            start = pending_start if pending_start is not None else max(0.0, end - duration)
            intervals.append(
                {
                    "start_seconds": max(0.0, start),
                    "end_seconds": max(0.0, end),
                    "duration_seconds": max(0.0, end - start),
                }
            )
            pending_start = None
    if pending_start is not None:
        start = max(0.0, min(duration_seconds, pending_start))
        intervals.append(
            {
                "start_seconds": start,
                "end_seconds": duration_seconds,
                "duration_seconds": max(0.0, duration_seconds - start),
            }
        )
    return intervals


def probe_audio_activity(path: Path, duration_seconds: float) -> dict[str, Any]:
    command = [
        str(FFMPEG),
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "info",
        "-i",
        str(path),
        "-af",
        f"silencedetect=noise={SILENCE_THRESHOLD_DB}dB:d={SILENCE_MIN_DURATION_SECONDS}",
        "-f",
        "null",
        os.devnull,
    ]
    completed = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if completed.returncode != 0:
        raise GateError(
            "ffmpeg activity probe failed: "
            + (completed.stderr.strip() or f"exit code {completed.returncode}")
        )
    intervals = parse_silence_intervals(completed.stderr, duration_seconds)
    trailing_silence = next(
        (
            interval
            for interval in reversed(intervals)
            if abs(interval["end_seconds"] - duration_seconds) <= 0.05
        ),
        None,
    )
    last_activity_end = (
        trailing_silence["start_seconds"] if trailing_silence is not None else duration_seconds
    )
    active_intervals: list[dict[str, float]] = []
    cursor = 0.0
    for interval in sorted(intervals, key=lambda item: item["start_seconds"]):
        start = max(cursor, float(interval["start_seconds"]))
        if start > cursor:
            active_intervals.append(
                {
                    "start_seconds": cursor,
                    "end_seconds": start,
                    "duration_seconds": start - cursor,
                }
            )
        cursor = max(cursor, float(interval["end_seconds"]))
    if cursor < duration_seconds:
        active_intervals.append(
            {
                "start_seconds": cursor,
                "end_seconds": duration_seconds,
                "duration_seconds": duration_seconds - cursor,
            }
        )
    significant_activity = [
        interval
        for interval in active_intervals
        if interval["duration_seconds"] >= MIN_ACTIVITY_INTERVAL_SECONDS
    ]
    if significant_activity:
        last_activity_end = significant_activity[-1]["end_seconds"]
    return {
        "method": "ffmpeg-silencedetect",
        "threshold_db": SILENCE_THRESHOLD_DB,
        "minimum_silence_seconds": SILENCE_MIN_DURATION_SECONDS,
        "command": command,
        "silence_intervals": intervals,
        "minimum_activity_interval_seconds": MIN_ACTIVITY_INTERVAL_SECONDS,
        "active_intervals": active_intervals,
        "significant_active_intervals": significant_activity,
        "trailing_silence": trailing_silence,
        "last_activity_end_seconds": last_activity_end,
    }


def merge_intervals(intervals: list[tuple[float, float]]) -> list[tuple[float, float]]:
    merged: list[list[float]] = []
    for start, end in sorted(intervals):
        if end <= start:
            continue
        if not merged or start > merged[-1][1]:
            merged.append([start, end])
        else:
            merged[-1][1] = max(merged[-1][1], end)
    return [(start, end) for start, end in merged]


def activity_coverage_metrics(
    transcript_intervals: list[tuple[float, float]],
    activity_intervals: list[tuple[float, float]],
) -> dict[str, float]:
    transcript = merge_intervals(transcript_intervals)
    activity = merge_intervals(activity_intervals)
    activity_seconds = sum(end - start for start, end in activity)
    covered_seconds = 0.0
    max_gap = 0.0
    for active_start, active_end in activity:
        cursor = active_start
        for text_start, text_end in transcript:
            if text_end <= active_start or text_start >= active_end:
                continue
            overlap_start = max(active_start, text_start)
            overlap_end = min(active_end, text_end)
            if overlap_start > cursor:
                max_gap = max(max_gap, overlap_start - cursor)
            if overlap_end > overlap_start:
                covered_seconds += overlap_end - overlap_start
                cursor = max(cursor, overlap_end)
        if cursor < active_end:
            max_gap = max(max_gap, active_end - cursor)
    return {
        "activity_seconds": activity_seconds,
        "covered_activity_seconds": covered_seconds,
        "activity_coverage_ratio": covered_seconds / activity_seconds if activity_seconds else 0.0,
        "max_uncovered_activity_gap_seconds": max_gap,
    }


def validate_output(
    sample_name: str,
    output: dict[str, Any],
    duration_seconds: float,
    activity: dict[str, Any],
) -> dict[str, Any]:
    failures: list[str] = []
    diagnostic_warnings: list[str] = []
    text = str(output.get("text") or "").strip()
    raw_text = str(output.get("raw_text") or "").strip()
    segments = output.get("segments") or []
    speaker_segments = output.get("speaker_segments") or []
    raw_turns = output.get("raw_turns") or []
    if not text or not raw_text:
        failures.append("model output text/raw_text is empty")
    if not segments:
        failures.append("no parsed transcript segments")
    if not speaker_segments:
        failures.append("diarization produced no speaker segments")
    reported_language = output.get("language")
    if reported_language not in ("", "zh"):
        failures.append(
            f"unexpected output language={reported_language!r}; expected 'zh' or native-unreported empty value"
        )
    if output.get("timestamp_kind") != "segment":
        failures.append(
            f"unexpected timestamp_kind={output.get('timestamp_kind')!r}; expected 'segment'"
        )
    if output.get("last_timestamp_source") != "strict_raw_turn_end":
        failures.append("last timestamp is not proven by a strict raw turn end marker")
    if len(raw_turns) != len(segments):
        failures.append(
            f"strict raw turn count differs from parsed segment count: "
            f"raw={len(raw_turns)}, parsed={len(segments)}"
        )
    if len(speaker_segments) != len(segments):
        failures.append(
            f"speaker segment count differs from transcript segment count: "
            f"speaker={len(speaker_segments)}, parsed={len(segments)}"
        )

    previous_start = -1
    previous_end = -1
    parsed_speaker_last_end: dict[int, int] = {}
    maximum_observed_same_speaker_overlap_ms = 0
    timestamps_valid = True
    for index, segment in enumerate(segments):
        start = int(segment["t0_ms"])
        end = int(segment["t1_ms"])
        if start < 0 or end < start:
            failures.append(f"segment {index} has invalid timestamps: {start}..{end} ms")
            timestamps_valid = False
        if start < previous_start:
            failures.append(f"segment {index} start timestamp is not monotonic")
            timestamps_valid = False
        if end > round((duration_seconds + OUTPUT_END_TOLERANCE_SECONDS) * 1000):
            failures.append(f"segment {index} ends after the audio tolerance: {end} ms")
            timestamps_valid = False
        if int(segment.get("speaker_id", -1)) <= 0:
            failures.append(f"segment {index} has illegal speaker_id={segment.get('speaker_id')!r}")
        speaker_id = int(segment.get("speaker_id", -1))
        parsed_previous_end = parsed_speaker_last_end.get(speaker_id, -1)
        parsed_overlap_ms = max(0, parsed_previous_end - start)
        maximum_observed_same_speaker_overlap_ms = max(
            maximum_observed_same_speaker_overlap_ms, parsed_overlap_ms
        )
        if parsed_overlap_ms > round(MAX_SAME_SPEAKER_OVERLAP_SECONDS * 1000):
            failures.append(
                f"segment {index} overlap exceeds the bound for speaker_id={speaker_id}"
            )
            timestamps_valid = False
        if end - start > round(MAX_ACTIVE_SECONDS_PER_SEGMENT * 1000):
            failures.append(
                f"segment {index} spans more than {MAX_ACTIVE_SECONDS_PER_SEGMENT:.0f}s"
            )
        previous_start = start
        previous_end = end
        parsed_speaker_last_end[speaker_id] = max(parsed_previous_end, end)

    for index, raw_turn in enumerate(raw_turns):
        if index >= len(segments):
            break
        segment = segments[index]
        raw_start_ms = round(float(raw_turn["start_seconds"]) * 1000)
        raw_end_ms = round(float(raw_turn["end_seconds"]) * 1000)
        if (
            abs(raw_start_ms - int(segment["t0_ms"])) > 20
            or abs(raw_end_ms - int(segment["t1_ms"])) > 20
            or int(raw_turn["speaker_id"]) != int(segment["speaker_id"])
        ):
            failures.append(f"raw turn {index} does not match parsed segment metadata")
        if normalize_transcript_text(str(raw_turn["text"])) != normalize_transcript_text(
            str(segment.get("text") or "")
        ):
            failures.append(f"raw turn {index} text differs from parsed segment text")

    speaker_previous_start = -1
    speaker_previous_end = -1
    diarized_speaker_last_end: dict[int, int] = {}
    for index, speaker_segment in enumerate(speaker_segments):
        start = int(speaker_segment["t0_ms"])
        end = int(speaker_segment["t1_ms"])
        speaker_id = int(speaker_segment.get("speaker_id", -1))
        if start < 0 or end < start or start < speaker_previous_start:
            failures.append(f"speaker segment {index} has invalid or non-monotonic timestamps")
        if end > round((duration_seconds + OUTPUT_END_TOLERANCE_SECONDS) * 1000):
            failures.append(f"speaker segment {index} ends after the audio tolerance")
        if speaker_id <= 0 or speaker_id > 99:
            failures.append(f"speaker segment {index} has illegal speaker_id={speaker_id}")
        diarized_previous_end = diarized_speaker_last_end.get(speaker_id, -1)
        diarized_overlap_ms = max(0, diarized_previous_end - start)
        maximum_observed_same_speaker_overlap_ms = max(
            maximum_observed_same_speaker_overlap_ms, diarized_overlap_ms
        )
        if diarized_overlap_ms > round(MAX_SAME_SPEAKER_OVERLAP_SECONDS * 1000):
            failures.append(
                f"speaker segment {index} overlap exceeds the bound for speaker_id={speaker_id}"
            )
        if index < len(segments):
            parsed = segments[index]
            if (
                start != int(parsed["t0_ms"])
                or end != int(parsed["t1_ms"])
                or speaker_id != int(parsed["speaker_id"])
            ):
                failures.append(f"speaker segment {index} does not match parsed segment")
        speaker_previous_start = start
        speaker_previous_end = end
        diarized_speaker_last_end[speaker_id] = max(diarized_previous_end, end)

    raw_joined_text = "".join(str(row["text"]) for row in raw_turns)
    segment_joined_text = "".join(str(row.get("text") or "") for row in segments)
    normalized_top_text = normalize_transcript_text(text)
    if normalize_transcript_text(raw_joined_text) != normalized_top_text:
        failures.append("strict raw turn text differs from top-level result text")
    if normalize_transcript_text(segment_joined_text) != normalized_top_text:
        failures.append("parsed segment text differs from top-level result text")

    significant_activity = activity.get("significant_active_intervals") or []
    if not significant_activity:
        failures.append("activity probe found no significant non-silent audio")
        first_activity_start = 0.0
        last_activity_end = float(activity.get("last_activity_end_seconds") or 0.0)
    else:
        first_activity_start = float(significant_activity[0]["start_seconds"])
        last_activity_end = float(significant_activity[-1]["end_seconds"])
    first_timestamp = (
        float(raw_turns[0]["start_seconds"])
        if raw_turns
        else min((int(row["t0_ms"]) for row in segments), default=0) / 1000.0
    )
    last_timestamp = float(output.get("last_timestamp_seconds") or 0.0)
    if first_timestamp > first_activity_start + OUTPUT_START_TOLERANCE_SECONDS:
        # ffmpeg silencedetect measures non-silence, not speech.  A startup
        # notification, table knock, or other isolated sound can therefore
        # precede the first transcribable turn.  Keep this visible in the
        # evidence, but do not mislabel it as a runtime failure.  Material
        # omissions remain guarded by activity coverage/max-gap below and are
        # finally scored against the P0-R human transcript.
        diagnostic_warnings.append(
            "first transcript timestamp misses the beginning of non-silent audio: "
            f"first_timestamp={first_timestamp:.3f}s, "
            f"first_activity_start={first_activity_start:.3f}s, "
            f"tolerance={OUTPUT_START_TOLERANCE_SECONDS:.3f}s"
        )
    if last_timestamp + OUTPUT_END_TOLERANCE_SECONDS < last_activity_end:
        failures.append(
            "last transcript timestamp does not reach the final non-silent audio: "
            f"last_timestamp={last_timestamp:.3f}s, "
            f"last_activity_end={last_activity_end:.3f}s, "
            f"tolerance={OUTPUT_END_TOLERANCE_SECONDS:.3f}s"
        )
    if last_timestamp > duration_seconds + OUTPUT_END_TOLERANCE_SECONDS:
        failures.append(
            f"last transcript timestamp exceeds audio duration: {last_timestamp:.3f}s > {duration_seconds:.3f}s"
        )

    transcript_intervals = [
        (float(row["start_seconds"]), float(row["end_seconds"]))
        for row in raw_turns
    ]
    activity_intervals = [
        (float(row["start_seconds"]), float(row["end_seconds"]))
        for row in significant_activity
    ]
    coverage = activity_coverage_metrics(transcript_intervals, activity_intervals)
    if coverage["activity_coverage_ratio"] < MIN_ACTIVITY_COVERAGE_RATIO:
        failures.append(
            "transcript covers too little non-silent audio: "
            f"coverage={coverage['activity_coverage_ratio']:.3f}, "
            f"minimum={MIN_ACTIVITY_COVERAGE_RATIO:.3f}"
        )
    if coverage["max_uncovered_activity_gap_seconds"] > MAX_UNCOVERED_ACTIVITY_GAP_SECONDS:
        failures.append(
            "transcript has an excessive uncovered non-silent gap: "
            f"gap={coverage['max_uncovered_activity_gap_seconds']:.3f}s, "
            f"maximum={MAX_UNCOVERED_ACTIVITY_GAP_SECONDS:.3f}s"
        )
    transcript_seconds = sum(
        end - start for start, end in merge_intervals(transcript_intervals)
    )
    transcript_characters = len(normalize_transcript_text(raw_joined_text))
    text_density = (
        transcript_characters / transcript_seconds if transcript_seconds > 0 else 0.0
    )
    if text_density < MIN_TEXT_CHARS_PER_TRANSCRIPT_SECOND:
        failures.append(
            "transcript text density is implausibly low: "
            f"density={text_density:.3f}, "
            f"minimum={MIN_TEXT_CHARS_PER_TRANSCRIPT_SECOND:.3f} characters/s"
        )
    minimum_segment_count = max(
        1, math.ceil(coverage["activity_seconds"] / MAX_ACTIVE_SECONDS_PER_SEGMENT)
    )
    if len(segments) < minimum_segment_count:
        failures.append(
            f"too few transcript segments for active audio: got {len(segments)}, "
            f"minimum {minimum_segment_count}"
        )

    speaker_marker_count = len(raw_turns)
    raw_timestamp_count = 2 * len(raw_turns)

    anchors = EXPECTED_SAMPLES[sample_name].get("known_audible_anchor_probes", [])
    missing_anchors = [anchor for anchor in anchors if anchor not in raw_joined_text]
    anchor_probe_status = (
        "NOT_APPLICABLE"
        if not anchors
        else ("PASS" if not missing_anchors else "FAIL")
    )

    verdict = {
        "status": "PASS" if not failures else "FAIL",
        "scope": "STRUCTURAL_RUNTIME_ONLY",
        "language_request": "zh",
        "native_reported_language": reported_language or "UNREPORTED",
        "language_accuracy_status": "NOT_SCORABLE_P0_R_BLOCKED",
        "known_anchor_probe_status": anchor_probe_status,
        "known_anchor_probe_is_not_full_accuracy_score": True,
        "checks": {
            "nonempty_text": bool(text and raw_text),
            "parsed_segments": len(segments),
            "speaker_segments": len(speaker_segments),
            "timestamps_monotonic_and_bounded": timestamps_valid,
            "first_timestamp_seconds": first_timestamp,
            "first_activity_start_seconds": first_activity_start,
            "start_tolerance_seconds": OUTPUT_START_TOLERANCE_SECONDS,
            "start_alignment_status": (
                "PASS" if not diagnostic_warnings else "DIAGNOSTIC_WARNING"
            ),
            "start_alignment_is_semantic_accuracy_gate": False,
            "last_timestamp_seconds": last_timestamp,
            "last_activity_end_seconds": last_activity_end,
            "end_tolerance_seconds": OUTPUT_END_TOLERANCE_SECONDS,
            "known_audible_anchor_probes": anchors,
            "missing_text_anchors": missing_anchors,
            "explicit_speaker_marker_count": speaker_marker_count,
            "explicit_timestamp_marker_count": raw_timestamp_count,
            "minimum_segment_count": minimum_segment_count,
            "activity_coverage": coverage,
            "transcript_characters": transcript_characters,
            "transcript_interval_seconds": transcript_seconds,
            "text_characters_per_transcript_second": text_density,
            "minimum_text_characters_per_transcript_second": MIN_TEXT_CHARS_PER_TRANSCRIPT_SECOND,
            "minimum_activity_coverage_ratio": MIN_ACTIVITY_COVERAGE_RATIO,
            "maximum_uncovered_activity_gap_seconds": MAX_UNCOVERED_ACTIVITY_GAP_SECONDS,
            "maximum_same_speaker_overlap_seconds": MAX_SAME_SPEAKER_OVERLAP_SECONDS,
            "maximum_observed_same_speaker_overlap_seconds": (
                maximum_observed_same_speaker_overlap_ms / 1000.0
            ),
        },
        "diagnostic_warnings": diagnostic_warnings,
        "failures": failures,
    }
    if failures:
        raise GateError("output validation failed: " + " | ".join(failures))
    return verdict


class MemoryMonitor:
    def __init__(self, interval_seconds: float = 0.2):
        try:
            import psutil  # type: ignore
        except ImportError as exc:
            raise GateError("psutil is required for the P1 memory gate") from exc
        self._psutil = psutil
        self._process = psutil.Process(os.getpid())
        self._interval = interval_seconds
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self.process_peak_rss_bytes = 0
        self.process_peak_vms_bytes = 0
        self.process_tree_peak_rss_bytes = 0
        self.system_min_available_bytes: int | None = None
        self.samples = 0

    def start(self) -> None:
        self._sample()
        self._thread.start()

    def _sample(self) -> None:
        try:
            memory = self._process.memory_info()
            children = self._process.children(recursive=True)
            tree_rss = memory.rss
            for child in children:
                try:
                    tree_rss += child.memory_info().rss
                except (self._psutil.NoSuchProcess, self._psutil.AccessDenied):
                    pass
            system_available = self._psutil.virtual_memory().available
            self.process_peak_rss_bytes = max(self.process_peak_rss_bytes, memory.rss)
            self.process_peak_vms_bytes = max(self.process_peak_vms_bytes, memory.vms)
            self.process_tree_peak_rss_bytes = max(
                self.process_tree_peak_rss_bytes, tree_rss
            )
            if self.system_min_available_bytes is None:
                self.system_min_available_bytes = system_available
            else:
                self.system_min_available_bytes = min(
                    self.system_min_available_bytes, system_available
                )
            self.samples += 1
        except (self._psutil.NoSuchProcess, self._psutil.AccessDenied):
            pass

    def _run(self) -> None:
        while not self._stop.wait(self._interval):
            self._sample()

    def stop(self) -> dict[str, Any]:
        self._stop.set()
        self._thread.join(timeout=max(1.0, self._interval * 4))
        self._sample()
        return {
            "sample_interval_seconds": self._interval,
            "sample_count": self.samples,
            "process_peak_rss_bytes": self.process_peak_rss_bytes,
            "process_peak_vms_bytes": self.process_peak_vms_bytes,
            "process_tree_peak_rss_bytes": self.process_tree_peak_rss_bytes,
            "system_min_available_bytes": self.system_min_available_bytes,
        }


def run_sample(args: argparse.Namespace) -> dict[str, Any]:
    started = time.perf_counter()
    run_sample._started_perf = started  # type: ignore[attr-defined]
    run_sample._last_memory = None  # type: ignore[attr-defined]
    context: dict[str, Any] = {
        "started_at": now_iso(),
        "sample_name": args.sample_name,
        "model_path": str(args.model),
        "sample_path": str(args.sample),
    }
    run_sample._last_context = context  # type: ignore[attr-defined]
    inference_environment = validate_inference_environment()
    context["inference_environment"] = inference_environment
    toolchain = validate_test_toolchain()
    context["test_toolchain"] = toolchain
    host = capture_and_validate_host()
    context["host"] = host
    module, runtime = load_transcribe_cpp()
    context["runtime"] = runtime
    selected_device, devices = select_exact_vulkan_device(module)
    context["devices"] = devices
    model_record = validate_model(args.model, args.model_bytes, args.model_sha256)
    context["model"] = model_record
    validate_sample_lock(args)
    source_record = validate_locked_file(
        args.sample, args.sample_bytes, args.sample_sha256, "frozen source sample"
    )
    context["source_sample"] = source_record

    monitor = MemoryMonitor()
    monitor.start()
    memory_record: dict[str, Any] | None = None
    try:
        prepared_record, conversion = prepare_audio(args.sample_name, source_record)
        context["prepared_audio"] = prepared_record
        context["conversion"] = conversion
        activity = probe_audio_activity(
            Path(prepared_record["path"]), float(prepared_record["duration_seconds"])
        )
        context["audio_activity"] = activity
        pcm, pcm_record = load_pcm_float32(Path(prepared_record["path"]))
        validate_pcm_lock(args.sample_name, pcm_record)
        context["pcm_load"] = pcm_record

        model_load_started = time.perf_counter()
        with module.Model(
            model_record["path"], backend="vulkan", device=selected_device
        ) as model:
            model_load_seconds = time.perf_counter() - model_load_started
            resolved_device = model.device
            backend_name = str(model.backend)
            allowed_backend_names = {
                EXPECTED_DEVICE_KIND.casefold(),
                str(selected_device.name).casefold(),
            }
            if (
                resolved_device != selected_device
                or resolved_device.kind != EXPECTED_DEVICE_KIND
                or resolved_device.description != EXPECTED_DEVICE_DESCRIPTION
                or backend_name.casefold() not in allowed_backend_names
            ):
                raise GateError(
                    "model did not stay on the exact Intel Arc Vulkan device: "
                    f"backend={model.backend!r}, device={device_record(resolved_device)!r}"
                )

            capabilities = dataclasses.asdict(model.capabilities)
            model_identity = {
                "arch": model.arch,
                "variant": model.variant,
                "backend": backend_name,
                "device": device_record(resolved_device),
                "capabilities": capabilities,
            }
            context["model_identity"] = model_identity
            loaded_native_modules = capture_loaded_native_modules(phase="inference")
            context["loaded_native_modules"] = loaded_native_modules
            with model.session(n_threads=0, kv_type="auto", n_ctx=0) as session:
                limits = dataclasses.asdict(session.limits)
                context["session_limits"] = limits
                duration_ms = round(prepared_record["duration_seconds"] * 1000)
                maximum_ms = limits["effective_max_audio_ms"]
                if maximum_ms and duration_ms > maximum_ms:
                    raise GateError(
                        f"prepared audio is longer than the session limit: "
                        f"audio={duration_ms}ms, limit={maximum_ms}ms"
                    )
                inference_started = time.perf_counter()
                context["model_load_seconds"] = model_load_seconds
                context["inference_started_at"] = now_iso()
                result = session.run(
                    pcm,
                    language="zh",
                    timestamps="segment",
                    diarize="on",
                )
                inference_seconds = time.perf_counter() - inference_started
                context["inference_seconds"] = inference_seconds

        output = result_record(result)
        duration_seconds = float(prepared_record["duration_seconds"])
        context["output"] = output
        output_gate = validate_output(
            args.sample_name, output, duration_seconds, activity
        )
        context["output_gate"] = output_gate
        inference_rtf = inference_seconds / duration_seconds
        post_run_integrity = {
            "model": validate_locked_file(
                args.model, EXPECTED_MODEL_BYTES, EXPECTED_MODEL_SHA256, "model after inference"
            ),
            "source_sample": validate_locked_file(
                args.sample,
                args.sample_bytes,
                args.sample_sha256,
                "frozen source sample after inference",
            ),
            "prepared_audio": validate_locked_file(
                Path(prepared_record["path"]),
                int(prepared_record["bytes"]),
                str(prepared_record["sha256"]),
                "prepared audio after inference",
            ),
            "loaded_native_modules": capture_loaded_native_modules(phase="inference"),
            "runtime_and_binding": validate_frozen_files(),
            "test_toolchain": validate_test_toolchain(),
        }
        controlled_runner_elapsed_seconds = time.perf_counter() - started
        controlled_runner_rtf = controlled_runner_elapsed_seconds / duration_seconds
        max_rtf = validate_rtf(args.sample_name, controlled_runner_rtf)
        return {
            "schema_version": 1,
            "status": "STRUCTURAL_RUNTIME_PASS",
            "run_status": "COMPLETED",
            "accuracy_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "simplified_chinese_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "semantic_completeness_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "diarization_accuracy_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "p1_verdict": "NOT_EVALUATED_P0_R_BLOCKED",
            "release_go": False,
            "completeness_scope": (
                "structural timestamp/activity coverage only; semantic omissions require frozen human truth"
            ),
            "captured_at": now_iso(),
            "gate": "MOSS_V3_P1_INTEL_ARC_VULKAN_SINGLE_SAMPLE",
            "execution_backend": {
                "selected_primary_device": "Intel Arc Vulkan",
                "selection_verified": True,
                "primary_device_fallback_allowed": False,
                "scheduler_cpu_fallback_present": True,
                "cpu_preprocessing_present": True,
                "gpu_only_execution_proven": False,
            },
            "runtime": runtime,
            "inference_environment": inference_environment,
            "test_toolchain": toolchain,
            "host": host,
            "devices": devices,
            "model": model_record,
            "source_sample": source_record,
            "prepared_audio": prepared_record,
            "conversion": conversion,
            "audio_activity": activity,
            "pcm_load": pcm_record,
            "model_identity": model_identity,
            "loaded_native_modules": loaded_native_modules,
            "session_limits": limits,
            "run_options": {
                "language": "zh",
                "timestamps": "segment",
                "diarize": "on",
                "n_threads": 0,
                "kv_type": "auto",
                "n_ctx": 0,
                "exact_device": {
                    "kind": EXPECTED_DEVICE_KIND,
                    "description": EXPECTED_DEVICE_DESCRIPTION,
                    "device_id": EXPECTED_DEVICE_ID,
                    "device_type": EXPECTED_DEVICE_TYPE,
                },
            },
            "model_load_seconds": model_load_seconds,
            "inference_seconds": inference_seconds,
            "audio_duration_seconds": duration_seconds,
            "inference_real_time_factor": inference_rtf,
            "controlled_runner_elapsed_seconds": controlled_runner_elapsed_seconds,
            "controlled_runner_real_time_factor": controlled_runner_rtf,
            "real_time_factor": controlled_runner_rtf,
            "performance_gate_basis": "controlled_runner_real_time_factor",
            "performance_status": "PASS" if max_rtf is not None else "NOT_GATED",
            "maximum_allowed_rtf": max_rtf,
            "output": output,
            "output_gate": output_gate,
            "post_run_integrity": post_run_integrity,
            "total_elapsed_seconds": controlled_runner_elapsed_seconds,
        }
    finally:
        memory_record = monitor.stop()
        # Attach memory on success through the returned object.  On failure the
        # caller receives this value from the monitor snapshot kept below.
        run_sample._last_memory = memory_record  # type: ignore[attr-defined]


def partial_result_record(exc: BaseException) -> dict[str, Any] | None:
    partial = getattr(exc, "partial_result", None)
    if partial is None:
        return None
    try:
        return result_record(partial)
    except Exception as serialization_error:  # pragma: no cover - diagnostic fallback
        return {
            "text": str(getattr(partial, "text", "")),
            "raw_text": str(getattr(partial, "raw_text", "")),
            "language": str(getattr(partial, "language", "")),
            "timestamp_kind": str(getattr(partial, "timestamp_kind", "")),
            "serialization_error": {
                "type": type(serialization_error).__name__,
                "message": str(serialization_error),
            }
        }


def ensure_evidence_directory(path: Path, expected_commit: str) -> Path:
    PUBLIC_EVIDENCE_ROOT.mkdir(parents=True, exist_ok=True)
    enforce_and_audit_restricted_acl(
        PUBLIC_EVIDENCE_ROOT, "public evidence root", PUBLIC_EVIDENCE_ROOT
    )
    resolved = path.resolve()
    require_under(resolved, PUBLIC_EVIDENCE_ROOT, "public evidence directory")
    if resolved.parent != PUBLIC_EVIDENCE_ROOT:
        raise GateError("public evidence directory must be a direct child of the fixed evidence root")
    match = re.fullmatch(
        r"MOSS-V3-P1-(?:FINAL|DEV|RUN)-([0-9A-Fa-f]{7,40})(?:-[0-9]{8})?",
        resolved.name,
    )
    if match is None:
        raise GateError(
            "public evidence directory must use the fixed "
            "MOSS-V3-P1-(FINAL|DEV|RUN)-<git-commit>[-YYYYMMDD] format"
        )
    commit_token = match.group(1).lower()
    normalized_commit = expected_commit.lower()
    if len(normalized_commit) != 40 or re.fullmatch(r"[0-9a-f]{40}", normalized_commit) is None:
        raise GateError("validated Git commit must be a full 40-character hexadecimal identity")
    if not normalized_commit.startswith(commit_token):
        raise GateError("public evidence directory commit does not match the validated HEAD")
    resolved.mkdir(parents=True, exist_ok=True)
    resolved_after_create = path.resolve()
    require_under(resolved_after_create, PUBLIC_EVIDENCE_ROOT, "public evidence directory")
    if resolved_after_create.parent != PUBLIC_EVIDENCE_ROOT:
        raise GateError("public evidence directory changed identity during creation")
    if not resolved_after_create.is_dir() or (
        hasattr(path, "is_junction") and path.is_junction()
    ) or path.is_symlink():
        raise GateError(f"evidence path is not a plain directory: {resolved_after_create}")
    enforce_and_audit_restricted_acl(
        resolved_after_create, "public evidence directory", PUBLIC_EVIDENCE_ROOT
    )
    return resolved_after_create


def audit_restricted_acl(
    target: Path, label: str, expected_root: Path
) -> dict[str, Any]:
    """Read and verify a protected three-principal directory ACL."""

    resolved_target = target.resolve()
    require_under(resolved_target, expected_root.resolve(), label)
    script = r"""
$ErrorActionPreference = 'Stop'
$target = $env:P1_ACL_TARGET
if ([string]::IsNullOrWhiteSpace($target)) { throw 'P1_ACL_TARGET is missing' }
$current = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$check = Get-Acl -LiteralPath $target
$entries = @($check.Access | ForEach-Object {
  [pscustomobject]@{
    sid = $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
    rights = [int64]$_.FileSystemRights
    type = $_.AccessControlType.ToString()
    inherited = $_.IsInherited
    inheritance_flags = $_.InheritanceFlags.ToString()
    propagation_flags = $_.PropagationFlags.ToString()
  }
})
[pscustomobject]@{
  protected = $check.AreAccessRulesProtected
  current_sid = $current.Value
  entries = $entries
} | ConvertTo-Json -Depth 4 -Compress
"""
    completed = subprocess.run(
        [
            str(EXPECTED_POWERSHELL),
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        env={
            "SystemRoot": r"C:\Windows",
            "WINDIR": r"C:\Windows",
            "COMSPEC": r"C:\Windows\System32\cmd.exe",
            "PATH": r"C:\Windows\System32;C:\Windows",
            "PSModulePath": r"C:\Windows\System32\WindowsPowerShell\v1.0\Modules",
            "P1_ACL_TARGET": str(resolved_target),
        },
    )
    if completed.returncode != 0:
        raise GateError(f"failed to audit {label} ACL")
    try:
        audit = json.loads(completed.stdout.strip())
    except json.JSONDecodeError as exc:
        raise GateError(f"{label} ACL audit was not valid JSON") from exc
    allowed = {"S-1-5-18", "S-1-5-32-544", str(audit.get("current_sid"))}
    entries = audit.get("entries") or []
    observed = {str(entry.get("sid")) for entry in entries}
    if (
        audit.get("protected") is not True
        or observed != allowed
        or any(
            entry.get("type") != "Allow"
            or entry.get("inherited") is not False
            or int(entry.get("rights", 0)) & 0x1F01FF != 0x1F01FF
            or "ContainerInherit" not in str(entry.get("inheritance_flags"))
            or "ObjectInherit" not in str(entry.get("inheritance_flags"))
            or str(entry.get("propagation_flags")) != "None"
            for entry in entries
        )
    ):
        raise GateError(f"{label} ACL verification failed")
    return audit


def enforce_and_audit_restricted_acl(
    target: Path, label: str, expected_root: Path
) -> dict[str, Any]:
    """Replace inherited ACLs with three explicit full-control principals.

    This is intentionally executed only after ``validate_test_toolchain`` has
    authenticated the fixed PowerShell binary.  Both inheritance from the
    parent and previously inherited entries are removed, so a later parent
    ACL change cannot expose an already-written transcript.
    """

    powershell = EXPECTED_POWERSHELL.resolve()
    if not powershell.is_file():
        raise GateError("system PowerShell executable is unavailable for ACL enforcement")
    resolved_target = target.resolve()
    require_under(resolved_target, expected_root.resolve(), label)
    script = r"""
$ErrorActionPreference = 'Stop'
$target = $env:P1_ACL_TARGET
if ([string]::IsNullOrWhiteSpace($target)) { throw 'P1_ACL_TARGET is missing' }
$current = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$check = Get-Acl -LiteralPath $target
$entries = @($check.Access | ForEach-Object {
  [pscustomobject]@{
    sid = $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
    rights = [int64]$_.FileSystemRights
    type = $_.AccessControlType.ToString()
    inherited = $_.IsInherited
    inheritance_flags = $_.InheritanceFlags.ToString()
    propagation_flags = $_.PropagationFlags.ToString()
  }
})
[pscustomobject]@{
  protected = $check.AreAccessRulesProtected
  current_sid = $current.Value
  entries = $entries
} | ConvertTo-Json -Depth 4 -Compress
"""
    audit_environment = {
        "SystemRoot": r"C:\Windows",
        "WINDIR": r"C:\Windows",
        "COMSPEC": r"C:\Windows\System32\cmd.exe",
        "PATH": r"C:\Windows\System32;C:\Windows",
        "PSModulePath": r"C:\Windows\System32\WindowsPowerShell\v1.0\Modules",
        "P1_ACL_TARGET": str(resolved_target),
    }

    def audit_acl() -> dict[str, Any]:
        completed = subprocess.run(
            [
                str(powershell),
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            env=audit_environment,
        )
        if completed.returncode != 0:
            raise GateError(f"failed to audit {label} ACL")
        try:
            return json.loads(completed.stdout.strip())
        except json.JSONDecodeError as exc:
            raise GateError(f"{label} ACL audit was not valid JSON") from exc

    before = audit_acl()
    current_sid = str(before.get("current_sid"))
    allowed = {"S-1-5-18", "S-1-5-32-544", current_sid}
    before_entries = before.get("entries") or []
    unauthorized_explicit = sorted(
        {
            str(entry.get("sid"))
            for entry in before_entries
            if str(entry.get("sid")) not in allowed
            and entry.get("inherited") is False
        }
    )
    if unauthorized_explicit:
        raise GateError(
            f"{label} contains an unexpected explicit ACL principal; refusing automatic repair"
        )
    icacls_command = [
        str(EXPECTED_ICACLS),
        str(resolved_target),
        "/inheritance:r",
        "/grant:r",
        f"*{current_sid}:(OI)(CI)F",
        "*S-1-5-18:(OI)(CI)F",
        "*S-1-5-32-544:(OI)(CI)F",
    ]
    acl_update = subprocess.run(
        icacls_command,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
        env={
            "SystemRoot": r"C:\Windows",
            "WINDIR": r"C:\Windows",
            "COMSPEC": r"C:\Windows\System32\cmd.exe",
            "PATH": r"C:\Windows\System32;C:\Windows",
        },
    )
    if acl_update.returncode != 0:
        raise GateError(f"failed to set {label} ACL")

    audit = audit_acl()
    entries = audit.get("entries") or []
    observed = {str(entry.get("sid")) for entry in entries}
    if (
        audit.get("protected") is not True
        or observed != allowed
        or any(
            entry.get("type") != "Allow"
            or entry.get("inherited") is not False
            or int(entry.get("rights", 0)) & 0x1F01FF != 0x1F01FF
            or "ContainerInherit" not in str(entry.get("inheritance_flags"))
            or "ObjectInherit" not in str(entry.get("inheritance_flags"))
            or str(entry.get("propagation_flags")) != "None"
            for entry in entries
        )
    ):
        raise GateError(f"{label} ACL verification failed")
    resolved_after_acl = target.resolve()
    require_under(resolved_after_acl, expected_root.resolve(), label)
    if resolved_after_acl != resolved_target:
        raise GateError(f"{label} changed identity during ACL enforcement")
    return audit


def ensure_private_evidence_directory(public_directory: Path) -> Path:
    PRIVATE_EVIDENCE_ROOT.mkdir(parents=True, exist_ok=True)
    enforce_and_audit_restricted_acl(
        PRIVATE_EVIDENCE_ROOT, "private evidence root", PRIVATE_EVIDENCE_ROOT
    )
    resolved = (PRIVATE_EVIDENCE_ROOT / public_directory.name).resolve()
    require_under(resolved, PRIVATE_EVIDENCE_ROOT, "private evidence directory")
    resolved.mkdir(parents=True, exist_ok=True)
    resolved_after_create = (PRIVATE_EVIDENCE_ROOT / public_directory.name).resolve()
    require_under(resolved_after_create, PRIVATE_EVIDENCE_ROOT, "private evidence directory")
    if resolved_after_create.parent != PRIVATE_EVIDENCE_ROOT:
        raise GateError("private evidence directory changed identity during creation")
    if (
        hasattr(PRIVATE_EVIDENCE_ROOT / public_directory.name, "is_junction")
        and (PRIVATE_EVIDENCE_ROOT / public_directory.name).is_junction()
    ) or (PRIVATE_EVIDENCE_ROOT / public_directory.name).is_symlink():
        raise GateError("private evidence path is not a plain directory")
    enforce_and_audit_restricted_acl(
        resolved_after_create, "private evidence directory", PRIVATE_EVIDENCE_ROOT
    )
    return resolved_after_create


def atomic_write_text_exclusive(path: Path, content: str) -> None:
    if path.exists():
        raise GateError(f"refusing to overwrite evidence file: {path.name}")
    partial = path.with_name(f".{path.name}.{os.getpid()}.{time.time_ns()}.partial")
    try:
        with partial.open("x", encoding="utf-8", newline="\n") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        partial.rename(path)
    finally:
        if partial.exists():
            partial.unlink()


def write_evidence_atomic(path: Path, payload: dict[str, Any]) -> str:
    serialized = json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    json.loads(serialized)
    atomic_write_text_exclusive(path, serialized)
    digest = sha256(path)
    atomic_write_text_exclusive(path.with_suffix(path.suffix + ".sha256"), f"{digest}  {path.name}\n")
    return digest


def read_and_verify_sha256_sidecar(path: Path) -> str:
    sidecar = path.with_suffix(path.suffix + ".sha256")
    if not sidecar.is_file():
        raise GateError(f"evidence sidecar is missing: {sidecar.name}")
    content = sidecar.read_text(encoding="utf-8")
    match = re.fullmatch(r"([0-9A-F]{64})  ([^\r\n]+)\n", content)
    if match is None or match.group(2) != path.name:
        raise GateError(f"evidence sidecar is malformed: {sidecar.name}")
    actual = sha256(path)
    if actual != match.group(1):
        raise GateError(f"evidence sidecar hash mismatch: {sidecar.name}")
    return actual


def verify_evidence_bundle(
    public_directory: Path,
    private_directory: Path,
    public_path: Path,
    private_path: Path,
    attestation_path: Path,
) -> dict[str, str]:
    """Verify the complete JSON + sidecar + restricted attestation bundle."""

    public_directory = public_directory.resolve()
    private_directory = private_directory.resolve()
    public_path = public_path.resolve()
    private_path = private_path.resolve()
    attestation_path = attestation_path.resolve()
    if public_path.parent != public_directory or private_path.parent != private_directory:
        raise GateError("evidence bundle file is outside its bound directory")
    if public_directory.parent != PUBLIC_EVIDENCE_ROOT:
        raise GateError("public evidence bundle is outside the fixed public evidence root")
    if private_directory != (PRIVATE_EVIDENCE_ROOT / public_directory.name).resolve():
        raise GateError("private evidence bundle directory is not bound to the public run")
    audit_restricted_acl(
        public_directory, "public evidence bundle directory", PUBLIC_EVIDENCE_ROOT
    )
    audit_restricted_acl(
        private_directory, "private evidence bundle directory", PRIVATE_EVIDENCE_ROOT
    )
    if attestation_path.parent != private_directory:
        raise GateError("evidence attestation is outside the restricted directory")
    if private_path.name != f"{public_path.stem}.private.json":
        raise GateError("public and private evidence stems do not match")
    if attestation_path.name != f"{public_path.stem}.public-attestation.json":
        raise GateError("public evidence and attestation stems do not match")

    public_payload = read_json(public_path, "public evidence JSON")
    read_json(private_path, "private evidence JSON")
    attestation = read_json(attestation_path, "restricted public evidence attestation")
    assert_public_evidence_safe(public_payload)
    public_hash = read_and_verify_sha256_sidecar(public_path)
    private_hash = read_and_verify_sha256_sidecar(private_path)
    attestation_hash = read_and_verify_sha256_sidecar(attestation_path)
    private_reference = public_payload.get("private_evidence")
    expected_private_reference = {
        "bytes": private_path.stat().st_size,
        "sha256": private_hash,
        "restricted": True,
    }
    if (
        public_payload.get("restricted_attestation_required") is not True
        or private_reference != expected_private_reference
    ):
        raise GateError("public evidence does not bind the restricted private evidence")
    expected_attestation = {
        "schema_version": 1,
        "stage": "MOSS_V3_P1_PUBLIC_EVIDENCE_ATTESTATION",
        "captured_at": attestation.get("captured_at"),
        "public_file_name": public_path.name,
        "public_sha256": public_hash,
        "private_file_name": private_path.name,
        "private_sha256": private_hash,
        "acceptance_rule": "JSON_PLUS_SHA256_PLUS_RESTRICTED_ATTESTATION_REQUIRED",
    }
    if (
        attestation != expected_attestation
        or not isinstance(attestation.get("captured_at"), str)
        or ISO_TIMESTAMP_RE.fullmatch(str(attestation.get("captured_at"))) is None
    ):
        raise GateError("restricted public evidence attestation content mismatch")
    return {
        "public_sha256": public_hash,
        "private_sha256": private_hash,
        "attestation_sha256": attestation_hash,
    }


def write_public_attestation_atomic(
    private_directory: Path,
    public_path: Path,
    public_sha256: str,
    private_path: Path,
    private_sha256: str,
) -> Path:
    """Write an independent, restricted binding for the public/private pair.

    A public JSON and its adjacent SHA file are not accepted as a complete
    evidence set on their own.  This third record lives in the separately
    protected private directory and binds both immutable file identities.
    """

    actual_public_hash = read_and_verify_sha256_sidecar(public_path)
    actual_private_hash = read_and_verify_sha256_sidecar(private_path)
    if actual_public_hash != require_sha256(public_sha256, "public evidence SHA-256"):
        raise GateError("caller-provided public evidence hash does not match the file")
    if actual_private_hash != require_sha256(private_sha256, "private evidence SHA-256"):
        raise GateError("caller-provided private evidence hash does not match the file")
    attestation = private_directory / f"{public_path.stem}.public-attestation.json"
    write_evidence_atomic(
        attestation,
        {
            "schema_version": 1,
            "stage": "MOSS_V3_P1_PUBLIC_EVIDENCE_ATTESTATION",
            "captured_at": now_iso(),
            "public_file_name": public_path.name,
            "public_sha256": actual_public_hash,
            "private_file_name": private_path.name,
            "private_sha256": actual_private_hash,
            "acceptance_rule": "JSON_PLUS_SHA256_PLUS_RESTRICTED_ATTESTATION_REQUIRED",
        },
    )
    verify_evidence_bundle(
        public_path.parent,
        private_directory,
        public_path,
        private_path,
        attestation,
    )
    return attestation


def content_metadata(value: str) -> dict[str, Any]:
    encoded = value.encode("utf-8")
    return {
        "utf8_bytes": len(encoded),
        "characters": len(value),
    }


OMIT_PUBLIC_KEYS = {
    "command",
    "stderr",
    "stdout",
    "traceback",
    "message",
    "required_text_anchors",
    "known_audible_anchor_probes",
    "missing_text_anchors",
    "failures",
    "raw_turns",
    "segments",
    "speaker_segments",
    "global_turns",
    "active_intervals",
    "significant_active_intervals",
    "silence_intervals",
}
PATH_PUBLIC_KEYS = {
    "path",
    "source_root",
    "binding_root",
    "runtime_dir",
    "module_path",
    "library_path",
    "model_path",
    "sample_path",
    "evidence_file",
    "output_path",
}
CONTENT_PUBLIC_KEYS = {"text", "raw_text", "message"}
WINDOWS_ABSOLUTE_PATH_RE = re.compile(r"(?i)(?:[A-Z]:[\\/]|\\\\)")
SAFE_PUBLIC_LITERAL_VALUES = {
    "PASS",
    "FAIL",
    "PREFLIGHT_PASS",
    "COMPLETED",
    "NOT_RUN",
    "NOT_EVALUATED",
    "NOT_SCORABLE_P0_R_BLOCKED",
    "NOT_EVALUATED_P0_R_BLOCKED",
    "STRUCTURAL_RUNTIME_PASS",
    "STRUCTURAL_RUNTIME_ONLY",
    "structural timestamp/activity coverage only; semantic omissions require frozen human truth",
    "MOSS_V3_P1_INTEL_ARC_VULKAN_SINGLE_SAMPLE",
    "device-only",
    "Intel Arc Vulkan",
    "vulkan",
    "cpu",
    "segment",
    "strict_raw_turn_end",
    "pcm_s16le",
    "Q8_0",
    "zh",
    "en",
    "clean_for_all_p1_gate_sources",
    "reject_behavior_changing_environment_variables",
    "psutil_process_memory_maps",
}
ISO_TIMESTAMP_RE = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}"
    r"(?:\.\d{1,6})?(?:Z|[+-]\d{2}:\d{2})$"
)
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$")

# Public evidence is a fixed diagnostic DTO.  Unknown keys are fingerprinted
# even when they are ASCII, because participant names, client identifiers and
# business terms can all look like harmless dictionary keys.
SAFE_PUBLIC_SCHEMA_KEYS = frozenset(
    """
    accuracy_status activity_coverage activity_coverage_ratio activity_seconds
    admitted api_version approved_toolchain_manifest arch argument_count
    audio_activity audio_duration_seconds available_bytes available_gib backend
    backends base_executable base_runtime battery_percent battery_status
    binding_manifest binding_root bitexact_flags bounded_chunk_child build bytes
    capabilities captured_at channels characters checked_names checked_prefixes
    checks child child_execution_status child_exit_code chunk_count
    chunk_execution_status chunk_inference_verdict chunk_plan chunk_seconds
    chunked_runner chunked_tests chunks code codec controlled_runner_real_time_factor
    controlled_runner_elapsed_seconds command_argument_count command_fingerprint command_sha256
    completeness_scope contains_sensitive_diagnostics context_before_error contract
    conversion covered_activity_seconds cpu cpu_preprocessing_present created_at
    decode_ms description device device_id device_type devices diagnostic_line_count
    diarization_accuracy_status diarize driver duration_seconds effective_budget_bytes
    effective_budget_gib effective_max_audio_ms effective_n_ctx elapsed_seconds
    encode_ms end_seconds end_tolerance_seconds error estimate estimate_kind
    evidence_file exact_device exact_match_count executable_name execution_backend
    execution_status explicit_speaker_marker_count explicit_timestamp_marker_count
    ffmpeg file_count file_name file_names file_role files fingerprint_sha256
    first_activity_start_seconds first_timestamp_seconds float32_bytes float32_sha256
    format frames gate git git_commit git_status gpu gpu_only_execution_proven
    header_hash host index inference inference_environment inference_real_time_factor
    inference_seconds inference_started_at inference_verdict kind kv_bytes kv_gib
    kv_type lane language language_accuracy_status language_request languages
    last_activity_end_seconds last_timestamp_seconds last_timestamp_source
    library_path library_sha256 load_ms loaded loaded_native_modules lock
    manifest_sha256 max_audio_ms max_kv_bytes max_timestamp_kind
    max_uncovered_activity_gap_seconds maximum_allowed_rtf
    maximum_same_speaker_overlap_seconds maximum_observed_same_speaker_overlap_seconds
    maximum_uncovered_activity_gap_seconds mel_ms memory memory_admission_duration_seconds
    memory_estimate memory_free memory_total metadata_removed method
    minimum_activity_coverage_ratio minimum_activity_interval_seconds
    minimum_available_memory_bytes minimum_available_memory_gib minimum_segment_count
    minimum_silence_seconds minimum_text_characters_per_transcript_second mode model
    model_identity model_load model_load_seconds model_path model_resident_bytes
    module_path module_sha256 modules monolithic_memory_admission n_ctx n_threads name
    name_characters native_commit native_file native_provider native_reported_language
    native_sample_rate native_version nonempty_text observed_process_count os output
    output_gate overlap_before_seconds overlap_seconds p1_inference_verdict p1_verdict
    package_manifest parsed_segments partial_output path pcm_load peak_tree_rss_bytes
    peak_tree_rss_gib performance_gate_basis performance_status
    plan_is_not_inference_evidence policy post_run_integrity powershell prepared_audio
    prepared_minus_source_timeline_seconds present_names primary_device_fallback_allowed
    private_evidence private_evidence_file process_peak_rss_bytes process_peak_vms_bytes
    process_tree_peak_rss_bytes psutil python python_dll python_file pyvenv_cfg
    quantization raw_text raw_turn_count real_time_factor reason_code reasons
    registered_devices release_go requested_primary_device required_bytes required_gib
    required_module_count residual_process_check residual_processes_after_cleanup
    residual_processes_before_cleanup return_code run_options run_status runner runtime
    runtime_and_binding runtime_dir runtime_reserve_bytes safety_margin_bytes
    sample_count sample_interval_seconds sample_name sample_path sample_rate_hz
    sample_width_bytes samples scheduler_cpu_fallback_present schema_version scope
    silence_detection_minimum_seconds output_end_tolerance_seconds
    segment_count selected selected_primary_device selection_rule selection_verified
    semantic_completeness_status session_limits sha256 simplified_chinese_status source
    source_commit source_duration_seconds source_pcm_s16le_sha256 source_root
    source_sample source_status source_timeline speaker_ids speaker_segment_count
    start_seconds start_tolerance_seconds started_at state status storage storage_scope
    suffix supervision supervisor supervisor_tests supports_language_detect
    supports_spec_decode supports_streaming supports_translate system_min_available_bytes
    termination_reason test_toolchain tests text text_characters_per_transcript_second
    threshold_db timeline_duration_seconds timeline_normalization timestamp_kind
    timestamps timestamps_monotonic_and_bounded timings total_elapsed_seconds
    trailing_silence transcript_characters transcript_interval_seconds
    translate_target_languages type utf8_bytes validated_single_process_budget_bytes
    variant vendor venv_scripts version vulkan_loader known_anchor_probe_status restricted
    restricted_attestation_required
    known_anchor_probe_is_not_full_accuracy_score native_language_metadata_status
    """.split()
)


def is_safe_public_literal(value: str, key: str | None) -> bool:
    return bool(
        value in SAFE_PUBLIC_LITERAL_VALUES
        or value in EXPECTED_SAMPLES
        or SHA256_RE.fullmatch(value) is not None
        or (
            key in {"captured_at", "started_at", "inference_started_at"}
            and ISO_TIMESTAMP_RE.fullmatch(value)
        )
        or (
            key in {"git_commit", "source_commit"}
            and re.fullmatch(r"[0-9A-Fa-f]{7,40}", value)
        )
        or (key in {"version", "api_version"} and SEMVER_RE.fullmatch(value))
        or (key == "code" and re.fullmatch(r"P1_[A-Z0-9_]+", value))
        or (key == "suffix" and re.fullmatch(r"(?:\.[a-z0-9]{1,12})?", value))
    )


def make_public_evidence(value: Any, key: str | None = None) -> Any:
    if key in OMIT_PUBLIC_KEYS:
        return None
    if key in CONTENT_PUBLIC_KEYS and isinstance(value, str):
        return content_metadata(value)
    if key in PATH_PUBLIC_KEYS and isinstance(value, str):
        path = Path(value)
        return {
            "suffix": path.suffix.lower(),
            "name_characters": len(path.name),
        }
    if isinstance(value, str) and WINDOWS_ABSOLUTE_PATH_RE.search(value):
        return content_metadata(value)
    if isinstance(value, str):
        if is_safe_public_literal(value, key):
            return value
        return content_metadata(value)
    if isinstance(value, dict):
        public: dict[str, Any] = {}
        for child_key, child_value in value.items():
            if child_key in OMIT_PUBLIC_KEYS:
                continue
            public_key = str(child_key)
            if key in {"participant_map", "speaker_name_map", "term_map", "aliases"}:
                continue
            if public_key not in SAFE_PUBLIC_SCHEMA_KEYS:
                continue
            public[public_key] = make_public_evidence(child_value, str(child_key))
        return public
    if isinstance(value, (list, tuple)):
        return [make_public_evidence(item) for item in value]
    return value


def assert_public_evidence_safe(value: Any) -> None:
    rendered = json.dumps(value, ensure_ascii=False, allow_nan=False)
    if WINDOWS_ABSOLUTE_PATH_RE.search(rendered):
        raise GateError("public evidence still contains an absolute Windows path")

    def validate(item: Any, parent_key: str | None = None) -> None:
        if isinstance(item, dict):
            for child_key, child_value in item.items():
                if child_key not in SAFE_PUBLIC_SCHEMA_KEYS:
                    raise GateError("public evidence is not a complete fixed-schema DTO")
                validate(child_value, child_key)
            return
        if isinstance(item, list):
            for child in item:
                validate(child, parent_key)
            return
        if isinstance(item, str) and not is_safe_public_literal(item, parent_key):
            raise GateError("public evidence contains a non-schema string literal")
        if not isinstance(item, (str, int, float, bool, type(None))):
            raise GateError("public evidence contains an unsupported DTO value type")

    validate(value)


def stable_error_code(exc: BaseException) -> str:
    name = type(exc).__name__.upper()
    if isinstance(exc, GateError):
        return "P1_GATE_REJECTED"
    if "OUTOFMEMORY" in name or "OUT_OF_MEMORY" in name:
        return "P1_NATIVE_OUT_OF_MEMORY"
    if "TRUNCAT" in name:
        return "P1_NATIVE_OUTPUT_TRUNCATED"
    if isinstance(exc, KeyboardInterrupt):
        return "P1_CANCELLED"
    return "P1_UNEXPECTED_ERROR"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run one frozen MOSS Q8_0 sample on the exact Intel Arc Vulkan device. "
            "Use --device-only for a no-model runtime/device check."
        )
    )
    parser.add_argument(
        "--device-only",
        action="store_true",
        help="load only the frozen ctypes/DLL runtime, list devices, and exit",
    )
    parser.add_argument("--model", type=Path, help="fixed Q8_0 GGUF path under D:\\MeetilyData\\staging\\moss-p1")
    parser.add_argument("--model-bytes", type=int, help="expected GGUF byte count")
    parser.add_argument("--model-sha256", help="expected GGUF SHA-256")
    parser.add_argument("--sample", type=Path, help="one frozen source WAV/MP4/M4A")
    parser.add_argument("--sample-bytes", type=int, help="expected source byte count")
    parser.add_argument("--sample-sha256", help="expected source SHA-256")
    parser.add_argument(
        "--sample-name",
        help="stable ASCII evidence name, for example short_12s or business_737s",
    )
    parser.add_argument("--evidence", type=Path, help="new or existing evidence directory on D:")
    return parser


def validate_run_arguments(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    required = (
        "model",
        "model_bytes",
        "model_sha256",
        "sample",
        "sample_bytes",
        "sample_sha256",
        "sample_name",
        "evidence",
    )
    missing = [name for name in required if getattr(args, name) is None]
    if missing:
        parser.error("single-sample mode requires: " + ", ".join(f"--{name.replace('_', '-')}" for name in missing))
    if not SAFE_NAME_RE.fullmatch(args.sample_name):
        parser.error("--sample-name must match [A-Za-z0-9][A-Za-z0-9_-]{0,79}")
    if args.sample_name not in EXPECTED_SAMPLES:
        parser.error("--sample-name must be one of the frozen EXPECTED_SAMPLES names")
    if args.sample_name == "long_3096s":
        parser.error(
            "long_3096s monolithic execution is forbidden by the memory gate; "
            "use moss_v3_p1_chunked.py under the external supervisor"
        )


def device_only_payload() -> dict[str, Any]:
    inference_environment = validate_inference_environment()
    toolchain = validate_test_toolchain()
    host = capture_and_validate_host()
    module, runtime = load_transcribe_cpp()
    _, devices = select_exact_vulkan_device(module)
    loaded_native_modules = capture_loaded_native_modules(phase="device_only")
    return {
        "schema_version": 1,
        "status": "PREFLIGHT_PASS",
        "mode": "device-only",
        "captured_at": now_iso(),
        "model_load": "NOT_RUN",
        "inference": "NOT_RUN",
        "p1_verdict": "NOT_EVALUATED",
        "execution_backend": {
            "selected_primary_device": "Intel Arc Vulkan",
            "selection_verified": True,
            "primary_device_fallback_allowed": False,
            "scheduler_cpu_fallback_present": True,
            "cpu_preprocessing_present": True,
            "gpu_only_execution_proven": False,
        },
        "runtime": runtime,
        "inference_environment": inference_environment,
        "test_toolchain": toolchain,
        "host": host,
        "devices": devices,
        "loaded_native_modules": loaded_native_modules,
    }


def _main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8")

    parser = build_parser()
    args = parser.parse_args()
    if args.device_only:
        if args.evidence is None:
            parser.error("--device-only requires --evidence")
        sample_name = "device_only"
    else:
        validate_run_arguments(parser, args)
        sample_name = args.sample_name

    # Authenticate the fixed Python/PowerShell/Git/runner set before any ACL
    # mutation or evidence path creation.
    toolchain = validate_test_toolchain()
    evidence = ensure_evidence_directory(args.evidence, toolchain["git_commit"])
    private_evidence = ensure_private_evidence_directory(evidence)
    run_id = datetime.now().astimezone().strftime("%Y%m%d-%H%M%S-%f") + f"-{os.getpid()}"
    evidence_path = evidence / f"moss-p1-{sample_name}-{run_id}.json"
    private_evidence_path = private_evidence / f"moss-p1-{sample_name}-{run_id}.private.json"
    payload: dict[str, Any]
    try:
        if args.device_only:
            payload = device_only_payload()
            exit_code = 0
        else:
            payload = run_sample(args)
            payload["memory"] = getattr(run_sample, "_last_memory", None)
            exit_code = 0
        payload["evidence_file"] = evidence_path.name
        payload["private_evidence_file"] = private_evidence_path.name
    except (Exception, KeyboardInterrupt) as exc:
        started_perf = getattr(run_sample, "_started_perf", None)
        payload = {
            "schema_version": 1,
            "status": "FAIL",
            "captured_at": now_iso(),
            "gate": "MOSS_V3_P1_INTEL_ARC_VULKAN_SINGLE_SAMPLE",
            "execution_backend": {
                "requested_primary_device": "Intel Arc Vulkan",
                "selection_verified": False,
                "primary_device_fallback_allowed": False,
                "scheduler_cpu_fallback_present": True,
                "cpu_preprocessing_present": True,
                "gpu_only_execution_proven": False,
            },
            "sample_name": sample_name,
            "model_path": str(args.model) if args.model is not None else None,
            "sample_path": str(args.sample) if args.sample is not None else None,
            "memory": getattr(run_sample, "_last_memory", None),
            "context_before_error": getattr(run_sample, "_last_context", None),
            "partial_output": partial_result_record(exc),
            "total_elapsed_seconds": (
                time.perf_counter() - started_perf
                if isinstance(started_perf, (int, float))
                else None
            ),
            "error": {
                "code": stable_error_code(exc),
                "type": type(exc).__name__,
                "message": str(exc),
                "traceback": "".join(
                    traceback.format_exception(type(exc), exc, exc.__traceback__)
                ),
            },
            "evidence_file": evidence_path.name,
            "private_evidence_file": private_evidence_path.name,
        }
        exit_code = 1

    private_hash = write_evidence_atomic(private_evidence_path, payload)
    public_payload = make_public_evidence(payload)
    public_payload["private_evidence"] = {
        "bytes": private_evidence_path.stat().st_size,
        "sha256": private_hash,
        "restricted": True,
    }
    public_payload["restricted_attestation_required"] = True
    assert_public_evidence_safe(public_payload)
    public_hash = write_evidence_atomic(evidence_path, public_payload)
    write_public_attestation_atomic(
        private_evidence,
        evidence_path,
        public_hash,
        private_evidence_path,
        private_hash,
    )
    print(
        json.dumps(
            {
                "status": public_payload["status"],
                "evidence_file": evidence_path.name,
                "evidence_sha256": public_hash,
                "error_code": (public_payload.get("error") or {}).get("code"),
            },
            ensure_ascii=False,
            indent=2,
            allow_nan=False,
        )
    )
    return exit_code


def main() -> int:
    try:
        return _main()
    except (Exception, KeyboardInterrupt) as exc:
        print(
            json.dumps(
                {"status": "FAIL", "error_code": stable_error_code(exc)},
                ensure_ascii=False,
                separators=(",", ":"),
            )
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
