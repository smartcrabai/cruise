#!/usr/bin/env bash
# Publish one evidence directory to the orphan branch `cruise-evidence` without
# touching the checkout, index, or current branch, and print a Markdown link per
# file. Usage: publish.sh EVIDENCE_DIR DEST_PREFIX
#   EVIDENCE_DIR  e.g. target/verify-cruise/20261126-173000
#   DEST_PREFIX   path inside the branch, e.g. pr/verify-cruise-skill/20261126-173000
# Refuses when a text artifact matches a credential pattern. Screenshots cannot
# be scanned: capture only fixture data from an isolated run.
set -euo pipefail
src=${1:?usage: publish.sh EVIDENCE_DIR DEST_PREFIX}
dest=${2:?usage: publish.sh EVIDENCE_DIR DEST_PREFIX}
[[ -d "$src" ]] || { echo "publish.sh: $src is not a directory" >&2; exit 2; }
[[ "$dest" =~ ^[A-Za-z0-9._/-]+$ && "$dest" != /* && "$dest" != *..* ]] || { echo "publish.sh: unsafe DEST_PREFIX $dest" >&2; exit 2; }
branch=cruise-evidence
pattern='nonce=[0-9a-f]|ghp_|gho_|ghs_|github_pat_|glpat-|xox[abp]-|sk-[A-Za-z0-9]{20}|Bearer [A-Za-z0-9]|"token" *: *"'
if grep -rIlE "$pattern" "$src" >/dev/null 2>&1; then
  echo "publish.sh: credential-like text in:" >&2
  grep -rIlE "$pattern" "$src" >&2
  exit 1
fi
remote_url=$(git remote get-url origin)
slug=$(sed -E 's#(git@github.com:|https://github.com/)##; s#\.git$##' <<<"$remote_url")
git fetch -q origin "+refs/heads/$branch:refs/remotes/origin/$branch" 2>/dev/null || true
parent=$(git rev-parse -q --verify "refs/remotes/origin/$branch^{commit}" || true)
index=$(mktemp)
trap 'rm -f "$index"' EXIT
export GIT_INDEX_FILE=$index
if [[ -n "$parent" ]]; then git read-tree "$parent"; else git read-tree --empty; fi
while IFS= read -r -d '' file; do
  rel=${file#"$src"/}
  blob=$(git hash-object -w -- "$file")
  git update-index --add --cacheinfo "100644,$blob,$dest/$rel"
done < <(find "$src" -type f ! -name '*.raw' -print0 | sort -z)
tree=$(git write-tree)
commit=$(git commit-tree "$tree" ${parent:+-p "$parent"} -m "evidence: $dest")
git push -q origin "$commit:refs/heads/$branch"
unset GIT_INDEX_FILE
while IFS= read -r -d '' file; do
  rel=${file#"$src"/}
  url="https://github.com/$slug/blob/$commit/$dest/$rel"
  case "$rel" in
    *.png|*.gif) printf '![%s](https://github.com/%s/raw/%s/%s/%s)\n' "$rel" "$slug" "$commit" "$dest" "$rel" ;;
    *) printf -- '- [%s](%s)\n' "$rel" "$url" ;;
  esac
done < <(find "$src" -type f ! -name '*.raw' -print0 | sort -z)
