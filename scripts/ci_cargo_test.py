#!/usr/bin/env python3
"""Run every Rust test that CI covers, in parallel under cargo-nextest.

Doctests stay on `cargo test`, as do `harness = false` targets: nextest needs
libtest's `--list`. Those targets are built by one cargo invocation and their
binaries then run concurrently with the doctests after nextest. Each fresh
`cargo test --test` re-links `target/debug/cruise` (remove, then hard link), so
concurrent cargo invocations would make `CARGO_BIN_EXE_cruise` briefly vanish
under tests that are already running.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


def run(*args: str) -> None:
    print("+", " ".join(args), flush=True)
    subprocess.run(args, check=True)


def run_concurrently(commands: list[list[str]], cwd: Path) -> None:
    running = []
    for command in commands:
        print("+", " ".join(command), flush=True)
        log = tempfile.TemporaryFile()
        running.append((command, subprocess.Popen(command, cwd=cwd, stdout=log, stderr=subprocess.STDOUT), log))
    failure = None
    for command, process, log in running:
        status = process.wait()
        log.seek(0)
        print(f"== {' '.join(command)} (exit {status})", flush=True)
        sys.stdout.buffer.write(log.read())
        log.close()
        sys.stdout.flush()
        if status and failure is None:
            failure = subprocess.CalledProcessError(status, command)
    if failure:
        raise failure


def build_tests(names: list[str]) -> list[str]:
    command = ["cargo", "test", "--all-features", "--no-run", "--message-format=json-render-diagnostics"]
    command += [f"--test={name}" for name in names]
    print("+", " ".join(command), flush=True)
    output = subprocess.run(command, check=True, stdout=subprocess.PIPE, text=True).stdout
    executables = {}
    for line in output.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and "test" in message["target"]["kind"]:
            executables[message["target"]["name"]] = message["executable"]
    return [executables[name] for name in names]


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    package = manifest["package"]["name"]
    harnessless = [
        target["name"]
        for target in manifest.get("test", [])
        if target.get("harness", True) is False and target.get("test", True)
    ]
    filterset = "all()"
    if harnessless:
        filterset = "not (" + " | ".join(f"binary_id(={package}::{name})" for name in harnessless) + ")"
    try:
        run("cargo", "nextest", "run", "--profile", "ci", "--all-features", "-E", filterset)
        executables = build_tests(harnessless) if harnessless else []
        run_concurrently(
            [[executable] for executable in executables] + [["cargo", "test", "--all-features", "--doc"]],
            root,
        )
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
