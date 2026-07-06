#!/usr/bin/env python3
"""Verify that CONTRIBUTING.md documents the required development workflow.

This test checks that the root CONTRIBUTING.md contains the build, test, lint,
and pre-commit formatting commands, plus a note that the repository uses
squash merge for pull requests.

Usage: python3 scripts/test_contributing.py
"""

from pathlib import Path
import re
import sys

REPO_ROOT = Path(__file__).resolve().parent.parent
CONTRIBUTING = REPO_ROOT / "CONTRIBUTING.md"

PASS = 0
FAIL = 0


def pass_(label: str) -> None:
    global PASS
    print(f"PASS: {label}")
    PASS += 1


def fail_(label: str, reason: str) -> None:
    global FAIL
    print(f"FAIL: {label} -- {reason}")
    FAIL += 1


def main() -> int:
    if not CONTRIBUTING.exists():
        fail_("file exists", f"{CONTRIBUTING} not found")
        return 1
    pass_("file exists")

    content = CONTRIBUTING.read_text(encoding="utf-8")

    # Required commands
    required_commands = [
        ("cargo build", r"(?:^|\s|`)cargo build(?:\s|$|`)"),
        ("cargo test", r"(?:^|\s|`)cargo test(?:\s|$|`)"),
        ("cargo clippy", r"(?:^|\s|`)cargo clippy(?:\s|$|`)"),
        ("cargo fmt --all", r"(?:^|\s|`)cargo fmt --all(?:\s|$|`)"),
    ]
    for label, pattern in required_commands:
        if re.search(pattern, content):
            pass_(f"mentions command: {label}")
        else:
            fail_(f"mentions command: {label}", f"missing `{label}`")

    # Squash-merge note
    if re.search(r"squash\s*merge", content, re.IGNORECASE):
        pass_("mentions squash merge workflow")
    else:
        fail_("mentions squash merge workflow", "missing squash merge note")

    # English-only check: allow printable ASCII and whitespace only.
    non_ascii = [c for c in content if not (c.isascii() and (c.isprintable() or c in "\n\r\t"))]
    if not non_ascii:
        pass_("content is ASCII / English text")
    else:
        fail_("content is ASCII / English text", f"found non-ASCII: {non_ascii[:5]}")

    print(f"\nResults: {PASS} passed, {FAIL} failed")
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
