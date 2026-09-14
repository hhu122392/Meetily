#!/usr/bin/env python3
"""Verify the official v0.3 Tauri updater signature and emit JSON evidence."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
from pathlib import Path

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def decode_armored(value: str) -> list[str]:
    return base64.b64decode(value.strip(), validate=True).decode("utf-8").splitlines()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--installer", required=True, type=Path)
    parser.add_argument("--signature", required=True, type=Path)
    parser.add_argument("--tauri-config", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    installer = args.installer.resolve()
    signature_path = args.signature.resolve()
    tauri_config = args.tauri_config.resolve()
    updater_public_key = json.loads(tauri_config.read_text(encoding="utf-8"))["plugins"][
        "updater"
    ]["pubkey"]

    public_lines = decode_armored(updater_public_key)
    if len(public_lines) != 2:
        raise RuntimeError("unexpected minisign public-key armor")
    public_blob = base64.b64decode(public_lines[1], validate=True)
    if len(public_blob) != 42 or public_blob[:2] != b"Ed":
        raise RuntimeError("unexpected minisign Ed25519 public-key payload")
    key_id = public_blob[2:10]
    public_key = Ed25519PublicKey.from_public_bytes(public_blob[10:])

    signature_lines = decode_armored(signature_path.read_text(encoding="ascii"))
    if len(signature_lines) != 4:
        raise RuntimeError("unexpected minisign signature armor")
    signature_blob = base64.b64decode(signature_lines[1], validate=True)
    global_signature = base64.b64decode(signature_lines[3], validate=True)
    if len(signature_blob) != 74 or signature_blob[:2] != b"ED":
        raise RuntimeError("unexpected prehashed minisign signature payload")
    if signature_blob[2:10] != key_id:
        raise RuntimeError("signature key ID does not match the configured updater key")
    if len(global_signature) != 64:
        raise RuntimeError("unexpected minisign global signature length")

    main_signature = signature_blob[10:]
    file_digest = hashlib.blake2b(installer.read_bytes(), digest_size=64).digest()
    main_valid = True
    global_valid = True
    try:
        public_key.verify(main_signature, file_digest)
    except InvalidSignature:
        main_valid = False
    trusted_comment = signature_lines[2]
    if not trusted_comment.startswith("trusted comment: "):
        raise RuntimeError("trusted-comment prefix is missing")
    try:
        public_key.verify(
            global_signature,
            main_signature + trusted_comment[len("trusted comment: ") :].encode("utf-8"),
        )
    except InvalidSignature:
        global_valid = False

    result = {
        "schemaVersion": 1,
        "phase": "15.11 / Stage 5A-3",
        "scope": "Offline Ed25519/minisign verification of the official v0.3.0 Tauri updater signature",
        "installer": {
            "path": str(installer),
            "bytes": installer.stat().st_size,
            "sha256": sha256(installer),
            "blake2b512": file_digest.hex().upper(),
        },
        "signature": {
            "path": str(signature_path),
            "bytes": signature_path.stat().st_size,
            "sha256": sha256(signature_path),
            "algorithm": signature_blob[:2].decode("ascii"),
            "keyId": key_id[::-1].hex().upper(),
            "trustedComment": trusted_comment[len("trusted comment: ") :],
        },
        "configuredPublicKey": {
            "algorithm": public_blob[:2].decode("ascii"),
            "keyId": key_id[::-1].hex().upper(),
        },
        "assertions": {
            "mainSignatureValid": main_valid,
            "trustedCommentSignatureValid": global_valid,
            "keyIdsMatch": signature_blob[2:10] == key_id,
        },
        "passed": main_valid and global_valid and signature_blob[2:10] == key_id,
    }
    args.output.resolve().parent.mkdir(parents=True, exist_ok=True)
    args.output.resolve().write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
