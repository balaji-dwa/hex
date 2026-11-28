import hashlib
import json
import pathlib
import tempfile
import unittest

import manifest


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        self.zip_file = self.root / "Hex.zip"
        self.dmg_file = self.root / "Hex.dmg"
        self.zip_file.write_bytes(b"zip-payload-bytes")
        self.dmg_file.write_bytes(b"dmg-payload-bytes-longer")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_build_computes_exact_hashes_sizes_and_schema(self) -> None:
        out = self.root / "dev.json"
        rc = manifest.main(
            [
                "build",
                "--channel",
                "dev",
                "--version",
                "0.0.0-dev-1795843000",
                "--sequence",
                "7",
                "--zip",
                str(self.zip_file),
                "--dmg",
                str(self.dmg_file),
                "--notes-url",
                "https://releases.example.com/hex/dev",
                "--published-at",
                "2026-11-28T05:00:00Z",
                "--out",
                str(out),
            ]
        )
        self.assertEqual(rc, 0)
        data = json.loads(out.read_text(encoding="utf-8"))
        zip_sha = hashlib.sha256(b"zip-payload-bytes").hexdigest()
        dmg_sha = hashlib.sha256(b"dmg-payload-bytes-longer").hexdigest()

        self.assertEqual(data["schema"], 1)
        self.assertEqual(data["app"], "hex")
        self.assertEqual(data["artifact"], "desktop")
        self.assertEqual(data["channel"], "dev")
        self.assertEqual(data["sequence"], 7)
        self.assertEqual(data["release"]["version"], "0.0.0-dev-1795843000")
        self.assertEqual(data["release"]["rollout_percent"], 100)
        self.assertEqual(data["release"]["required"], False)
        self.assertEqual(data["release"]["distributions"], [{"kind": "direct"}])
        self.assertEqual(
            data["release"]["targets"]["macos-arm64"],
            {
                "url": f"/hex/files/{zip_sha}/Hex.zip",
                "size": len(b"zip-payload-bytes"),
                "sha256": zip_sha,
            },
        )
        self.assertEqual(
            data["release"]["targets"]["macos-arm64-dmg"],
            {
                "url": f"/hex/files/{dmg_sha}/Hex.dmg",
                "size": len(b"dmg-payload-bytes-longer"),
                "sha256": dmg_sha,
            },
        )

    def test_rollback_preserves_release_and_bumps_sequence(self) -> None:
        archived = manifest.build_manifest(
            channel="dev",
            version="0.0.0-dev-100",
            sequence=3,
            zip_path=self.zip_file,
            dmg_path=self.dmg_file,
            published_at="2026-11-27T00:00:00Z",
        )
        rolled = manifest.rollback_manifest(
            archived=archived,
            channel="dev",
            sequence=5,
            published_at="2026-11-28T01:00:00Z",
        )
        self.assertEqual(rolled["sequence"], 5)
        self.assertEqual(rolled["published_at"], "2026-11-28T01:00:00Z")
        self.assertEqual(rolled["release"], archived["release"])

    def test_rejects_invalid_version_and_notes_url(self) -> None:
        with self.assertRaises(ValueError):
            manifest.build_manifest(
                channel="dev",
                version=".hidden",
                sequence=1,
                zip_path=self.zip_file,
                dmg_path=self.dmg_file,
            )
        with self.assertRaises(ValueError):
            manifest.build_manifest(
                channel="dev",
                version="0.0.0-dev-1",
                sequence=1,
                zip_path=self.zip_file,
                dmg_path=self.dmg_file,
                notes_url="http://insecure.example.com/notes",
            )

    def test_notes_deduplicates_strips_control_chars_and_replaces_same_version(self) -> None:
        existing = {
            "schema": 1,
            "builds": [
                {
                    "version": "0.0.0-dev-10",
                    "commit": "oldsha",
                    "published_at": "2026-11-27T00:00:00Z",
                    "items": [{"title": "Old", "url": ""}],
                }
            ],
        }
        updated = manifest.update_dev_notes(
            version="0.0.0-dev-11",
            commit="abc1234",
            published_at="2026-11-28T00:00:00Z",
            titles=["Fix A", "", "Fix A", "Fix\x07 B"],
            existing=existing,
        )
        self.assertEqual(len(updated["builds"]), 2)
        self.assertEqual(
            [item["title"] for item in updated["builds"][0]["items"]],
            ["Fix A", "Fix B"],
        )
        rerun = manifest.update_dev_notes(
            version="0.0.0-dev-11",
            commit="abc1234",
            published_at="2026-11-28T00:01:00Z",
            titles=["Fix A"],
            existing=updated,
        )
        self.assertEqual(len(rerun["builds"]), 2)
        self.assertEqual(rerun["builds"][0]["version"], "0.0.0-dev-11")

    def test_next_sequence(self) -> None:
        missing = self.root / "missing.json"
        self.assertEqual(manifest.next_sequence(missing), 1)
        current = self.root / "dev.json"
        current.write_text(json.dumps({"sequence": 12}), encoding="utf-8")
        self.assertEqual(manifest.next_sequence(current), 13)

        current.write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Invalid sequence"):
            manifest.next_sequence(current)


if __name__ == "__main__":
    unittest.main()
