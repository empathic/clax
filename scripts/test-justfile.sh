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
recipes="$(just --summary)"
for r in dev watch install uninstall serve stop doctor wrapper-test install-test demo-room-sample; do
    case " $recipes " in *" $r "*) ;; *) echo "missing recipe: $r" >&2; missing=1 ;; esac
done
for r in dev-install dev-uninstall installer-test; do
    case " $recipes " in *" $r "*) echo "recipe $r should not exist" >&2; missing=1 ;; esac
done
exit "$missing"
