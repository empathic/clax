#!/usr/bin/env bash
# Checks scripts/check-test-timing.sh: a foreground sleep with a delay (a
# number, a variable, infinity) after any command start, and Python's
# time.sleep, are findings; an exec'd or backgrounded sleep, a word list
# that holds "sleep", a comment, and wait_for's poll are not; a comparison
# that judges a clock reading (Python's, Node's or the shell's, directly or
# through a name assigned one or a value computed from one, in a heredoc or
# a `-c` script) is a finding, and a reading printed, a name reassigned, a
# poll bounded by a count and a redirection are not; an exemption with its
# reason (on the line or alone before it) passes, one without a reason or
# with nothing to exempt is a finding; report mode exits 0 with its
# findings printed, CLAX_TIMING=deny exits 1; with no file it scans every
# script an sh_test in scripts/BUILD.bazel runs, the smoke and other test
# scripts, and not the perf benchmarks, product scripts or itself.
# shellcheck disable=SC2016 # the cases are script text, never expanded here
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
expect() { # expect flagged|pass NAME CONTENT
    printf '%s\n' "$3" > "$d/$2"
    if CLAX_TIMING=deny scripts/check-test-timing.sh "$d/$2" > "$d/out" 2>&1; then got=pass; else got=flagged; fi
    if [ "$got" = "$1" ]; then echo "PASS: $2 $1"; else echo "FAIL: $2 expected $1, got $got"; cat "$d/out"; fails=1; fi
}
expect flagged sleep.sh 'sleep 1'
expect flagged sleep-fraction.sh '    sleep 0.3'
expect flagged sleep-unit.sh 'sleep 2s'
expect flagged sleep-var.sh 'sleep "$DELAY"'
expect flagged sleep-brace.sh 'sleep ${DELAY}'
expect flagged sleep-infinity.sh 'sleep infinity'
expect flagged after-and.sh 'start_it && sleep 1 && check_it'
expect flagged after-semi.sh 'start_it; sleep 1'
expect flagged in-loop.sh 'while ! ready; do sleep 0.1; done'
expect flagged then.sh 'if x; then sleep 1; fi'
expect flagged subshell.sh '( sleep 1; echo done ) &'
expect flagged python.sh 'python3 -c "import time; time.sleep(0.5)"'
expect flagged python-heredoc.sh '    time.sleep(0.2)'
expect flagged asyncio.py 'await asyncio.sleep(1)'
expect flagged no-reason.sh 'sleep 1  # timing:'
expect flagged stale-same.sh 'echo hi  # timing: nothing to exempt'
expect flagged stale-above.sh '# timing: nothing below to exempt
echo hi'
expect flagged py-budget.sh 't0 = time.monotonic()
run()
lag = time.monotonic() - t0
if not 0 <= lag <= 1.0:
    fail(lag)'
expect flagged py-key.sh 'sent["t"] = time.monotonic()
post()
returned = time.monotonic()
if returned - sent["t"] > 1.0: fail()'
expect flagged py-deadline.sh 'deadline = time.time() + 120
while time.time() < deadline and not done():
    poll()'
expect flagged py-assert.sh 'start = time.perf_counter()
run()
assert time.perf_counter() - start < 2'
expect flagged py-continued.sh 'took = time.time() - t0
ok = (code == 0
      and took < 3)'
expect flagged node.sh "const t0 = Date.now();
if (Date.now() - t0 > 5000) fail('slow');"
expect flagged shell-date.sh 'end=$(( $(date +%s) + 30 ))
until ready; do
    [ "$(date +%s)" -lt "$end" ] || exit 1
done'
expect flagged shell-local.sh 'local name="$1" end=$(( $(date +%s) + secs ))
[ "$now" -ge "$end" ] && return 1'
expect flagged shell-seconds.sh 'deadline=$((SECONDS + 30))
[ "$SECONDS" -lt "$deadline" ] || exit 1'
expect flagged shell-took.sh 'started=$(date +%s)
run_it
took=$(( $(date +%s) - started ))
[ "$took" -lt 10 ] || fail'
expect flagged shell-arith.sh 'start=$EPOCHSECONDS
(( EPOCHSECONDS - start < 3 )) || fail'
expect flagged py-c.sh "T=\"\$(python3 -c 'import time; print(time.time())')\"
LAG=\"\$(python3 -c 'import sys; print(float(sys.argv[2]) - float(sys.argv[1]))' \"\$T0\" \"\$T\")\"
python3 -c 'import sys; sys.exit(0 if float(sys.argv[1]) <= 3.0 else 1)' \"\$LAG\""
expect flagged clock-no-reason.sh 'while time.time() < deadline:  # timing:'
expect pass exec.sh 'exec sleep 2147483647'
expect pass exec-a.sh '(exec -a "clax serve --foreground" sleep "$FOREVER") &'
expect pass background.sh 'sleep 300 &'
expect pass background-var.sh 'sleep "$FOREVER" &'
expect pass printf.sh "printf '#!/bin/sh\\nexec sleep 2147483647\\n' | fake_exe x"
expect pass words.sh 'for tool in bash env sh sed sleep sort ls; do :; done'
expect pass comment.sh '# no sleep 1 here'
expect pass wait-for.sh 'wait_for "the daemon" gone "$pid"'
expect pass measured.sh 't0 = time.monotonic()
run()
print(f"took {time.monotonic() - t0:.1f} s")'
expect pass measured-shell.sh 'T_DIALOG="$(python3 -c "import time; print(time.time())")"
LAG="$(python3 -c "import sys; print(float(sys.argv[2]) - float(sys.argv[1]))" "$T_TOOL" "$T_DIALOG")"
pass "the dialog ${LAG} s after the tool call"'
expect pass reassigned.sh 't = time.time()
print(t)
t = 3
if t > 2: go()'
expect pass utime.sh "age() { python3 -c 'import os, sys, time; t = time.time() + int(sys.argv[2]); os.utime(sys.argv[1], (t, t))' \"\$1\" \"\$2\"; }"
expect pass redirect.sh 't=$(date +%s)
echo "$t" > "$d/t"
echo "$t" >&2
cat < "$d/t" 2>/dev/null'
expect pass poll-count.sh 'for _ in range(600):
    if ready(): break
    # timing: the poll of a condition, bounded by the poll count
    time.sleep(0.2)'
expect pass counted.sh 'for _ in $(seq 1 50); do kill -0 "$pid" || break; done
[ "$tries" -lt 50 ]'
expect pass clock-marked.sh 'while time.time() < deadline:  # timing: the hang guard'"'"'s own deadline
    poll()'
expect pass marked.sh 'sleep 0.05  # timing: the poll of a condition with a hang guard'
expect pass marked-above.sh '# timing: the poll of a condition with a hang guard
sleep 0.05'

printf 'sleep 1\n' > "$d/report.sh"
out="$(env -u CLAX_TIMING scripts/check-test-timing.sh "$d/report.sh" 2>&1)" && rc=0 || rc=$?
case "$rc:$out" in
    0:*"$d/report.sh:1: sleep:"*) echo "PASS: report mode prints and passes" ;;
    *) echo "FAIL: report mode: exit $rc, output: $out"; fails=1 ;;
esac

# --sh-tests: the srcs of each sh_test call in a BUILD file, and no other
# rule's, nor a list comprehension's.
cat > "$d/BUILD.bazel" <<'EOF'
copy_to_bin(
    name = "product_bin",
    srcs = ["product.sh"],
)

sh_test(
    name = "a",
    srcs = ["test-a.sh"],
    data = [":all"],
)

sh_test(
    name = "gate",
    size = "small",
    srcs = ["check-b.sh"],
)

[sh_test(
    name = name,
    srcs = [script],
    tags = ["manual"],
) for name, script in [("perf", "perf-c.sh")]]
EOF
out="$(python3 scripts/check-test-timing.py --sh-tests "$d/BUILD.bazel" | tr '\n' ' ')"
if [ "$out" = "test-a.sh check-b.sh " ]; then echo "PASS: --sh-tests lists each sh_test's srcs"; else echo "FAIL: --sh-tests printed: $out"; fails=1; fi

# The files the repository's scan reads: every script an sh_test in
# scripts/BUILD.bazel runs but the check and this test, the smoke scripts, the
# fakes; not the perf benchmarks, nor a product script.
listed="$(scripts/check-test-timing.sh --list)"
missing=""
while IFS= read -r src; do
    case "$src" in check-test-timing.sh | test-check-test-timing.sh) continue ;; esac
    grep -qxF "scripts/$src" <<<"$listed" || missing="$missing $src"
done < <(python3 scripts/check-test-timing.py --sh-tests scripts/BUILD.bazel)
for want in smoke-comment-loop.sh smoke-capabilities.sh smoke-claude-ask.sh fleet-kind-smoke.sh check-no-unsafe.sh fake-anthropic.py test-dev.sh; do
    grep -qxF "scripts/$want" <<<"$listed" || missing="$missing $want"
done
extra=""
for unwanted in perf-daemon.sh perf-clients.sh ensure-clax.sh check-test-timing.sh test-check-test-timing.sh lib/wait.sh; do
    if grep -qxF "scripts/$unwanted" <<<"$listed"; then extra="$extra $unwanted"; fi
done
if [ -z "$missing$extra" ]; then
    echo "PASS: the scan reads every sh_test's script and the smoke scripts, not the perf benchmarks or product scripts"
else
    echo "FAIL: the scan misses:${missing:- nothing}; reads:${extra:- nothing more}"
    fails=1
fi

if out="$(env -u CLAX_TIMING scripts/check-test-timing.sh 2>&1)"; then
    case "$out" in
        *"timing finding"*) echo "PASS: the repository's scripts are scanned (findings: report mode)" ;;
        *) echo "PASS: the repository's scripts are scanned and clean" ;;
    esac
else
    echo "FAIL: scanning the repository's scripts failed: $out"
    fails=1
fi
exit "$fails"
