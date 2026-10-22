#!/bin/sh
# //design:tokens_sync: the generated token files are in step with
# design/tokens.json, and its contrast pairs and app-mode colours hold.
exec python3 "$TEST_SRCDIR/$TEST_WORKSPACE/scripts/sync-tokens.py" --check
