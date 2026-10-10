#!/usr/bin/env bash
# Prints the environment a harness's shell tool gives a command, so the
# CLI's agent-channel detection (crates/clax-cli/src/channel.rs, spec §16)
# lists exactly the variables a harness is seen to set.
#
# Run it from inside a harness, through its shell tool (ask the agent to run
# `bash scripts/probe-harness-env.sh` and show the output verbatim). It
# prints, one per line and sorted:
#   - every variable's name whose name suggests a harness (CLAUDE*, CODEX*,
#     GROK*, PI_*, CLAX*, AI_AGENT), with its value only for the variables
#     the CLI may read (CLAUDECODE, CODEX_THREAD_ID, CODEX_SANDBOX,
#     CODEX_CI, GROK_SESSION_ID, PI_CODING_AGENT, CLAX_CHANNEL; IDs and
#     flags, never secrets), others as <set>;
#   - then `names:` and the names of every other variable set, values
#     never shown (tokens live in some of them).
# Record the result per harness in docs/contract.md ("Harness shell
# environments").
set -euo pipefail
shown="CLAUDECODE CODEX_THREAD_ID CODEX_SANDBOX CODEX_CI GROK_SESSION_ID PI_CODING_AGENT CLAX_CHANNEL"
harness=""
others=""
while IFS= read -r name; do
    case "$name" in
        CLAUDE*|CODEX*|GROK*|PI_*|CLAX*|AI_AGENT)
            value="<set>"
            for s in $shown; do
                if [ "$s" = "$name" ]; then value="${!name}"; fi
            done
            harness="$harness$name=$value"$'\n'
            ;;
        *) others="$others$name"$'\n' ;;
    esac
done < <(compgen -e | LC_ALL=C sort)
printf 'harness:\n%s' "$harness"
printf 'names:\n%s' "$others"
