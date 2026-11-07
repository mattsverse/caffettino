#!/usr/bin/env python3
"""Version metadata and the Homebrew cask for a Caffettino release (Python 3.11+)."""

import argparse
import hashlib
import os
from pathlib import Path
import plistlib
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")


def version_parts(version):
    if not VERSION.fullmatch(version):
        raise ValueError(f"Expected a stable major.minor.patch version, got {version!r}")
    return tuple(map(int, version.split(".")))


def project_version():
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["package"]["version"]
    version_parts(version)
    return version


def validate_tag(tag, version):
    if tag != f"v{version}":
        raise ValueError(f"Release tag {tag!r} must match Cargo.toml: v{version}")


def project_plist():
    with (ROOT / "assets/Info.plist").open("rb") as source:
        return plistlib.load(source)


def write_plist(output, version, build_number="1"):
    if not re.fullmatch(r"[1-9][0-9]{0,3}", build_number):
        raise ValueError("The bundle build number must be an integer from 1 to 9999")
    metadata = project_plist()
    metadata["CFBundleShortVersionString"] = version
    metadata["CFBundleVersion"] = build_number
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("wb") as target:
        plistlib.dump(metadata, target)


def cask_text(version, checksum, repository):
    version_parts(version)
    if not re.fullmatch(r"[a-f0-9]{64}", checksum):
        raise ValueError("Expected a SHA-256 checksum")
    if not REPOSITORY.fullmatch(repository):
        raise ValueError("Expected a GitHub owner/repository")
    bundle_identifier = project_plist()["CFBundleIdentifier"]
    return f'''cask "caffettino" do
  version "{version}"
  sha256 "{checksum}"

  url "https://github.com/{repository}/releases/download/v#{{version}}/Caffettino-#{{version}}-arm64.dmg"
  name "Caffettino"
  desc "Whimsical menu bar app that keeps your Mac awake"
  homepage "https://github.com/{repository}"

  depends_on arch: :arm64
  depends_on macos: ">= :ventura"

  # Caffettino updates itself in place; `brew upgrade` leaves it to that.
  auto_updates true

  app "Caffettino.app"

  uninstall quit: "{bundle_identifier}"
end
'''


def cask_version(text):
    match = re.search(r'^  version "([^"]+)"$', text, re.MULTILINE)
    if not match:
        raise ValueError("Cannot read the existing cask version; refusing to overwrite it")
    return version_parts(match[1])


def install_cask(source, tap):
    content = source.read_text()
    incoming = cask_version(content)
    destination = tap / "Casks/caffettino.rb"
    if destination.exists():
        existing = destination.read_text()
        current = cask_version(existing)
        if current > incoming:
            print("The tap already has a newer version; leaving it in place.")
            return
        if current == incoming and existing != content:
            raise ValueError("This version already exists with different contents; refusing to replace it")
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(content)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    metadata = commands.add_parser("version")
    metadata.add_argument("--tag")
    plist = commands.add_parser("plist")
    plist.add_argument("--output", type=Path, required=True)
    plist.add_argument("--build-number", default=os.environ.get("GITHUB_RUN_NUMBER", "1"))
    cask = commands.add_parser("cask")
    cask.add_argument("--dmg", type=Path, required=True)
    cask.add_argument("--repository", required=True)
    cask.add_argument("--output", type=Path, required=True)
    install = commands.add_parser("install-cask")
    install.add_argument("--source", type=Path, required=True)
    install.add_argument("--tap", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "install-cask":
        install_cask(args.source, args.tap)
        return
    version = project_version()
    if args.command == "version":
        if args.tag:
            validate_tag(args.tag, version)
        print(version)
    elif args.command == "plist":
        write_plist(args.output, version, args.build_number)
    elif args.command == "cask":
        expected = f"Caffettino-{version}-arm64.dmg"
        if args.dmg.name != expected:
            raise ValueError(f"Expected DMG filename {expected!r}")
        with args.dmg.open("rb") as artifact:
            checksum = hashlib.file_digest(artifact, "sha256").hexdigest()
        args.output.write_text(cask_text(version, checksum, args.repository))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        sys.exit(str(error))
