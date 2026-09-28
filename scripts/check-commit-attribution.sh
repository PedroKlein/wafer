#!/bin/bash
# Usage: check-commit-attribution.sh <base> <head>
#
# Lists every commit in <base>..<head> whose author or committer is an AI
# agent identity, or whose message carries AI attribution lines, and exits 1
# if there is any.
set -euo pipefail

base=$1
head=$2
identity='(^|<)[^<>]*@anthropic\.com>?$|^claude <'
message='^claude-session:|^co-authored-by:.*(anthropic|claude)|generated (with|by) \[?claude code|claude\.ai/code'

failed=0
for sha in $(git rev-list "$base..$head"); do
    reasons=()
    author=$(git log -1 --format='%an <%ae>' "$sha")
    committer=$(git log -1 --format='%cn <%ce>' "$sha")
    if grep -qiE "$identity" <<<"$author"; then
        reasons+=("author $author")
    fi
    if grep -qiE "$identity" <<<"$committer"; then
        reasons+=("committer $committer")
    fi
    while IFS= read -r line; do
        reasons+=("message line: $line")
    done < <(git log -1 --format='%B' "$sha" | grep -iE "$message" || true)

    if ((${#reasons[@]})); then
        failed=1
        git log -1 --format='%h %s' "$sha"
        printf '    %s\n' "${reasons[@]}"
    fi
done

if ((failed)); then
    echo
    echo "Rewrite these commits with your own identity and without AI attribution lines."
    exit 1
fi
echo "$(git rev-list --count "$base..$head") commits checked, no AI attribution found"
