#!/bin/sh
# Stand-in evidence agent: drives the listed feature through verify_pty and reports.
# ../mode picks: proved | failed | outside
mode=$(cat ../mode 2>/dev/null || echo proved)
dir=$(sed -n 's/^Evidence directory for this run: `\(.*\)`$/\1/p' "$(dirname "$CRUISE_EVIDENCE_PLAN")/artifacts/feature-evidence/plan.md")
mkdir -p "$dir/greet"
verify_pty --out "$PWD/$dir/greet" -- sh src/app.sh cruise >/dev/null
[ "$mode" = outside ] && echo "# edited by the evidence step" >> src/app.sh
status=proved; summary="sh src/app.sh cruise printed hello there, cruise (exit 0)"
[ "$mode" = failed ] && status=failed && summary="sh src/app.sh cruise printed hello there, cruise; the map expects hello, cruise"
printf '{"features":[{"id":"greet","status":"%s","summary":"%s","files":["%s/greet/01-final.txt","%s/greet/01-final.html","%s/greet/result.json"]}]}\n' "$status" "$summary" "$dir" "$dir" "$dir" > "$(dirname "$CRUISE_EVIDENCE_PLAN")/artifacts/feature-evidence/report.json"
cat "$(dirname "$CRUISE_EVIDENCE_PLAN")/artifacts/feature-evidence/report.json"
