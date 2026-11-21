#!/usr/bin/env python3
"""Run every Rust test that CI covers, with integration tests under cargo-nextest.

Unit tests and doctests stay on `cargo test`. Integration-test binaries run in
parallel under nextest, except `harness = false` targets: nextest needs libtest's
`--list`, so those run through `cargo test --test`.
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
    integration = "kind(test)"
    if harnessless:
        excluded = " | ".join(f"binary_id(={package}::{name})" for name in harnessless)
        integration += f" and not ({excluded})"
    try:
        run("cargo", "test", "--all-features", "--lib", "--bins")
        run("cargo", "nextest", "run", "--all-features", "-E", integration)
        if harnessless:
            run("cargo", "test", "--all-features", *(arg for name in harnessless for arg in ("--test", name)))
        run("cargo", "test", "--all-features", "--doc")
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
