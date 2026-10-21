#!/usr/bin/env python3
"""Check the cargo-dist package contract consumed by cruise's native updater."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tarfile
import tomllib
import zipfile
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
HEX_SHA256 = re.compile(r"[0-9a-fA-F]{64}\Z")
CHECKSUM_LINE = re.compile(r"([0-9a-fA-F]{64}) [ *](.+)\Z")


def fail(message: str) -> None:
    raise ValueError(message)


def targets_from_cargo_toml() -> list[str]:
    with (ROOT / "Cargo.toml").open("rb") as cargo_file:
        dist = tomllib.load(cargo_file)["workspace"]["metadata"]["dist"]
    targets = dist.get("targets")
    if not isinstance(targets, list) or not targets or any(not isinstance(t, str) for t in targets):
        fail("Cargo.toml workspace.metadata.dist.targets must be a non-empty string list")
    if len(set(targets)) != len(targets):
        fail("Cargo.toml workspace.metadata.dist.targets contains duplicates")
    return targets


def one_file(directory: Path, name: str) -> Path:
    matches = [path for path in directory.rglob(name) if path.is_file()]
    if not matches:
        fail(f"missing artifact: {name} under {directory}")
    if len(matches) != 1:
        fail(f"expected one {name} under {directory}, found {len(matches)}")
    return matches[0]


def checksum_line(path: Path, expected_name: str) -> str:
    lines = [line for line in path.read_text(encoding="ascii").splitlines() if line.strip()]
    if len(lines) != 1:
        fail(f"{path.name} must contain exactly one SHA256 line")
    match = CHECKSUM_LINE.fullmatch(lines[0])
    if match is None or match.group(2) != expected_name:
        fail(f"{path.name} must name {expected_name} in sha256sum format")
    return match.group(1).lower()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def asset_for(assets: Any, name: str, target: str, expected_path: str) -> dict[str, Any]:
    if not isinstance(assets, list):
        fail(f"archive for {target} is missing manifest asset declarations")
    asset_id = f"cruise-{target}-exe-{name}"
    matches = [
        asset
        for asset in assets
        if isinstance(asset, dict)
        and asset.get("kind") == "executable"
        and asset.get("name") == name
        and asset.get("id") == asset_id
    ]
    if len(matches) != 1:
        fail(f"archive for {target} must declare exactly one {name} executable")
    asset = matches[0]
    if asset.get("path") != expected_path:
        fail(f"manifest {name} path for {target} must be {expected_path}")
    return asset


def safe_archive_path(name: str) -> tuple[str, ...]:
    if not name or name.startswith("/") or "\\" in name or "\0" in name:
        fail(f"unsafe archive member path: {name!r}")
    parts = tuple(name.split("/"))
    if any(part in ("", ".", "..") for part in parts):
        fail(f"unsafe archive member path: {name!r}")
    return parts


def check_tar_xz(path: Path, target: str, launcher: str, runtime: str) -> None:
    root = f"cruise-{target}"
    seen: set[str] = set()
    root_seen = False
    executables: set[str] = set()
    allowed = {"LICENSE", "README.md", launcher, runtime}
    try:
        with tarfile.open(path, mode="r:xz") as archive:
            for entry in archive:
                parts = safe_archive_path(entry.name.rstrip("/") if entry.isdir() else entry.name)
                if entry.isdir():
                    if parts != (root,) or root_seen:
                        fail(f"{path.name} must contain only the {root}/ directory")
                    root_seen = True
                    continue
                if not entry.isfile() or len(parts) != 2 or parts[0] != root:
                    fail(f"{path.name} contains an unsupported or nested member: {entry.name}")
                filename = parts[1]
                if filename not in allowed or entry.name in seen:
                    fail(f"{path.name} contains an unexpected or duplicate member: {entry.name}")
                seen.add(entry.name)
                if filename in (launcher, runtime):
                    if entry.mode & 0o111 == 0:
                        fail(f"{path.name} does not mark {filename} executable")
                    executables.add(filename)
    except (tarfile.TarError, OSError, EOFError) as error:
        fail(f"invalid tar.xz archive {path.name}: {error}")
    if not root_seen:
        fail(f"{path.name} is missing its top-level {root}/ directory")
    if not {launcher, runtime} <= executables:
        fail(f"{path.name} must contain executable {launcher} and {runtime} in {root}/")


def check_zip(path: Path, launcher: str, runtime: str) -> None:
    seen: set[str] = set()
    executables: set[str] = set()
    allowed = {"LICENSE", "README.md", launcher, runtime}
    try:
        with zipfile.ZipFile(path) as archive:
            for entry in archive.infolist():
                if entry.is_dir() or entry.flag_bits & 1:
                    fail(f"{path.name} must contain unencrypted root-level files")
                parts = safe_archive_path(entry.filename)
                if len(parts) != 1 or parts[0] not in allowed or entry.filename in seen:
                    fail(f"{path.name} contains an unexpected, nested, or duplicate member: {entry.filename}")
                if entry.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
                    fail(f"{path.name} uses unsupported ZIP compression")
                seen.add(entry.filename)
                if parts[0] in (launcher, runtime):
                    executables.add(parts[0])
            corrupt = archive.testzip()
            if corrupt is not None:
                fail(f"{path.name} has a corrupt ZIP member: {corrupt}")
    except (zipfile.BadZipFile, OSError, EOFError) as error:
        fail(f"invalid ZIP archive {path.name}: {error}")
    if not {launcher, runtime} <= executables:
        fail(f"{path.name} must contain root-level {launcher} and {runtime}")


def manifest_data(path: Path) -> tuple[dict[str, Any], set[str]]:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read cargo-dist manifest {path}: {error}")
    if not isinstance(manifest, dict) or not isinstance(manifest.get("artifacts"), dict):
        fail("cargo-dist manifest must contain an artifacts object")
    releases = manifest.get("releases")
    if not isinstance(releases, list):
        fail("cargo-dist manifest must contain a releases list")
    matches = [release for release in releases if isinstance(release, dict) and release.get("app_name") == "cruise"]
    if len(matches) != 1:
        fail("cargo-dist manifest must contain exactly one cruise release")
    release = matches[0]
    names = release.get("artifacts")
    if not isinstance(names, list) or any(not isinstance(name, str) for name in names):
        fail("cruise release must declare its artifact names")
    if len(set(names)) != len(names):
        fail("cruise release contains duplicate artifact names")
    return manifest, set(names)


def check_target(
    target: str, manifest_artifacts: dict[str, Any], release_names: set[str], directory: Path
) -> tuple[str, str]:
    suffix = ".zip" if target.endswith("-windows-msvc") else ".tar.xz"
    archive_name = f"cruise-{target}{suffix}"
    checksum_name = f"{archive_name}.sha256"
    archive_meta = manifest_artifacts.get(archive_name)
    if not isinstance(archive_meta, dict) or archive_meta.get("name") != archive_name:
        fail(f"manifest is missing archive metadata for {target}: {archive_name}")
    if archive_meta.get("kind") != "executable-zip":
        fail(f"manifest archive kind for {target} must be executable-zip")
    triples = archive_meta.get("target_triples")
    if not isinstance(triples, list) or target not in triples:
        fail(f"manifest archive for {target} does not declare its target triple")
    if archive_name not in release_names:
        fail(f"cruise release does not list {archive_name}")

    windows = target.endswith("-windows-msvc")
    launcher = "cruise.exe" if windows else "cruise"
    runtime = "cruise-runtime.exe" if windows else "cruise-runtime"
    assets = archive_meta.get("assets")
    asset_for(assets, "cruise", target, launcher)
    asset_for(assets, "cruise-runtime", target, runtime)

    checksum_meta = manifest_artifacts.get(checksum_name)
    checksum_triples = checksum_meta.get("target_triples") if isinstance(checksum_meta, dict) else None
    if (
        not isinstance(checksum_meta, dict)
        or checksum_meta.get("name") != checksum_name
        or checksum_meta.get("kind") != "checksum"
        or not isinstance(checksum_triples, list)
        or target not in checksum_triples
        or archive_meta.get("checksum") != checksum_name
        or checksum_name not in release_names
    ):
        fail(f"manifest is missing matching checksum metadata for {archive_name}")
    checksums = archive_meta.get("checksums")
    expected_digest = checksums.get("sha256") if isinstance(checksums, dict) else None
    if not isinstance(expected_digest, str) or HEX_SHA256.fullmatch(expected_digest) is None:
        fail(f"manifest has no valid SHA256 for {archive_name}")

    archive_path = one_file(directory, archive_name)
    actual_digest = sha256(archive_path)
    if actual_digest != expected_digest.lower():
        fail(f"manifest SHA256 does not match {archive_name}")
    checksum_path = one_file(directory, checksum_name)
    if checksum_line(checksum_path, archive_name) != actual_digest:
        fail(f"paired checksum does not match {archive_name}")

    if suffix == ".zip":
        check_zip(archive_path, launcher, runtime)
    else:
        check_tar_xz(archive_path, target, launcher, runtime)
    return archive_name, actual_digest


def check_sha256_sum(
    manifest: dict[str, Any], release_names: set[str], directory: Path, digests: dict[str, str]
) -> None:
    metadata = manifest["artifacts"].get("sha256.sum")
    candidates = [path for path in directory.rglob("sha256.sum") if path.is_file()]
    upload_files = manifest.get("upload_files")
    if upload_files is not None and not isinstance(upload_files, list):
        fail("cargo-dist upload_files must be a list when present")
    emitted = (
        any(str(path).replace("\\", "/").rsplit("/", 1)[-1] == "sha256.sum" for path in upload_files)
        if upload_files is not None
        else "sha256.sum" in release_names
    )
    emitted = emitted or bool(candidates)
    if not emitted:
        return
    if (
        not isinstance(metadata, dict)
        or metadata.get("name") != "sha256.sum"
        or metadata.get("kind") != "unified-checksum"
        or "sha256.sum" not in release_names
    ):
        fail("manifest sha256.sum metadata is invalid")
    if len(candidates) != 1:
        fail(f"manifest declares sha256.sum but found {len(candidates)} files")
    lines: dict[str, str] = {}
    for line in candidates[0].read_text(encoding="ascii").splitlines():
        if not line.strip():
            continue
        match = CHECKSUM_LINE.fullmatch(line)
        if match is None:
            fail(f"invalid sha256.sum line: {line!r}")
        digest, name = match.groups()
        if name in lines:
            fail(f"sha256.sum contains duplicate entry for {name}")
        lines[name] = digest.lower()
    for name, expected in digests.items():
        if lines.get(name) != expected:
            fail(f"sha256.sum does not contain the matching digest for {name}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path, help="cargo-dist JSON manifest")
    parser.add_argument("--artifact-dir", required=True, type=Path, help="directory containing built assets")
    parser.add_argument("--target", action="append", help="validate only this configured target (repeatable)")
    args = parser.parse_args()
    try:
        configured_targets = targets_from_cargo_toml()
        targets = args.target or configured_targets
        unknown = sorted(set(targets) - set(configured_targets))
        if unknown:
            fail(f"requested target(s) are not configured in Cargo.toml: {', '.join(unknown)}")
        if len(set(targets)) != len(targets):
            fail("requested target list contains duplicates")
        if not args.artifact_dir.is_dir():
            fail(f"artifact directory does not exist: {args.artifact_dir}")
        manifest, release_names = manifest_data(args.manifest)
        digests: dict[str, str] = {}
        for target in targets:
            name, digest = check_target(target, manifest["artifacts"], release_names, args.artifact_dir)
            digests[name] = digest
            print(f"verified {target}: {name}")
        check_sha256_sum(manifest, release_names, args.artifact_dir, digests)
        return 0
    except (OSError, KeyError, TypeError, ValueError) as error:
        print(f"update artifact check failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
