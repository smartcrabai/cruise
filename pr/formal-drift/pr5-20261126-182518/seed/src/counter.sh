#!/bin/sh
# Increment the counter file, refusing to pass the limit.
n=$(cat count 2>/dev/null || echo 0)
[ "$n" -lt 3 ] || { echo "limit reached" >&2; exit 1; }
echo $((n + 1)) > count
