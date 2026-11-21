#!/usr/bin/env python3
"""Run every Rust test that CI covers, in parallel under cargo-nextest.

Doctests stay on `cargo test`, as do `harness = false` targets: nextest needs
libtest's `--list`. Those `cargo test` runs execute concurrently after nextest.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


def run(*args: str) -> None:
    print("+", " ".join(args), flush=True)
    subprocess.run(args, check=True)


def run_concurrently(commands: list[list[str]]) -> None:
    running = []
    for command in commands:
        print("+", " ".join(command), flush=True)
        log = tempfile.TemporaryFile()
        running.append((command, subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT), log))
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


def main() -> int:
    manifest = tomllib.loads((Path(__file__).resolve().parent.parent / "Cargo.toml").read_text(encoding="utf-8"))
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
        run_concurrently(
            [["cargo", "test", "--all-features", "--test", name] for name in harnessless]
            + [["cargo", "test", "--all-features", "--doc"]]
        )
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
