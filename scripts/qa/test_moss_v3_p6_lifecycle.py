from __future__ import annotations

import json
from pathlib import Path
import shutil
import sqlite3
import tempfile
from types import SimpleNamespace
import unittest

from moss_v3_p6_common import FAIL, PASS, GateError, sha256_file
from moss_v3_p6_lifecycle import command_snapshot, command_verify
from moss_v3_p6_release import git_head


REPO = Path(__file__).resolve().parents[2]


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


class LifecycleToolTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.head = git_head(REPO)
        self.recordings = self.root / "recordings"
        self.recordings.mkdir()
        (self.recordings / "meeting.wav.hash-only-fixture").write_text("recording-bytes", encoding="utf-8")
        self.settings = self.root / "settings.json"
        write_json(self.settings, {"theme": "dark", "moss": {"enabled": False}})
        self.database = self.root / "meeting.sqlite"
        connection = sqlite3.connect(self.database)
        connection.executescript(
            """
            CREATE TABLE meetings (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
            CREATE TABLE transcripts (id INTEGER PRIMARY KEY, meeting_id INTEGER, body TEXT NOT NULL);
            INSERT INTO meetings VALUES (1, 'private meeting title');
            INSERT INTO transcripts VALUES (1, 1, 'private transcript text');
            """
        )
        connection.commit()
        connection.close()
        self.moss = self.root / "moss-data"
        self.moss.mkdir()
        (self.moss / "model.gguf.fixture").write_bytes(b"model")
        (self.moss / "runtime.dll").write_bytes(b"runtime")
        self.protected_trees = {
            "templates": self.root / "templates",
            "whisper_models": self.root / "whisper-models",
            "parakeet_models": self.root / "parakeet-models",
            "qwen_models": self.root / "qwen-models",
            "gemma_models": self.root / "gemma-models",
        }
        for root_id, path in self.protected_trees.items():
            path.mkdir()
            (path / f"{root_id}.fixture").write_text(root_id, encoding="utf-8")
        self.application = self.root / "application"
        self.config = self.root / "snapshot-config.json"
        write_json(
            self.config,
            {
                "schema_version": 1,
                "roots": [
                    {
                        "id": "recordings",
                        "path": str(self.recordings),
                        "kind": "tree",
                        "classification": "protected",
                        "required": True,
                    },
                    {
                        "id": "settings",
                        "path": str(self.settings),
                        "kind": "json",
                        "classification": "protected",
                        "required": True,
                    },
                    {
                        "id": "database",
                        "path": str(self.database),
                        "kind": "sqlite",
                        "classification": "protected",
                        "required": True,
                        "tables": "*",
                    },
                    *[
                        {
                            "id": root_id,
                            "path": str(path),
                            "kind": "tree",
                            "classification": "protected",
                            "required": True,
                        }
                        for root_id, path in self.protected_trees.items()
                    ],
                    {
                        "id": "moss_data",
                        "path": str(self.moss),
                        "kind": "tree",
                        "classification": "moss_owned",
                        "required": True,
                    },
                    {
                        "id": "application",
                        "path": str(self.application),
                        "kind": "tree",
                        "classification": "application",
                        "required": False,
                    },
                ],
            },
        )
        self.snapshots: dict[str, Path] = {}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def capture(self, checkpoint: str) -> Path:
        output = self.root / f"{checkpoint}.json"
        result = command_snapshot(
            SimpleNamespace(repo=REPO, config=self.config, checkpoint=checkpoint, output=output)
        )
        self.assertEqual(result, 0)
        self.snapshots[checkpoint] = output
        return output

    def make_release_manifest(self) -> Path:
        output = self.root / "release-manifest.json"
        write_json(
            output,
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P6_RELEASE_MANIFEST",
                "source_commit": self.head,
                "status": PASS,
                "deployment_roots": {"moss": {"classification": "moss_data"}},
                "files": [
                    {
                        "id": "model",
                        "root": "moss",
                        "path": "model.gguf.fixture",
                        "role": "moss_model",
                        "bytes": (self.moss / "model.gguf.fixture").stat().st_size,
                        "sha256": sha256_file(self.moss / "model.gguf.fixture"),
                    },
                    {
                        "id": "runtime",
                        "root": "moss",
                        "path": "runtime.dll",
                        "role": "moss_runtime",
                        "bytes": (self.moss / "runtime.dll").stat().st_size,
                        "sha256": sha256_file(self.moss / "runtime.dll"),
                    },
                ],
            },
        )
        return output

    def protected_assertions(self) -> list[dict[str, str]]:
        return [
            {"root": "recordings", "mode": "exact"},
            {"root": "settings", "mode": "json_preserve"},
            {"root": "database", "mode": "sqlite_preserve"},
            *[
                {"root": root_id, "mode": "exact"}
                for root_id in self.protected_trees
            ],
        ]

    def make_spec(self) -> Path:
        spec = self.root / "lifecycle-spec.json"
        write_json(
            spec,
            {
                "schema_version": 1,
                "transitions": [
                    {
                        "id": "install",
                        "before": "before_install",
                        "after": "after_install",
                        "assertions": [
                            *self.protected_assertions(),
                            {"root": "moss_data", "mode": "exact"},
                            {"root": "application", "mode": "nonempty"},
                            {"root": "application", "mode": "no_moss_assets"},
                        ],
                    },
                    {
                        "id": "upgrade",
                        "before": "before_upgrade",
                        "after": "after_upgrade",
                        "assertions": [
                            *self.protected_assertions(),
                            {"root": "moss_data", "mode": "exact"},
                            {"root": "application", "mode": "changed"},
                            {"root": "application", "mode": "nonempty"},
                            {"root": "application", "mode": "no_moss_assets"},
                        ],
                    },
                    {
                        "id": "uninstall",
                        "before": "after_upgrade",
                        "after": "after_uninstall",
                        "assertions": [
                            *self.protected_assertions(),
                            {"root": "moss_data", "mode": "exact"},
                            {"root": "application", "mode": "absent"},
                        ],
                    },
                    {
                        "id": "rollback",
                        "before": "before_rollback",
                        "after": "after_rollback",
                        "assertions": [
                            *self.protected_assertions(),
                            {"root": "moss_data", "mode": "exact"},
                            {"root": "application", "mode": "changed"},
                            {"root": "application", "mode": "nonempty"},
                            {"root": "application", "mode": "no_moss_assets"},
                        ],
                    },
                ],
            },
        )
        return spec

    def prepare_all_snapshots(self) -> None:
        self.capture("before_install")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v1")
        self.capture("after_install")
        (self.application / "meetily.exe").write_bytes(b"stable-before-upgrade")
        self.capture("before_upgrade")
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("after_upgrade")
        shutil.rmtree(self.application)
        self.capture("after_uninstall")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("before_rollback")
        (self.application / "meetily.exe").write_bytes(b"v0")
        self.capture("after_rollback")

    def verify(self, spec: Path, release_manifest: Path) -> tuple[int, dict[str, object]]:
        output = self.root / "lifecycle-report.json"
        result = command_verify(
            SimpleNamespace(
                repo=REPO,
                spec=spec,
                snapshot=[f"{name}={path}" for name, path in self.snapshots.items()],
                release_manifest=release_manifest,
                output=output,
            )
        )
        return result, json.loads(output.read_text(encoding="utf-8"))

    def test_snapshot_hides_json_and_sqlite_values(self) -> None:
        path = self.capture("privacy")
        text = path.read_text(encoding="utf-8")
        self.assertNotIn("private meeting title", text)
        self.assertNotIn("private transcript text", text)
        self.assertNotIn('"dark"', text)
        report = json.loads(text)
        database = next(root for root in report["roots"] if root["id"] == "database")
        self.assertEqual(database["integrity"], "ok")
        self.assertEqual({table["row_count"] for table in database["tables"]}, {1})

    def test_all_four_lifecycle_transitions_pass_with_strong_data_checks(self) -> None:
        self.prepare_all_snapshots()
        result, report = self.verify(self.make_spec(), self.make_release_manifest())
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], PASS)
        self.assertEqual({item["id"] for item in report["transitions"]}, {"install", "upgrade", "uninstall", "rollback"})
        self.assertFalse(report["coverage_failures"])

    def test_changed_recording_hash_fails_lifecycle(self) -> None:
        self.prepare_all_snapshots()
        after_upgrade = json.loads(self.snapshots["after_upgrade"].read_text(encoding="utf-8"))
        recordings = next(root for root in after_upgrade["roots"] if root["id"] == "recordings")
        recordings["records"][0]["sha256"] = "0" * 64
        write_json(self.snapshots["after_upgrade"], after_upgrade)
        result, report = self.verify(self.make_spec(), self.make_release_manifest())
        self.assertEqual(result, 1)
        self.assertEqual(report["status"], FAIL)
        self.assertFalse(report["checks"]["upgrade:recordings:exact"])

    def test_missing_protected_root_assertion_is_a_hard_failure(self) -> None:
        self.prepare_all_snapshots()
        spec_path = self.make_spec()
        spec = json.loads(spec_path.read_text(encoding="utf-8"))
        spec["transitions"][0]["assertions"] = [
            item for item in spec["transitions"][0]["assertions"] if item["root"] != "database"
        ]
        write_json(spec_path, spec)
        result, report = self.verify(spec_path, self.make_release_manifest())
        self.assertEqual(result, 1)
        self.assertIn("install:database:protected root lacks strong assertion", report["coverage_failures"])

    def test_application_cannot_contain_D_drive_model_hash(self) -> None:
        self.capture("before_install")
        self.application.mkdir()
        shutil.copyfile(self.moss / "model.gguf.fixture", self.application / "hidden.bin")
        self.capture("after_install")
        (self.application / "hidden.bin").write_bytes(b"stable")
        self.capture("before_upgrade")
        (self.application / "hidden.bin").write_bytes(b"upgrade")
        self.capture("after_upgrade")
        shutil.rmtree(self.application)
        self.capture("after_uninstall")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"upgrade")
        self.capture("before_rollback")
        (self.application / "meetily.exe").write_bytes(b"rollback")
        self.capture("after_rollback")
        result, report = self.verify(self.make_spec(), self.make_release_manifest())
        self.assertEqual(result, 1)
        self.assertFalse(report["checks"]["install:application:no_moss_assets"])

    def test_uninstall_requires_application_directory_removal(self) -> None:
        self.capture("before_install")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v1")
        self.capture("after_install")
        self.capture("before_upgrade")
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("after_upgrade")
        (self.application / "meetily.exe").unlink()
        self.capture("after_uninstall")
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("before_rollback")
        (self.application / "meetily.exe").write_bytes(b"v0")
        self.capture("after_rollback")
        result, report = self.verify(self.make_spec(), self.make_release_manifest())
        self.assertEqual(result, 1)
        self.assertFalse(report["checks"]["uninstall:application:absent"])

    def test_sqlite_preserve_allows_additive_schema_and_rows(self) -> None:
        self.capture("before_install")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v1")
        connection = sqlite3.connect(self.database)
        connection.execute("ALTER TABLE meetings ADD COLUMN moss_state TEXT")
        connection.execute("INSERT INTO meetings (id, title, moss_state) VALUES (2, 'new', 'candidate')")
        connection.commit()
        connection.close()
        self.capture("after_install")
        (self.application / "meetily.exe").write_bytes(b"stable-before-upgrade")
        self.capture("before_upgrade")
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("after_upgrade")
        shutil.rmtree(self.application)
        self.capture("after_uninstall")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("before_rollback")
        (self.application / "meetily.exe").write_bytes(b"v0")
        self.capture("after_rollback")

        result, report = self.verify(self.make_spec(), self.make_release_manifest())
        self.assertEqual(result, 0)
        self.assertEqual(report["status"], PASS)

    def test_sqlite_preserve_rejects_rewritten_old_rows(self) -> None:
        self.capture("before_install")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v1")
        connection = sqlite3.connect(self.database)
        connection.execute("UPDATE meetings SET title = 'rewritten' WHERE id = 1")
        connection.commit()
        connection.close()
        self.capture("after_install")
        (self.application / "meetily.exe").write_bytes(b"stable-before-upgrade")
        self.capture("before_upgrade")
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("after_upgrade")
        shutil.rmtree(self.application)
        self.capture("after_uninstall")
        self.application.mkdir()
        (self.application / "meetily.exe").write_bytes(b"v2")
        self.capture("before_rollback")
        (self.application / "meetily.exe").write_bytes(b"v0")
        self.capture("after_rollback")

        result, report = self.verify(self.make_spec(), self.make_release_manifest())
        self.assertEqual(result, 1)
        self.assertFalse(report["checks"]["install:database:sqlite_preserve"])

    def test_verify_rejects_missing_named_checkpoint(self) -> None:
        self.prepare_all_snapshots()
        del self.snapshots["before_upgrade"]
        with self.assertRaisesRegex(GateError, "seven checkpoints"):
            self.verify(self.make_spec(), self.make_release_manifest())

    def test_required_protected_tree_cannot_be_empty(self) -> None:
        (self.protected_trees["gemma_models"] / "gemma_models.fixture").unlink()
        output = self.root / "empty-protected.json"
        result = command_snapshot(
            SimpleNamespace(
                repo=REPO,
                config=self.config,
                checkpoint="before_install",
                output=output,
            )
        )
        self.assertEqual(result, 1)
        report = json.loads(output.read_text(encoding="utf-8"))
        self.assertIn("gemma_models", report["empty_required_tree_roots"])


if __name__ == "__main__":
    unittest.main()
