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
for r in dev watch install uninstall serve stop doctor wrapper-test install-test demo-room-sample release; do
    case " $recipes " in *" $r "*) ;; *) echo "missing recipe: $r" >&2; missing=1 ;; esac
done
for r in dev-install dev-uninstall installer-test; do
    case " $recipes " in *" $r "*) echo "recipe $r should not exist" >&2; missing=1 ;; esac
done

# Recipes hand their arguments on word for word: one with a space, shell
# syntax or a `$(...)` reaches the script as given and is never run. The
# justfile runs in a scratch copy whose scripts are stubs printing their
# arguments one per line.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/scripts"
cp justfile "$tmp/"
for s in run.sh check-wasm.sh bazel-gc.sh release.sh cloudflare-setup.sh; do
    printf '#!/bin/sh\nfor a in "$@"; do printf "%%s\\n" "$a"; done\n' > "$tmp/scripts/$s"
    chmod +x "$tmp/scripts/$s"
done
args=(-E 'test(foo)' 'a b' '$(touch pwned)' ';' '*')
want="$(printf '%s\n' "${args[@]}")"
for r in build build-native web web-native web-test web-test-native web-e2e web-e2e-native \
    pi-test pi-test-native test test-native check check-native ci ci-native perf serve \
    check-wasm bazel-gc release clax-dev-deploy; do
    got="$(cd "$tmp" && just "$r" "${args[@]}" 2>&1 | tail -n "${#args[@]}")" || true
    if [ "$got" != "$want" ] || [ -e "$tmp/pwned" ]; then
        echo "just $r does not pass its arguments intact; got:" >&2
        printf '%s\n' "$got" | sed 's/^/    | /' >&2
        missing=1
    fi
done
# clax-dev-deploy deploys clax.dev, the production instance, and nothing
# else: it always passes --production (docs/prod.md, "Deploying from a
# laptop").
got="$(cd "$tmp" && just clax-dev-deploy --smoke 2>&1 | tail -n 4)" || true
if [ "$got" != "$(printf '%s\n' --legal bundled --production --smoke)" ]; then
    echo "just clax-dev-deploy does not pass --legal bundled --production; got:" >&2
    printf '%s\n' "$got" | sed 's/^/    | /' >&2
    missing=1
fi
exit "$missing"
