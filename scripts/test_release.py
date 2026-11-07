import hashlib
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch

import release


class ReleaseTests(unittest.TestCase):
    def test_tag_must_match_stable_manifest_version(self):
        release.validate_tag("v0.1.0", "0.1.0")
        for tag in ("0.1.0", "v0.2.0", "v0.1.0-beta", "v0.1.0\n"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.validate_tag(tag, "0.1.0")
        for version in ("01.0.0", "1.2", "1.0.0-beta", '1.0.0"; system("bad")'):
            with self.subTest(version=version), self.assertRaises(ValueError):
                release.version_parts(version)

    def test_plist_uses_manifest_version_and_keeps_bundle_identity(self):
        with (release.ROOT / "assets/Info.plist").open("rb") as source:
            template = plistlib.load(source)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "Contents/Info.plist"
            release.write_plist(path, "2.3.4", "42")
            with path.open("rb") as source:
                metadata = plistlib.load(source)
        self.assertEqual(metadata["CFBundleShortVersionString"], "2.3.4")
        self.assertEqual(metadata["CFBundleVersion"], "42")
        self.assertEqual(metadata["CFBundleIdentifier"], template["CFBundleIdentifier"])
        self.assertEqual(metadata["LSMinimumSystemVersion"], "13.0")
        self.assertTrue(metadata["LSUIElement"])

    def test_cask_installs_app_from_matching_dmg(self):
        checksum = hashlib.sha256(b"a signed disk image").hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "assets").mkdir()
            with (root / "assets/Info.plist").open("wb") as source:
                plistlib.dump({"CFBundleIdentifier": "com.example.caffettino"}, source)
            with patch.object(release, "ROOT", root):
                text = release.cask_text("0.1.0", checksum, "mattsverse/caffettino")
        self.assertIn(f'sha256 "{checksum}"', text)
        self.assertIn("/mattsverse/caffettino/releases/download/v#{version}/Caffettino-#{version}-arm64.dmg", text)
        self.assertIn('app "Caffettino.app"', text)
        self.assertIn("auto_updates true", text)
        self.assertIn("depends_on arch: :arm64", text)
        self.assertIn('depends_on macos: ">= :ventura"', text)
        self.assertIn('uninstall quit: "com.example.caffettino"', text)
        self.assertNotIn(".zip", text)
        for repository in ("bad", 'owner/repo"', "owner/repo\n", "owner/repo/other"):
            with self.subTest(repository=repository), self.assertRaises(ValueError):
                release.cask_text("0.1.0", checksum, repository)
        with self.assertRaises(ValueError):
            release.cask_text("0.1.0", "not-a-checksum", "owner/repo")

    def test_tap_update_is_idempotent_and_cannot_downgrade(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "caffettino.rb"
            tap = root / "tap"
            old = release.cask_text("0.9.0", "a" * 64, "mattsverse/caffettino")
            new = release.cask_text("0.10.0", "b" * 64, "mattsverse/caffettino")
            source.write_text(old)
            release.install_cask(source, tap)
            source.write_text(new)
            release.install_cask(source, tap)
            release.install_cask(source, tap)
            source.write_text(old)
            release.install_cask(source, tap)
            self.assertEqual((tap / "Casks/caffettino.rb").read_text(), new)

    def test_same_version_with_different_checksum_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "caffettino.rb"
            source.write_text(release.cask_text("0.1.0", "a" * 64, "owner/repo"))
            release.install_cask(source, root / "tap")
            source.write_text(release.cask_text("0.1.0", "b" * 64, "owner/repo"))
            with self.assertRaisesRegex(ValueError, "different contents"):
                release.install_cask(source, root / "tap")

    def test_unrecognized_existing_cask_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "tap/Casks/caffettino.rb"
            destination.parent.mkdir(parents=True)
            destination.write_text('cask "caffettino" do\n  version :latest\nend\n')
            source = root / "caffettino.rb"
            source.write_text(release.cask_text("0.1.0", "a" * 64, "owner/repo"))
            with self.assertRaisesRegex(ValueError, "Cannot read"):
                release.install_cask(source, root / "tap")
            self.assertIn("version :latest", destination.read_text())


if __name__ == "__main__":
    unittest.main()
