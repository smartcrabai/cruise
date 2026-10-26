#!/usr/bin/env bash
# Install only checksum-pinned tools. Source target/formal-tools/env.sh afterwards.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TOOLS="$ROOT/target/formal-tools"
mkdir -p "$TOOLS"
if [[ $(node --version) != v22.23.3 ]]; then
  echo 'Node 22.23.3 is required (CI uses actions/setup-node).' >&2
  exit 1
fi
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}
fetch() {
  local url=$1 dest=$2 expected=$3
  if [[ ! -f "$dest" ]] || [[ $(sha256 "$dest") != "$expected" ]]; then
    curl --fail --location --silent --show-error --retry 3 "$url" -o "$dest.part"
    if [[ $(sha256 "$dest.part") != "$expected" ]]; then
      echo "SHA256 mismatch: $url" >&2
      rm -f "$dest.part"
      return 1
    fi
    mv "$dest.part" "$dest"
  fi
}
# Hashes independently retrieved from the release's .tar.gz.sha256.txt sidecars.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    asset=OpenJDK25U-jdk_aarch64_mac_hotspot_25.0.5_8.tar.gz
    digest=36dc14d797aaff6d89aab302cd053fdba6207dfdcf772052fa7b023a94b0e857
    java_subdir=Contents/Home
    ;;
  Linux-x86_64)
    asset=OpenJDK25U-jdk_x64_linux_hotspot_25.0.5_8.tar.gz
    digest=68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb
    java_subdir=.
    ;;
  *) echo 'Supported formal platforms: Darwin aarch64 and Linux x64' >&2; exit 1 ;;
esac
fetch "https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5%2B8/$asset" "$TOOLS/$asset" "$digest"
# Always extract the verified archive rather than trusting a stale executable.
mkdir -p "$TOOLS/jdk-25.0.5+8"
tar -xzf "$TOOLS/$asset" -C "$TOOLS/jdk-25.0.5+8" --strip-components=1
JAVA_HOME="$TOOLS/jdk-25.0.5+8/$java_subdir"
export JAVA_HOME
export PATH="$JAVA_HOME/bin:$PATH"
java -version 2>&1 | tee "$TOOLS/java-version.log"
java -XshowSettings:properties -version 2>&1 | grep -Eq 'java.runtime.version = 25\.0\.5\+8-LTS[[:space:]]*$'
fetch 'https://github.com/AlloyTools/org.alloytools.alloy/releases/download/v6.2.0/org.alloytools.alloy.dist.jar' "$TOOLS/alloy-6.2.0.jar" 6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d
# The lockfile overrides Quint's adm-zip to 0.6.1 to fix ZIP extraction advisories.
npm ci --prefix "$ROOT/tests/formal" --ignore-scripts
[[ $("$ROOT/tests/formal/node_modules/.bin/quint" --version) == 0.33.0 ]]
# Bash-escaped paths work even when the repository path contains spaces.
# shellcheck disable=SC2016 # PATH must expand when env.sh is sourced, not now.
printf 'export JAVA_HOME=%q\nexport PATH=%q:$PATH\n' "$JAVA_HOME" "$JAVA_HOME/bin" > "$TOOLS/env.sh"
echo 'Formal tools ready. Source target/formal-tools/env.sh before invoking tools directly.'
