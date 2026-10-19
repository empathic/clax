#!/usr/bin/env bash
# licence-check.sh: fails unless apps/clax_app/LICENSE, the licence the app
# shows on its licences page, ends with the text of LICENSES/GPL-2.0-only.txt
# under the app's copyright header.
set -euo pipefail
if [ -n "${TEST_SRCDIR:-}" ]; then
    root="$TEST_SRCDIR/$TEST_WORKSPACE"
else
    root="$(cd "$(dirname "$0")/../../.." && pwd)"
fi
app="$root/apps/clax_app/LICENSE"
gpl="$root/LICENSES/GPL-2.0-only.txt"
if [ "$(sed -n 2p "$app")" != 'Copyright (C) 2026 Empathic, Inc.' ]; then
    echo "apps/clax_app/LICENSE lacks 'Copyright (C) 2026 Empathic, Inc.' as its second line" >&2
    exit 1
fi
n="$(wc -c < "$gpl")"
if ! tail -c "$n" "$app" | cmp -s - "$gpl"; then
    echo "apps/clax_app/LICENSE does not end with LICENSES/GPL-2.0-only.txt: copy it again under the header" >&2
    exit 1
fi
