#!/usr/bin/env bash
# The app's gate: static analysis, then the unit and widget tests. Run from
# anywhere; it works in apps/clax_app.
set -euo pipefail
cd "$(dirname "$0")/.."
flutter analyze --no-fatal-infos
flutter test --reporter=compact
