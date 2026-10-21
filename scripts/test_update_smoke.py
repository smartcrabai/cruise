#!/usr/bin/env python3
"""Exercise native update using real, versioned Cruise launcher/runtime binaries.

Build A and B from isolated source copies with debug assertions and the
``test-utils`` feature, then pass the resulting artifacts to this script. The
script intentionally fails when artifacts or test-only seams are missing.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zipfile
from pathlib import Path
from typing import Any


API_PATH = "/repos/smartcrabai/cruise/releases/latest"
RELEASE_TARGETS = {
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
}


def fail(message: str) -> None:
    raise AssertionError(message)


def require_file(path: Path, label: str) -> Path:
    if not path.is_file():
        fail(f"missing {label}: {path}; build real Cruise A/B binaries first")
    return path.resolve()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def file_stamp(path: Path) -> tuple[int, int, int, int, str]:
    info = path.stat()
    return (info.st_size, info.st_mtime_ns, info.st_mode, info.st_ino, sha256_file(path))

def file_metadata(path: Path) -> str:
    try:
        info = path.stat()
    except OSError as error:
        return f"{path}: stat failed: {error}"
    attributes = getattr(info, "st_file_attributes", None)
    # FILE_ATTRIBUTE_READONLY is bit 0 on Windows.
    read_only = None if attributes is None else bool(attributes & 0x1)
    return (
        f"{path}: mode={info.st_mode:#o}, size={info.st_size}, "
        f"mtime_ns={info.st_mtime_ns}, file_attributes={attributes!r}, "
        f"read_only={read_only}"
    )


def tree_snapshot(root: Path) -> dict[str, tuple[str, str | None, int, int, int, int]]:
    """Record path identity, contents, permissions, and timestamps."""
    if not root.exists():
        return {}
    snapshot: dict[str, tuple[str, str | None, int, int, int, int]] = {}
    for path in (root, *sorted(root.rglob("*"))):
        info = path.lstat()
        if path.is_symlink():
            kind, content = "symlink", os.readlink(path)
        elif path.is_dir():
            kind, content = "directory", None
        elif path.is_file():
            kind, content = "file", sha256_file(path)
        else:
            kind, content = "other", None
        relative = "." if path == root else path.relative_to(root).as_posix()
        snapshot[relative] = (
            kind,
            content,
            info.st_size,
            info.st_mtime_ns,
            info.st_mode,
            info.st_ino,
        )
    return snapshot


def persistent_snapshot(root: Path) -> dict[str, tuple]:
    return {
        name: record if record[0] != "directory" else ("directory",)
        for name, record in tree_snapshot(root).items()
    }



def validate_native_executable(path: Path, target: str, label: str) -> None:
    with path.open("rb") as stream:
        magic = stream.read(4)
    if target == "x86_64-pc-windows-msvc":
        valid = magic[:2] == b"MZ"
    elif target.endswith("-apple-darwin"):
        valid = magic in {
            b"\xfe\xed\xfa\xce",
            b"\xce\xfa\xed\xfe",
            b"\xfe\xed\xfa\xcf",
            b"\xcf\xfa\xed\xfe",
            b"\xca\xfe\xba\xbe",
            b"\xbe\xba\xfe\xca",
        }
    else:
        valid = magic == b"\x7fELF"
    if not valid:
        fail(f"{label} is not a compiled native {target} executable: {path}")


def validate_stable_versions(current: str, next_version: str) -> None:
    pattern = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
    current_match = pattern.fullmatch(current)
    next_match = pattern.fullmatch(next_version)
    if current_match is None or next_match is None:
        fail("versions must be stable X.Y.Z values (without a leading v)")
    if tuple(map(int, next_match.groups())) <= tuple(map(int, current_match.groups())):
        fail(f"next version {next_version} must be newer than current {current}")


def workspace_version(manifest: Path) -> str:
    text = manifest.read_text(encoding="utf-8")
    section = re.search(r"(?ms)^\[workspace\.package\]\s*(.*?)(?=^\[|\Z)", text)
    if section is None:
        fail(f"missing [workspace.package] in {manifest}")
    version = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', section.group(1))
    if version is None:
        fail(f"missing workspace package version in {manifest}")
    return version.group(1)


def set_workspace_version(manifest: Path, version: str) -> None:
    lines = manifest.read_text(encoding="utf-8").splitlines(keepends=True)
    section_start = next(
        (index for index, line in enumerate(lines) if line.strip() == "[workspace.package]"),
        None,
    )
    if section_start is None:
        fail(f"missing [workspace.package] in {manifest}")
    for index in range(section_start + 1, len(lines)):
        if lines[index].lstrip().startswith("["):
            break
        match = re.match(r'^(\s*version\s*=\s*)"[^"]+"(.*)$', lines[index].rstrip("\r\n"))
        if match:
            newline = "\r\n" if lines[index].endswith("\r\n") else "\n" if lines[index].endswith("\n") else ""
            lines[index] = f'{match.group(1)}"{version}"{match.group(2)}{newline}'
            manifest.write_text("".join(lines), encoding="utf-8")
            return
    fail(f"missing workspace package version in {manifest}")


def copy_source_tree(source: Path, destination: Path) -> None:
    ignored = shutil.ignore_patterns(
        ".git",
        "target",
        "node_modules",
        ".venv",
        ".direnv",
        "dist",
        "build",
        "__pycache__",
    )
    shutil.copytree(source, destination, ignore=ignored)


def native_target() -> str:
    rustc = shutil.which("rustc")
    if rustc is None:
        fail("rustc is not on PATH; install the stable Rust toolchain")
    try:
        result = subprocess.run(
            [rustc, "--version", "--verbose"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        fail(f"could not inspect the Rust host target: {error}")
    if result.returncode != 0:
        fail(f"rustc target query failed: {result.stderr}")
    for line in result.stdout.splitlines():
        if line.startswith("host: "):
            return line.partition(": ")[2]
    fail("rustc --version --verbose did not report a host target")


def previous_patch(version: str) -> str:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
    if match is None:
        fail(f"source workspace version must be stable X.Y.Z, got {version!r}")
    major, minor, patch = map(int, match.groups())
    if patch:
        return f"{major}.{minor}.{patch - 1}"
    if minor:
        return f"{major}.{minor - 1}.99"
    if major:
        return f"{major - 1}.99.99"
    fail(f"cannot derive a lower version from {version}")


def build_and_smoke(args: argparse.Namespace) -> None:
    repo = Path(args.repo).resolve()
    manifest = repo / "Cargo.toml"
    if not manifest.is_file():
        fail(f"source repository has no Cargo.toml: {repo}")
    next_version = args.next_version or workspace_version(manifest)
    if next_version != workspace_version(manifest):
        fail(
            f"B must match the source workspace version "
            f"({workspace_version(manifest)}), got {next_version}"
        )
    current_version = args.current_version or previous_patch(next_version)
    validate_stable_versions(current_version, next_version)
    target = native_target()
    if args.target and args.target != target:
        fail(f"--build creates host binaries for {target}, not requested target {args.target}")

    if args.output_dir:
        output = Path(args.output_dir).resolve()
    else:
        output = Path(tempfile.mkdtemp(prefix="cruise-update-build-")).resolve()
    try:
        output.relative_to(repo)
    except ValueError:
        pass
    else:
        fail("--output-dir must be outside the source repository")
    output.mkdir(parents=True, exist_ok=True)
    source_a = output / "source-a"
    source_b = output / "source-b"
    if source_a.exists() or source_b.exists():
        fail(f"build output already contains source copies: {output}")

    copy_source_tree(repo, source_a)
    copy_source_tree(repo, source_b)
    set_workspace_version(source_a / "Cargo.toml", current_version)
    set_workspace_version(source_b / "Cargo.toml", next_version)
    target_dir = Path(args.cargo_target_dir or os.environ.get("CARGO_TARGET_DIR", repo / "target")).resolve()
    build_env = os.environ.copy()
    build_env["CARGO_TARGET_DIR"] = str(target_dir)
    build_env.pop("CARGO_BUILD_TARGET", None)
    cargo = args.cargo
    binary_dir = target_dir / "debug"
    extension = ".exe" if os.name == "nt" else ""
    source_launcher = binary_dir / f"cruise{extension}"
    source_runtime = binary_dir / f"cruise-runtime{extension}"
    artifact_a = output / "A"
    artifact_b = output / "B"
    artifact_a.mkdir()
    artifact_b.mkdir()

    for version, source in ((current_version, source_a), (next_version, source_b)):
        print(f"Building real Cruise {version} binaries from isolated source {source}")
        try:
            result = subprocess.run(
                [
                    cargo,
                    "build",
                    "--features",
                    "test-utils",
                    "--bin",
                    "cruise",
                    "--bin",
                    "cruise-runtime",
                ],
                cwd=source,
                env=build_env,
                check=False,
            )
        except OSError as error:
            fail(f"could not run Cargo build: {error}")
        if result.returncode != 0:
            fail(f"building Cruise {version} failed with exit {result.returncode}")
        if version == current_version:
            shutil.copy2(source_launcher, artifact_a / launcher_name())
            shutil.copy2(source_runtime, artifact_a / runtime_name())
        else:
            shutil.copy2(source_runtime, artifact_b / runtime_name())

    print(f"Running real-binary smoke with artifacts in {output}")
    smoke(
        argparse.Namespace(
            launcher=artifact_a / launcher_name(),
            runtime=artifact_a / runtime_name(),
            next_runtime=artifact_b / runtime_name(),
            current_version=current_version,
            next_version=next_version,
            target=target,
        )
    )

def run_process(
    argv: list[str | os.PathLike[str]],
    env: dict[str, str],
    *,
    timeout: int = 180,
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            [os.fspath(argument) for argument in argv],
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        fail(f"could not run {argv[0]}: {error}")


def combined_output(result: subprocess.CompletedProcess[str]) -> str:
    return f"{result.stdout}{result.stderr}"


def expect_success(result: subprocess.CompletedProcess[str], description: str) -> None:
    if result.returncode != 0:
        fail(f"{description} exited {result.returncode}:\n{combined_output(result)}")


def expect_failure(result: subprocess.CompletedProcess[str], description: str) -> None:
    if result.returncode == 0:
        fail(f"{description} unexpectedly succeeded:\n{combined_output(result)}")


def assert_version(launcher: Path, env: dict[str, str], version: str, description: str) -> None:
    result = run_process([launcher, "--version"], env)
    expect_success(result, description)
    if version not in combined_output(result):
        fail(f"{description} did not report {version}:\n{combined_output(result)}")


def runtime_name() -> str:
    return "cruise-runtime.exe" if os.name == "nt" else "cruise-runtime"


def launcher_name() -> str:
    return "cruise.exe" if os.name == "nt" else "cruise"


def isolated_env(
    home: Path,
    cargo_home: Path,
    state_home: Path,
    fixture_base: str,
    *,
    extra: dict[str, str] | None = None,
) -> dict[str, str]:
    env = os.environ.copy()
    for key in tuple(env):
        if key.upper().startswith("CRUISE_"):
            env.pop(key, None)
    env.update(
        {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "CARGO_HOME": str(cargo_home),
            "XDG_STATE_HOME": str(state_home),
            "APPDATA": str(home / "AppData" / "Roaming"),
            "LOCALAPPDATA": str(home / "AppData" / "Local"),
            "CRUISE_UPDATE_TEST_BASE_URL": fixture_base,
        }
    )
    if extra:
        env.update(extra)
    return env


def assert_test_capabilities(runtimes: tuple[tuple[Path, str], ...]) -> None:
    with tempfile.TemporaryDirectory(prefix="cruise-update-capabilities-") as temporary:
        root = Path(temporary)
        home = root / "home"
        env = os.environ.copy()
        for key in tuple(env):
            if key.upper().startswith("CRUISE_"):
                env.pop(key, None)
        env.update(
            {
                "HOME": str(home),
                "USERPROFILE": str(home),
                "CARGO_HOME": str(root / "cargo-home"),
                "XDG_STATE_HOME": str(root / "state-home"),
                "APPDATA": str(home / "AppData" / "Roaming"),
                "LOCALAPPDATA": str(home / "AppData" / "Local"),
                "CRUISE_UPDATE_TEST_CAPABILITIES": "1",
            }
        )
        expected = "cruise-update-test-utils-v1\n"
        for runtime, label in runtimes:
            result = run_process([runtime, "update", "--status"], env)
            if result.returncode != 0 or result.stdout != expected:
                fail(
                    f"{label} did not confirm the update smoke test capability "
                    "handshake; rebuild A and B with `python scripts/test_update_smoke.py "
                    "--build` (debug assertions and test-utils are required). "
                    f"Expected exit 0 and {expected!r}, got exit {result.returncode}, "
                    f"stdout={result.stdout!r}, stderr={result.stderr!r}"
                )


def copy_pair(
    source_launcher: Path,
    source_runtime: Path,
    install_bin: Path,
) -> tuple[Path, Path]:
    install_bin.mkdir(parents=True, exist_ok=True)
    launcher = install_bin / launcher_name()
    runtime = install_bin / runtime_name()
    shutil.copy2(source_launcher, launcher)
    shutil.copy2(source_runtime, runtime)
    return launcher, runtime


def make_tar_xz(source_runtime: Path, archive: Path, target: str) -> None:
    with tarfile.open(archive, "w:xz") as output:
        mtime = int(source_runtime.stat().st_mtime)
        root = tarfile.TarInfo(f"cruise-{target}")
        root.type = tarfile.DIRTYPE
        root.mode = 0o755
        root.mtime = mtime
        output.addfile(root)
        info = tarfile.TarInfo(f"cruise-{target}/{runtime_name()}")
        info.size = source_runtime.stat().st_size
        info.mode = 0o755
        info.mtime = mtime
        with source_runtime.open("rb") as stream:
            output.addfile(info, stream)


def make_zip(source_runtime: Path, archive: Path) -> None:
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
        output.write(source_runtime, arcname=runtime_name())


class FixtureState:
    def __init__(self, root: Path, target: str, next_runtime: Path, version: str):
        self.root = root
        self.target = target
        self.version = version
        self.archive_name = f"cruise-{target}." + ("zip" if target.endswith("windows-msvc") else "tar.xz")
        self.archive_path = root / self.archive_name
        if target.endswith("windows-msvc"):
            make_zip(next_runtime, self.archive_path)
        else:
            make_tar_xz(next_runtime, self.archive_path, target)
        self.archive_digest = sha256_file(self.archive_path)
        self.checksum = self.archive_digest
        self.checksum_name = f"{self.archive_name}.sha256"
        self.checksum_path = root / self.checksum_name
        self.checksum_path.write_text(f"{self.archive_digest}  {self.archive_name}\n", encoding="ascii")

    @property
    def release_path(self) -> str:
        return f"/smartcrabai/cruise/releases/download/v{self.version}"

    @property
    def manifest_path(self) -> str:
        return f"{self.release_path}/dist-manifest.json"

    @property
    def archive_url_path(self) -> str:
        return f"{self.release_path}/{self.archive_name}"

    @property
    def checksum_url_path(self) -> str:
        return f"{self.release_path}/{self.checksum_name}"

    def manifest(self) -> dict[str, Any]:
        executable_path = runtime_name()
        checksum_name = self.checksum_name
        return {
            "announcement_tag": f"v{self.version}",
            "announcement_is_prerelease": False,
            "releases": [
                {
                    "app_name": "cruise",
                    "app_version": self.version,
                    "artifacts": [self.archive_name, checksum_name],
                }
            ],
            "artifacts": {
                self.archive_name: {
                    "name": self.archive_name,
                    "kind": "executable-zip",
                    "target_triples": [self.target],
                    "assets": [
                        {
                            "id": f"cruise-{self.target}-exe-cruise-runtime",
                            "name": "cruise-runtime",
                            "path": executable_path,
                            "kind": "executable",
                        }
                    ],
                    "checksum": checksum_name,
                    "checksums": {"sha256": self.checksum},
                },
                checksum_name: {
                    "name": checksum_name,
                    "kind": "checksum",
                    "target_triples": [self.target],
                },
            },
        }

    def release(self, base_url: str) -> dict[str, Any]:
        return {
            "tag_name": f"v{self.version}",
            "prerelease": False,
            "draft": False,
            "assets": [
                {
                    "id": 1,
                    "name": "dist-manifest.json",
                    "browser_download_url": base_url + self.manifest_path,
                },
                {
                    "id": 2,
                    "name": self.archive_name,
                    "browser_download_url": base_url + self.archive_url_path,
                },
                {
                    "id": 3,
                    "name": self.checksum_name,
                    "browser_download_url": base_url + self.checksum_url_path,
                },
            ],
        }


class FixtureHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:  # noqa: N802 - stdlib handler contract
        state: FixtureState = self.server.fixture_state  # type: ignore[attr-defined]
        base_url: str = self.server.fixture_base  # type: ignore[attr-defined]
        self.server.record_request(self.path)  # type: ignore[attr-defined]
        if self.path == API_PATH:
            body = json.dumps(state.release(base_url)).encode()
            self.send_bytes(body, "application/json")
        elif self.path == state.manifest_path:
            body = json.dumps(state.manifest()).encode()
            self.send_bytes(body, "application/json")
        elif self.path == state.checksum_url_path:
            self.send_file(state.checksum_path, "text/plain")
        elif self.path == state.archive_url_path:
            self.send_file(state.archive_path, "application/octet-stream")
        else:
            self.send_error(404, "fixture path not found")

    def send_bytes(self, body: bytes, content_type: str) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_file(self, path: Path, content_type: str) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(path.stat().st_size))
        self.end_headers()
        with path.open("rb") as stream:
            shutil.copyfileobj(stream, self.wfile, length=1024 * 1024)

    def log_message(self, _format: str, *_args: object) -> None:
        return


class FixtureServer:
    def __init__(self, state: FixtureState):
        self.state = state
        self.requests: list[str] = []
        self.requests_lock = threading.Lock()
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
        self.server.fixture_state = state  # type: ignore[attr-defined]
        self.server.record_request = self.record_request  # type: ignore[attr-defined]
        self.port = self.server.server_address[1]
        self.base_url = f"http://127.0.0.1:{self.port}"
        self.server.fixture_base = self.base_url  # type: ignore[attr-defined]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def record_request(self, path: str) -> None:
        with self.requests_lock:
            self.requests.append(path)

    def request_count(self) -> int:
        with self.requests_lock:
            return len(self.requests)

    def requests_since(self, count: int) -> list[str]:
        with self.requests_lock:
            return self.requests[count:]

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


class Install:
    def __init__(
        self,
        root: Path,
        source_launcher: Path,
        source_runtime: Path,
        fixture: FixtureServer,
        *,
        managed: bool,
    ):
        self.root = root
        self.cargo_home = root / "cargo-home"
        self.state_home = root / "state-home"
        self.home = root / "home"
        self.home.mkdir(parents=True)
        self.cargo_home.mkdir(parents=True)
        self.state_home.mkdir(parents=True)
        self.bin = self.cargo_home / "bin" if managed else root / "external" / "bin"
        self.launcher, self.runtime = copy_pair(source_launcher, source_runtime, self.bin)
        self.env = isolated_env(self.home, self.cargo_home, self.state_home, fixture.base_url)
        self.state_dir = self.state_home / "cruise"


def assert_uses_fixture(fixture: FixtureServer, before: int, *, needs_archive: bool) -> None:
    requests = fixture.requests_since(before)
    if API_PATH not in requests:
        fail("no request reached the loopback release API; rebuild with debug assertions and test-utils")
    if needs_archive and not any(path.endswith("/" + fixture.state.archive_name) for path in requests):
        fail("the loopback runtime archive was not downloaded")


def assert_gate_failure(
    result: subprocess.CompletedProcess[str],
    description: str,
    diagnostic: str,
) -> None:
    expect_failure(result, description)
    if diagnostic.casefold() not in combined_output(result).casefold():
        fail(f"{description} did not report {diagnostic!r}:\n{combined_output(result)}")


def run_update(install: Install, fixture: FixtureServer, *, extra: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    env = dict(install.env)
    if extra:
        env.update(extra)
    return run_process([install.launcher, "update"], env)


def smoke(args: argparse.Namespace) -> None:
    launcher = require_file(Path(args.launcher), "A launcher")
    runtime = require_file(Path(args.runtime), "A runtime")
    next_runtime = require_file(Path(args.next_runtime), "B runtime")
    validate_stable_versions(args.current_version, args.next_version)
    if args.target not in RELEASE_TARGETS:
        fail(f"unsupported release target: {args.target!r}")
    if (
        (args.target == "x86_64-pc-windows-msvc" and os.name != "nt")
        or (args.target.endswith("-apple-darwin") and sys.platform != "darwin")
        or ("-unknown-linux-gnu" in args.target and not sys.platform.startswith("linux"))
    ):
        fail(f"release target {args.target} is not runnable on this host ({sys.platform})")
    for path, label in (
        (launcher, "A launcher"),
        (runtime, "A runtime"),
        (next_runtime, "B runtime"),
    ):
        validate_native_executable(path, args.target, label)

    base_env = os.environ.copy()
    for key in tuple(base_env):
        if key.upper().startswith("CRUISE_"):
            base_env.pop(key, None)
    version_env = dict(base_env)
    for path, version, label in (
        (runtime, args.current_version, "A runtime"),
        (next_runtime, args.next_version, "B runtime"),
        (launcher, args.current_version, "A launcher"),
    ):
        result = run_process([path, "--version"], version_env)
        expect_success(result, f"{label} version probe")
        if version not in combined_output(result):
            fail(f"{label} is not version {version}:\n{combined_output(result)}")

    assert_test_capabilities(((runtime, "A runtime"), (next_runtime, "B runtime")))

    with tempfile.TemporaryDirectory(prefix="cruise-update-smoke-") as temporary:
        root = Path(temporary)
        fixture_state = FixtureState(root, args.target, next_runtime, args.next_version)
        fixture = FixtureServer(fixture_state)
        try:
            happy_root = root / "happy"
            happy_root.mkdir()
            happy = Install(happy_root, launcher, runtime, fixture, managed=True)
            fixed_before = {
                happy.launcher: file_stamp(happy.launcher),
                happy.runtime: file_stamp(happy.runtime),
            }
            assert_version(happy.launcher, happy.env, args.current_version, "launcher before update")

            request_start = fixture.request_count()
            updated = run_update(happy, fixture)
            expect_success(updated, "managed update A to B")
            assert_uses_fixture(fixture, request_start, needs_archive=True)
            assert_version(happy.launcher, happy.env, args.next_version, "same launcher after update")
            if {path: file_stamp(path) for path in fixed_before} != fixed_before:
                fail("native update modified the fixed launcher or sibling runtime")
            active_pointer = happy.bin / ".cruise-runtime-active"
            if not active_pointer.is_file():
                fail("successful update did not activate a runtime through the fixed launcher")
            active_name = active_pointer.read_text(encoding="utf-8").strip()
            if not active_name or Path(active_name).name != active_name or ".." in active_name:
                fail(f"active runtime pointer is not a safe basename: {active_name!r}")
            active_runtime = happy.bin / active_name
            if not active_runtime.is_file():
                fail(f"selected runtime does not exist: {active_runtime}")
            selected_stamp = file_stamp(active_runtime)
            bin_after_update = persistent_snapshot(happy.bin)
            state_after_update = persistent_snapshot(happy.state_dir)

            request_start = fixture.request_count()
            no_op = run_update(happy, fixture)
            expect_success(no_op, "same-version no-op update")
            assert_uses_fixture(fixture, request_start, needs_archive=False)
            if persistent_snapshot(happy.bin) != bin_after_update or persistent_snapshot(happy.state_dir) != state_after_update:
                fail("same-version update changed the installed files or transaction state")
            assert_version(happy.launcher, happy.env, args.next_version, "launcher after no-op")

            # Simulate a fixed-pair reinstall with the same A runtime bytes but a
            # fresh file identity/mtime; the overlay must fall back without deletion.
            replacement = happy.bin / f".{runtime_name()}.reinstall"
            shutil.copy2(runtime, replacement)
            changed_mtime = max(time.time_ns(), happy.runtime.stat().st_mtime_ns + 5_000_000_000)
            os.utime(replacement, ns=(changed_mtime, changed_mtime))
            try:
                os.replace(replacement, happy.runtime)
            except OSError as error:
                details = "\n".join(
                    file_metadata(path)
                    for path in (runtime, replacement, happy.runtime)
                )
                fail(
                    f"fixed-pair reinstall could not replace {happy.runtime}: {error}\n"
                    f"File metadata after the failed replacement:\n{details}"
                )
            assert_version(happy.launcher, happy.env, args.current_version, "launcher after fixed-pair reinstall")
            if not active_runtime.is_file() or file_stamp(active_runtime) != selected_stamp:
                fail("fixed-pair fallback deleted or modified the retained overlay runtime")
            if active_pointer.read_text(encoding="utf-8").strip() != active_name:
                fail("fixed-pair fallback mutated the active pointer")

            digest_root = root / "digest-failure"
            digest_root.mkdir()
            digest = Install(digest_root, launcher, runtime, fixture, managed=True)
            fixture.state.checksum = "0" * 64
            digest_pair_before = {digest.launcher: file_stamp(digest.launcher), digest.runtime: file_stamp(digest.runtime)}
            request_start = fixture.request_count()
            failed_digest = run_update(digest, fixture)
            expect_failure(failed_digest, "archive digest mismatch")
            assert_uses_fixture(fixture, request_start, needs_archive=True)
            assert_version(digest.launcher, digest.env, args.current_version, "launcher after digest failure")
            if (digest.bin / ".cruise-runtime-active").exists():
                fail("digest failure changed the previously empty active selection")
            if {path: file_stamp(path) for path in digest_pair_before} != digest_pair_before:
                fail("digest failure modified the fixed launcher/runtime pair")

            fixture.state.checksum = fixture.state.archive_digest
            rollback_root = root / "postprobe-rollback"
            rollback_root.mkdir()
            rollback = Install(rollback_root, launcher, runtime, fixture, managed=True)
            rollback_pair_before = {rollback.launcher: file_stamp(rollback.launcher), rollback.runtime: file_stamp(rollback.runtime)}
            request_start = fixture.request_count()
            failed_probe = run_update(
                rollback,
                fixture,
                extra={"CRUISE_UPDATE_TEST_FAIL_POSTPROBE": "1"},
            )
            expect_failure(failed_probe, "injected launcher postprobe failure")
            assert_uses_fixture(fixture, request_start, needs_archive=True)
            assert_version(rollback.launcher, rollback.env, args.current_version, "launcher after postprobe rollback")
            if (rollback.bin / ".cruise-runtime-active").exists():
                fail("postprobe rollback did not restore the original absent active pointer")
            if {path: file_stamp(path) for path in rollback_pair_before} != rollback_pair_before:
                fail("postprobe rollback modified the fixed launcher/runtime pair")

            for checkpoint in ("ready", "activating", "committed", "completed"):
                checkpoint_root = root / f"checkpoint-{checkpoint}"
                checkpoint_root.mkdir()
                interrupted = Install(checkpoint_root, launcher, runtime, fixture, managed=True)
                before = fixture.request_count()
                stopped = run_update(
                    interrupted,
                    fixture,
                    extra={"CRUISE_UPDATE_TEST_STOP_AFTER": checkpoint},
                )
                if stopped.returncode != 86:
                    fail(
                        f"checkpoint {checkpoint!r} did not stop with exit 86 "
                        f"(got {stopped.returncode}):\n{combined_output(stopped)}"
                    )
                assert_uses_fixture(fixture, before, needs_archive=True)
                expected_at_stop = args.next_version if checkpoint in ("committed", "completed") else args.current_version
                assert_version(interrupted.launcher, interrupted.env, expected_at_stop, f"launcher after {checkpoint} interruption")
                recovered = run_update(interrupted, fixture)
                expect_success(recovered, f"recovery after {checkpoint} interruption")
                assert_version(interrupted.launcher, interrupted.env, args.next_version, f"launcher after {checkpoint} recovery")

            external_root = root / "external-gate"
            external_root.mkdir()
            external = Install(external_root, launcher, runtime, fixture, managed=False)
            external_before = tree_snapshot(external.root)
            external_state_before = tree_snapshot(external.state_home)
            before = fixture.request_count()
            rejected_external = run_update(external, fixture)
            assert_gate_failure(rejected_external, "external install gate", "original distribution")
            if fixture.request_count() != before:
                fail("external update gate contacted the release fixture")
            if tree_snapshot(external.root) != external_before or tree_snapshot(external.state_home) != external_state_before:
                fail("external update gate mutated install or state files")

            override_root = root / "override-gate"
            override_root.mkdir()
            override = Install(override_root, launcher, runtime, fixture, managed=True)
            override_before = tree_snapshot(override.root)
            override_state_before = tree_snapshot(override.state_home)
            before = fixture.request_count()
            rejected_override = run_update(
                override,
                fixture,
                extra={"CRUISE_RUNTIME": str(override.runtime)},
            )
            assert_gate_failure(rejected_override, "CRUISE_RUNTIME override gate", "cruise_runtime")
            if fixture.request_count() != before:
                fail("CRUISE_RUNTIME override gate contacted the release fixture")
            if tree_snapshot(override.root) != override_before or tree_snapshot(override.state_home) != override_state_before:
                fail("CRUISE_RUNTIME override gate mutated install or state files")

            print(
                "PASS: real Cruise launcher/runtime update smoke "
                f"({args.current_version} -> {args.next_version}, {args.target}); "
                "activation, no-op, digest failure, rollback, four recovery checkpoints, "
                "reinstallation fallback, external and override gates"
            )
        finally:
            fixture.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", action="store_true", help="build A/B in isolated source copies, then run the smoke")
    parser.add_argument("--repo", default=Path(__file__).resolve().parent.parent, help="repository source to copy in --build mode")
    parser.add_argument("--output-dir", help="artifact/source-copy output directory in --build mode")
    parser.add_argument("--cargo-target-dir", help="shared Cargo cache directory for both source copies")
    parser.add_argument("--cargo", default="cargo", help="Cargo executable for --build mode")
    parser.add_argument("--launcher", help="compiled A cruise launcher")
    parser.add_argument("--runtime", help="compiled A cruise-runtime")
    parser.add_argument("--next-runtime", help="compiled newer B cruise-runtime")
    parser.add_argument("--current-version", help="stable version compiled into A, e.g. 0.2.15")
    parser.add_argument("--next-version", help="stable version compiled into B, e.g. 0.2.16")
    parser.add_argument("--target", help="Rust release target triple for B (or host target with --build)")
    args = parser.parse_args()
    try:
        if args.build:
            build_and_smoke(args)
        else:
            missing = [
                option
                for option in ("launcher", "runtime", "next_runtime", "current_version", "next_version", "target")
                if getattr(args, option) is None
            ]
            if missing:
                parser.error("without --build, these options are required: " + ", ".join("--" + name.replace("_", "-") for name in missing))
            smoke(args)
    except AssertionError as error:
        print(f"update smoke failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
