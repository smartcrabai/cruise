#!/usr/bin/env bash
# Standalone, checksum-pinned Alloy runner. Never trust an extracted cached JDK.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TOOLS="$ROOT/target/formal-tools"
mkdir -p "$TOOLS" "$ROOT/target/formal-reports"
RUN=$(mktemp -d "$ROOT/target/formal-reports/instruction-inbox-alloy.XXXXXX")
echo "Reports: $RUN"
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi; }
fetch() {
  local url=$1 dest=$2 digest=$3 part
  if [[ ! -f "$dest" ]] || [[ $(sha256 "$dest") != "$digest" ]]; then
    part=$(mktemp "$TOOLS/download.XXXXXX")
    if ! curl --fail --location --silent --show-error --retry 3 "$url" -o "$part"; then rm -f "$part"; return 1; fi
    if [[ $(sha256 "$part") != "$digest" ]]; then echo "Checksum mismatch: $url" >&2; rm -f "$part"; return 1; fi
    mv "$part" "$dest"
  fi
}
# Temurin release SHA-256 sidecars; Alloy 6.2.0 release distribution jar.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) asset=OpenJDK25U-jdk_aarch64_mac_hotspot_25.0.5_8.tar.gz; digest=36dc14d797aaff6d89aab302cd053fdba6207dfdcf772052fa7b023a94b0e857; java_subdir=Contents/Home ;;
  Linux-x86_64) asset=OpenJDK25U-jdk_x64_linux_hotspot_25.0.5_8.tar.gz; digest=68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb; java_subdir=. ;;
  *) echo 'Supported platforms: Darwin arm64 and Linux x86_64' >&2; exit 1 ;;
esac
fetch "https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5%2B8/$asset" "$TOOLS/$asset" "$digest"
fetch 'https://github.com/AlloyTools/org.alloytools.alloy/releases/download/v6.2.0/org.alloytools.alloy.dist.jar' "$TOOLS/alloy-6.2.0.jar" 6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d
JDK=$(mktemp -d "$TOOLS/alloy-jdk.XXXXXX")
trap 'rm -rf "$JDK"' EXIT
tar -xzf "$TOOLS/$asset" -C "$JDK" --strip-components=1
JAVA="$JDK/$java_subdir/bin/java"
"$JAVA" -XshowSettings:properties -version > "$RUN/java-version.log" 2>&1
grep -Eq 'java.runtime.version = 25\.0\.5\+8-LTS[[:space:]]*$' "$RUN/java-version.log"
MODEL="$ROOT/tests/formal/instruction_inbox.als"
JAR="$TOOLS/alloy-6.2.0.jar"
"$JAVA" -jar "$JAR" help exec > "$RUN/help-exec.log" 2>&1
"$JAVA" -jar "$JAR" commands "$MODEL" > "$RUN/commands.log" 2>&1
"$JAVA" --enable-native-access=ALL-UNNAMED -jar "$JAR" exec --command '*' --solver sat4j --output "$RUN/output" --type json "$MODEL" 2>&1 | tee "$RUN/exec.log"
node "$ROOT/scripts/formal/check_alloy_receipt.cjs" "$MODEL" "$RUN/output/receipt.json" | tee "$RUN/receipt-check.log"
echo "Alloy checks passed. Reports: $RUN"
