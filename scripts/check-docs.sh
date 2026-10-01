#!/bin/bash
# Usage: scripts/check-docs.sh
#
# Catches three kinds of drift between docs/ and the code, and exits 1 on any:
#  1. A crate version in the "Runtime dependencies" table of
#     docs/operations/dependencies.md that does not match what Cargo.lock
#     resolves for the workspace. A documented `0.8` matches 0.8.x; a
#     40-hex git revision in the row must be the locked source revision.
#  2. A crates/wafer*/... path cited anywhere in docs/ that does not exist.
#     Historical records under docs/history/ are skipped.
#  3. A relative Markdown link, in any tracked .md file outside docs/history/,
#     whose target file does not exist. Anchors are not checked.
set -euo pipefail
cd "$(dirname "$0")/.."

failed=0
rows=0

# "name version source" for every direct dependency of a workspace crate.
locked=$(awk '
    /^\[\[package\]\]/ { indeps = 0; next }
    /^name = /     { name = $3; gsub(/"/, "", name); pkgs[++n] = name }
    /^version = /  { v = $3; gsub(/"/, "", v); ver[n] = v; all[name] = all[name] " " v }
    /^source = /   { s = $3; gsub(/"/, "", s); src[name " " ver[n]] = s; islocal[n] = 0; next }
    /^dependencies = \[/ { indeps = 1; next }
    indeps && /^\]/ { indeps = 0; next }
    indeps { d = $0; gsub(/[",]/, "", d); split(d, f, " "); deps[n] = deps[n] "|" f[1] " " f[2] }
    END {
        for (i = 1; i <= n; i++) {
            if (i in islocal) continue
            m = split(deps[i], list, "|")
            for (j = 2; j <= m; j++) {
                split(list[j], f, " "); dn = f[1]; dv = f[2]
                if (dv == "") { dv = all[dn]; sub(/^ /, "", dv) }
                print dn, dv, src[dn " " dv]
            }
        }
    }' Cargo.lock | sort -u)

while IFS= read -r row; do
    [[ $row =~ ^\|\ \`([A-Za-z0-9_-]+)\`\ \|\ \`([0-9][^\`]*)\` ]] || continue
    crate=${BASH_REMATCH[1]}
    rows=$((rows + 1))
    want=${BASH_REMATCH[2]}
    have=$(awk -v c="$crate" '$1 == c { print $2 }' <<<"$locked")
    if [[ -z $have ]]; then
        echo "dependencies.md: $crate is not a direct workspace dependency in Cargo.lock"
        failed=1
        continue
    fi
    ok=0
    for v in $have; do
        [[ $v == "$want" || $v == "$want".* ]] && ok=1
    done
    if ((!ok)); then
        echo "dependencies.md: $crate documented as $want, Cargo.lock has $(echo $have)"
        failed=1
    fi
    for rev in $(grep -oE '\b[0-9a-f]{40}\b' <<<"$row" || true); do
        if ! awk -v c="$crate" '$1 == c { print $3 }' <<<"$locked" | grep -q "$rev"; then
            echo "dependencies.md: $crate revision $rev is not the one in Cargo.lock"
            failed=1
        fi
    done
done < <(sed -n '/^## Runtime dependencies/,/^## /p' docs/operations/dependencies.md)
if ((rows == 0)); then
    echo "dependencies.md: no rows found under \"## Runtime dependencies\""
    failed=1
fi

while IFS=: read -r file line path; do
    if [[ ! -e $path ]]; then
        echo "$file:$line: $path does not exist"
        failed=1
    fi
done < <(grep -rnoE 'crates/wafer[A-Za-z0-9_-]*(/[A-Za-z0-9_.-]*[A-Za-z0-9_])*' docs \
    --exclude-dir=history |
    sort -t: -k1,1 -k2,2n | uniq)

while IFS= read -r file; do
    while IFS=: read -r line link; do
        target=${link#](}
        target=${target%)}
        target=${target%%#*}
        [[ $target =~ ^(https?|mailto): ]] && continue
        if [[ ! -e $(dirname "$file")/$target ]]; then
            echo "$file:$line: link target $target does not exist"
            failed=1
        fi
    done < <(grep -noE '\]\([^)#[:space:]][^)[:space:]]*\)' "$file" || true)
done < <(git ls-files '*.md' ':!docs/history/')

if ((failed)); then
    exit 1
fi
echo "docs: dependency versions, crates/ paths and relative links match the tree"
