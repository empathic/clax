#!/usr/bin/env bash
# Runs the live build eval on macOS: each prompt of tool/eval/prompts.json
# (or --prompts a,b) built by the app's own agent on the Messages API,
# on the embedded core with a scratch home, with results in
# target/flutter-app/eval/<run>/.
#
#   ANTHROPIC_API_KEY=... apps/clax_app/tool/eval/run.sh [--model ID] [--prompts a,b] [--max-spend USD] [--run NAME]
#
# It spends real money (up to $6 a build; refused when that times the
# prompts is over --max-spend, $30 by default). It builds the macOS debug
# app from tool/eval/live_build.dart into apps/clax_app/build/macos, so do
# not run it while another macOS build or integration test of the app runs.
set -euo pipefail
app="$(cd "$(dirname "$0")/../.." && pwd)"
repo="$(cd "$app/../.." && pwd)"
flutter="${FLUTTER:-flutter}"
dart="$(dirname "$(command -v "$flutter")")/dart"

# One run name for the preflight and the app.
case " $* " in
    *" --run "*) ;;
    *) set -- "$@" --run "$(date +%Y%m%d-%H%M%S)" ;;
esac

# The arguments and the spend guard, before anything is built.
(cd "$app" && "$dart" run tool/eval/preflight.dart "$@")
if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo "eval: ANTHROPIC_API_KEY is not set" >&2
    exit 2
fi

# The core the app bundles (the Runner's "Embed core" phase copies it).
"$repo/scripts/build-app-clax.sh" macos --debug
args_json="$(python3 -c 'import json, sys; print(json.dumps(sys.argv[1:]))' "$@")"
mkdir -p "$repo/target/flutter-app/eval"
cd "$app"
CLAX_EVAL_ARGS="$args_json" \
CLAX_EVAL_APP_DIR="$app" \
CLAX_EVAL_OUT_ROOT="$repo/target/flutter-app/eval" \
    "$flutter" run -d macos --debug -t tool/eval/live_build.dart
