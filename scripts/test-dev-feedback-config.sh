#!/usr/bin/env bash
# Checks scripts/lib/feedback-config.py, which scripts/dev-feedback.sh
# rewrites a home's `[feedback]` table with: the table is found with
# spaces and a trailing comment on its header, ends at the next real
# header (not at a bracketed line inside a multi-line string or array),
# and every other line is kept.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
fails=0
check() { # check NAME EXPECTED ACTUAL
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else printf 'FAIL: %s\nwant:\n%s\ngot:\n%s\n' "$1" "$2" "$3"; fails=1; fi
}
py() { python3 scripts/lib/feedback-config.py "$@"; }

# No file: the table alone.
py write "$d/none.toml" q1 http://localhost:7481
check "no file" '[feedback]
queue = "q1"
service_grant_file = "feedback-grant.json"
origin = "http://localhost:7481"' "$(cat "$d/none.toml")"
check "queue of a written table" q1 "$(py queue "$d/none.toml")"
check "queue of no file" "" "$(py queue "$d/missing.toml")"

# A header with spaces and a comment, a later table, and look-alike lines.
cat > "$d/c.toml" <<'EOF'
[serve]
port = 7481

  [ feedback ]   # set by dev-feedback
queue = "old"
service_grant_file = "x.json"

[bot]
note = """
[feedback]
queue = "in a string"
"""
models = [
  [1],
]
EOF
check "queue of a spaced header" old "$(py queue "$d/c.toml")"
py write "$d/c.toml" q2 http://localhost:7481
check "rewrite keeps the rest" '[serve]
port = 7481

[bot]
note = """
[feedback]
queue = "in a string"
"""
models = [
  [1],
]

[feedback]
queue = "q2"
service_grant_file = "feedback-grant.json"
origin = "http://localhost:7481"' "$(cat "$d/c.toml")"
check "queue after the rewrite" q2 "$(py queue "$d/c.toml")"

# Written again: the same file.
before="$(cat "$d/c.toml")"
py write "$d/c.toml" q2 http://localhost:7481
check "rewrite is stable" "$before" "$(cat "$d/c.toml")"

# The file's mode is kept.
chmod 600 "$d/c.toml"
py write "$d/c.toml" q3 http://localhost:7481
# GNU stat first: on Linux `stat -f` is the filesystem's status, and succeeds.
check "mode kept" 600 "$(stat -c %a "$d/c.toml" 2>/dev/null || stat -f %Lp "$d/c.toml")"

exit "$fails"
