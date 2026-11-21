#!/usr/bin/env python3
"""Run every Rust test that CI covers, in parallel under cargo-nextest.

Doctests stay on `cargo test`, as do `harness = false` targets: nextest needs
libtest's `--list`.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path


def run(*args: str) -> None:
    print("+", " ".join(args), flush=True)
    subprocess.run(args, check=True)


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
        if harnessless:
            run("cargo", "test", "--all-features", *(arg for name in harnessless for arg in ("--test", name)))
        run("cargo", "test", "--all-features", "--doc")
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
