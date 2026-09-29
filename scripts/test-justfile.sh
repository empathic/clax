#!/usr/bin/env bash
# Fails if the justfile does not evaluate or any recipe lacks a description.
set -euo pipefail
cd "$(dirname "$0")/.."
just --evaluate >/dev/null
missing=0
while IFS= read -r line; do
    case "$line" in
        "Available recipes:"|"") continue ;;
    esac
    if [[ "$line" != *"#"* ]]; then
        echo "recipe without a description: ${line# }" >&2
        missing=1
    fi
done < <(just --list --unsorted)
exit "$missing"
