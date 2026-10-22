#!/bin/sh
# //design:icons_sync: the committed app icons were rendered from
# design/mark.json and design/tokens.json as they are.
exec bash "$TEST_SRCDIR/$TEST_WORKSPACE/scripts/build-app-icons.sh" --check
