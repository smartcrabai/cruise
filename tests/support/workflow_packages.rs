//! Shared offline GitHub API fixture for workflow package integration tests.

use std::path::Path;

const GH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
args="$*"
sha="$(cat "$D/sha")"
base="$D/files"
[ ! -d "$base/$sha" ] || base="$base/$sha"
case "$args" in
  *contents/*)
    path="${args#*contents/}"; path="${path%%\?*}"
    ref="${args##*ref=}"; ref="${ref%% *}"
    [ "$ref" = "$sha" ] || { echo "contents requested with unpinned ref $ref" >&2; exit 1; }
    f="$base/$path"
    [ -f "$f" ] || { echo "not found: $path" >&2; exit 1; }
    base64 < "$f" | tr -d '\n'; echo ;;
  *commits/*)
    case "$args" in *commit.tree.sha*) echo "$sha $sha" ;; *) echo "$sha" ;; esac ;;
  *git/trees/*)
    jq="$4"
    path="${jq#*select(.path == \"}"
    path="${path%%\"*}"
    mode="$D/modes/$sha/$path"
    if [ -f "$mode" ]; then cat "$mode"; echo
    elif [ -f "$base/$path" ]; then echo 'blob 100644'
    fi ;;
  *default_branch*) echo main ;;
  *) echo "unexpected: $args" >&2; exit 1 ;;
esac
"#;

pub fn install_gh(root: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("fixture setup failed: {e}"));
    let gh = bin.join("gh");
    std::fs::write(&gh, GH).unwrap_or_else(|e| panic!("fixture setup failed: {e}"));
    std::fs::set_permissions(gh, std::fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("fixture setup failed: {e}"));
}
