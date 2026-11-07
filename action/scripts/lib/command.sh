#!/usr/bin/env bash
# Parse stdin using the last trigger mention, writing remaining text to $2.
parse_command() {
  python3 -c '
import re, sys
phrase, path = sys.argv[1:]
body = sys.stdin.read()
pattern = re.compile(r"(?:^|(?<=\s))" + re.escape(phrase) + r"(?=[\s.,!?;:]|$)", re.I)
matches = list(pattern.finditer(body))
if not matches:
    sys.exit(1)
remainder = re.sub(r"^[\s.,!?;:]+", "", body[matches[-1].end():])
command, rest = "run", remainder
first_line, _, tail = remainder.partition("\n")
tokens = first_line.split(None, 1)
if tokens:
    candidate = tokens[0]
    bare = candidate[1:] if candidate.startswith("/") else candidate
    bare = bare.rstrip(".,!?;:").lower()
    if bare in {"run", "exec", "plan", "fix"} or (bare == "tell" and candidate.startswith("/")):
        command = bare
        after = tokens[1] if len(tokens) > 1 else ""
        rest = (after + "\n" + tail) if tail else after
with open(path, "w", encoding="utf-8") as f:
    f.write(rest.strip())
sys.stdout.write(command)
' "$1" "$2"
}
