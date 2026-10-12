#!/bin/sh
# Keeps the app's prompt assets in step with the contract they copy.
#
# Writes into apps/clax_app/assets/prompt:
#   page-contract.md          docs/contract.md "## Page contract" up to
#                             "## Runtime capabilities"
#   runtime-capabilities.md   docs/contract.md "## Runtime capabilities" up to
#                             "## Runtime capabilities in detail"
#   db.d.ts                   web/contract/0.2.61/db.d.ts
#   clax-extensions.d.ts      web/contract/clax-extensions.d.ts
#
# Usage: scripts/sync-app-prompt.sh [--check]
#   --check  writes nothing; exits 1 with a diff when a copy differs.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/apps/clax_app/assets/prompt"
contract="$root/docs/contract.md"

check=0
case "${1:-}" in
    --check) check=1 ;;
    "") ;;
    *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

tmp="$(mktemp -d "${TMPDIR:-/tmp}/sync-app-prompt.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

# section START END: the lines of docs/contract.md from the heading line
# equal to START up to, not including, the heading line equal to END.
# Fails when either heading is missing.
section() {
    awk -v start="$1" -v end="$2" '
        $0 == start { on = 1; seen_start = 1 }
        $0 == end && on { on = 0; seen_end = 1; exit }
        on { print }
        END { if (!seen_start || !seen_end) exit 1 }
    ' "$contract" || {
        echo "sync-app-prompt: \"$1\" .. \"$2\" not found in docs/contract.md" >&2
        exit 1
    }
}

section "## Page contract" "## Runtime capabilities" > "$tmp/page-contract.md"
section "## Runtime capabilities" "## Runtime capabilities in detail" > "$tmp/runtime-capabilities.md"
cat "$root/web/contract/0.2.61/db.d.ts" > "$tmp/db.d.ts"
cat "$root/web/contract/clax-extensions.d.ts" > "$tmp/clax-extensions.d.ts"

files="page-contract.md runtime-capabilities.md db.d.ts clax-extensions.d.ts"

if [ "$check" -eq 1 ]; then
    status=0
    for f in $files; do
        if [ ! -f "$dest/$f" ]; then
            echo "sync-app-prompt: apps/clax_app/assets/prompt/$f is missing" >&2
            status=1
        elif ! diff -u "$dest/$f" "$tmp/$f"; then
            status=1
        fi
    done
    if [ "$status" -ne 0 ]; then
        echo "sync-app-prompt: the prompt copies are stale; run scripts/sync-app-prompt.sh" >&2
    fi
    exit "$status"
fi

mkdir -p "$dest"
for f in $files; do
    if ! cmp -s "$tmp/$f" "$dest/$f" 2>/dev/null; then
        mv -f "$tmp/$f" "$dest/$f"
        echo "sync-app-prompt: wrote apps/clax_app/assets/prompt/$f"
    fi
done
