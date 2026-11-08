"""Exercise release gates without Apple credentials or network requests."""

import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import release


FAKE_TOOL = r'''#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["TEST_TOOL_LOG"], "a") as log:
    log.write(json.dumps([name, *args]) + "\n")
failure = os.environ.get("TEST_FAILURE", "")
if name == "uname":
    print("Darwin" if args == ["-s"] else "arm64")
elif name == "lipo" and failure == "single-arch":
    sys.exit(1)
elif name == "security":
    if args[0] == "create-keychain":
        pathlib.Path(args[-1]).touch()
    elif args[0] == "delete-keychain":
        pathlib.Path(args[-1]).unlink(missing_ok=True)
    elif args[0] == "find-identity" and failure != "identity":
        team = "OTHERTEAM" if failure == "wrong-team" else "TESTTEAM"
        print('  1) ' + 'A' * 40 + ' "Developer ID Application: Test (' + team + ')"')
    elif args == ["list-keychains", "-d", "user"]:
        print(os.environ.get("TEST_KEYCHAIN_LIST", '    "/Users/test/Library/Keychains/login.keychain-db"\n    "/Users/test/Library/Keychains/another keychain.keychain-db"'))
elif name == "xcrun" and args[:2] == ["notarytool", "submit"]:
    kind = "app" if args[2].endswith(".zip") else "dmg"
    print(json.dumps({"id": "test-id", "status": "Invalid" if failure == kind else "Accepted"}))
elif name == "xcrun" and args[:2] == ["notarytool", "log"]:
    pathlib.Path(args[-1]).write_text('{"issues": []}')
elif name == "ditto":
    if "-k" in args:
        pathlib.Path(args[-1]).write_bytes(b"transport archive")
    else:
        shutil.copytree(args[-2], args[-1])
elif name == "hdiutil":
    if args[0] == "create":
        pathlib.Path(args[-1]).write_bytes(b"disk image")
    elif args[0] == "attach":
        root = pathlib.Path(os.environ["TEST_TOOL_LOG"]).parent
        shutil.copytree(root / "dist/Caffettino.app", pathlib.Path(args[-1]) / "Caffettino.app")
    elif args[0] == "detach":
        shutil.rmtree(args[-1])
elif name == "spctl" and failure == "mounted-app" and "/mounted/" in args[-1]:
    sys.exit(1)
'''


@unittest.skipUnless(sys.platform == "darwin", "uses the macOS PlistBuddy version check")
class SigningGatesTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "scripts").mkdir()
        (self.root / "assets").mkdir()
        for filename in ("sign-release.sh", "release.py", "keychain.sh"):
            shutil.copy(release.ROOT / "scripts" / filename, self.root / "scripts" / filename)
        shutil.copy(release.ROOT / "Cargo.toml", self.root / "Cargo.toml")
        shutil.copy(release.ROOT / "assets/Info.plist", self.root / "assets/Info.plist")
        app = self.root / "dist/Caffettino.app/Contents"
        (app / "MacOS").mkdir(parents=True)
        (app / "MacOS/caffettino").touch(mode=0o755)
        release.write_plist(app / "Info.plist", release.project_version())
        binaries = self.root / "bin"
        binaries.mkdir()
        shim = binaries / "fake-tool"
        shim.write_text(FAKE_TOOL)
        shim.chmod(0o755)
        for command in ("uname", "lipo", "security", "codesign", "xcrun", "ditto", "hdiutil", "spctl"):
            (binaries / command).symlink_to(shim)
        self.log = self.root / "tools.jsonl"
        self.environment = {
            **os.environ,
            "PATH": f"{binaries}:{os.environ['PATH']}",
            "RUNNER_TEMP": str(self.root),
            "RELEASE_TAG": f"v{release.project_version()}",
            "APPLE_CERTIFICATE_P12": base64.b64encode(b"test certificate").decode(),
            "APPLE_CERTIFICATE_PASSWORD": "test-password",
            "APPLE_ID": "test@example.com",
            "APPLE_TEAM_ID": "TESTTEAM",
            "APPLE_APP_SPECIFIC_PASSWORD": "test-app-password",
            "TEST_TOOL_LOG": str(self.log),
        }

    def run_signing(self, failure=""):
        result = subprocess.run(
            ["bash", str(self.root / "scripts/sign-release.sh")],
            env={**self.environment, "TEST_FAILURE": failure},
            capture_output=True, text=True,
        )
        self.assertEqual(list(self.root.glob("caffettino-signing.*")), [], "private signing files leaked")
        calls = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return result, calls

    def test_success_notarizes_both_app_and_dmg_and_cleans_credentials(self):
        result, calls = self.run_signing()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum(call[:3] == ["xcrun", "notarytool", "submit"] for call in calls), 2)
        self.assertEqual(sum(call[:3] == ["xcrun", "notarytool", "log"] for call in calls), 2)
        signing = next(call for call in calls if call[:2] == ["codesign", "--force"])
        self.assertIn("runtime", signing)
        self.assertIn("--timestamp", signing)
        self.assertTrue(list((self.root / "dist").glob("*.dmg")))
        update = str(self.root / f"dist/Caffettino-{release.project_version()}-universal.zip")
        archived = next(i for i, call in enumerate(calls) if call[0] == "ditto" and call[-1] == update)
        stapled = next(i for i, call in enumerate(calls) if call[:3] == ["xcrun", "stapler", "staple"])
        self.assertLess(stapled, archived, "the updater's archive holds the stapled app")
        self.assertEqual(calls[archived][1:4], ["-c", "-k", "--keepParent"])
        profile = next(call for call in calls if call[:3] == ["xcrun", "notarytool", "store-credentials"])
        self.assertEqual(profile[3], "caffettino")
        self.assertIn("--apple-id", profile)
        self.assertIn("--team-id", profile)
        for call in calls:
            if call[:3] == ["xcrun", "notarytool", "submit"]:
                self.assertEqual(call[call.index("--keychain-profile") + 1], "caffettino")
                self.assertNotIn("--key", call)
        changes = [call for call in calls if call[:5] == ["security", "list-keychains", "-d", "user", "-s"]]
        original = ["/Users/test/Library/Keychains/login.keychain-db", "/Users/test/Library/Keychains/another keychain.keychain-db"]
        self.assertEqual(changes[0][6:], original)
        self.assertEqual(changes[-1][5:], original)
        self.assertTrue(any(call[:2] == ["hdiutil", "verify"] for call in calls))
        self.assertTrue(any(call[:2] == ["hdiutil", "detach"] for call in calls))

    def test_empty_keychain_search_list_is_restored(self):
        self.environment["TEST_KEYCHAIN_LIST"] = ""
        result, calls = self.run_signing()
        self.assertEqual(result.returncode, 0, result.stderr)
        changes = [call for call in calls if call[:5] == ["security", "list-keychains", "-d", "user", "-s"]]
        self.assertEqual(len(changes[0]), 6)
        self.assertEqual(len(changes[-1]), 5)

    def test_mounted_app_rejection_detaches_image_and_cleans_credentials(self):
        result, calls = self.run_signing("mounted-app")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(any(call[:2] == ["hdiutil", "detach"] for call in calls))
        self.assertEqual(calls[-2][:5], ["security", "list-keychains", "-d", "user", "-s"])
        self.assertEqual(calls[-1][:2], ["security", "delete-keychain"])

    def test_apple_rejection_aborts_and_cleans_credentials(self):
        result, calls = self.run_signing("app")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Apple did not accept", result.stderr)
        self.assertTrue(any(call[:3] == ["xcrun", "notarytool", "log"] for call in calls))
        self.assertFalse(any(call[0] == "hdiutil" for call in calls))

    def test_dmg_rejection_aborts_and_cleans_credentials(self):
        result, calls = self.run_signing("dmg")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(call[:3] == ["xcrun", "stapler", "staple"] and call[-1].endswith(".dmg") for call in calls))

    def test_invalid_identity_cannot_fall_back_to_ad_hoc_signing(self):
        result, calls = self.run_signing("identity")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exactly one valid Developer ID", result.stderr)
        self.assertFalse(any(call[0] == "codesign" for call in calls))

    def test_wrong_team_identity_is_rejected(self):
        result, calls = self.run_signing("wrong-team")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("for APPLE_TEAM_ID", result.stderr)
        self.assertFalse(any(call[0] == "codesign" for call in calls))

    def test_single_architecture_app_is_rejected_before_importing_credentials(self):
        result, calls = self.run_signing("single-arch")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Releases must be universal", result.stderr)
        self.assertFalse(any(call[0] == "security" for call in calls))

    def test_missing_secret_fails_before_importing_credentials(self):
        self.environment.pop("APPLE_APP_SPECIFIC_PASSWORD")
        result, calls = self.run_signing()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Missing GitHub Actions secret: APPLE_APP_SPECIFIC_PASSWORD", result.stderr)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
