#!/usr/bin/env bash
# Tests scripts/find-candidate.sh in a scratch repository, with a fake gh on
# PATH; the real gh is never run. The fake answers `gh run list` with
# $FAKE_STATE/run-<commit> (the ID of the successful staging.yml run on a
# push of that commit; none when the file is missing) and the merge
# commit's pull requests with $FAKE_STATE/pr, and logs every call to
# $FAKE_STATE/calls.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
# shellcheck source=scripts/fake-exe.sh
. "$HERE/scripts/fake-exe.sh"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
SCRIPT="$HERE/scripts/find-candidate.sh"

mkdir -p "$T/bin" "$T/state"
fake_exe "$T/bin/gh" <<'PY'
#!/usr/bin/env python3
import os, sys
state = os.environ["FAKE_STATE"]
args = sys.argv[1:]
open(os.path.join(state, "calls"), "a").write(" ".join(args) + "\n")
def read(name):
    p = os.path.join(state, name)
    return open(p).read().strip() if os.path.exists(p) else ""
if args[:2] == ["run", "list"]:
    want = {"--workflow": "staging.yml", "--event": "push", "--status": "success"}
    for k, v in want.items():
        if args[args.index(k) + 1] != v:
            sys.exit(f"fake gh: run list without {k} {v}")
    print(read("run-" + args[args.index("--commit") + 1]))
elif args[0] == "api" and args[1].endswith("/pulls"):
    print(read("pr"))
else:
    sys.exit("fake gh: unexpected " + " ".join(args))
PY
export PATH="$T/bin:$PATH" FAKE_STATE="$T/state" GH_REPO=empathic/clax

# prod with one release merged: prod's tip P, the release branch's head H
# (P, then a merge of main, then the release commit), merged into prod as M.
R="$T/repo"
git init -q -b prod "$R"
g() { git -C "$R" -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false "$@"; }
echo 0.6.0 > "$R/VERSION"; g add VERSION; g commit -q -m "Release 0.6.0"
g switch -q -c main
echo feature > "$R/feature"; g add feature; g commit -q -m "feat: a feature"
g switch -q -c release/0.7.0 prod
g merge -q --no-ff main -m "Merge origin/main into release/0.7.0"
echo 0.7.0 > "$R/VERSION"; g commit -q -am "Release 0.7.0"
H="$(g rev-parse HEAD)"
g switch -q prod
g merge -q --no-ff release/0.7.0 -m "Merge pull request #70 from empathic/release/0.7.0"
M="$(g rev-parse HEAD)"
echo 4242 > "$T/state/run-$H"
echo 70 > "$T/state/pr"

# `expect <case> <want exit> <output pattern> <commit>`
expect() {
    local out rc
    out="$(cd "$R" && "$SCRIPT" "$4" 2>&1)"; rc=$?
    if [ "$rc" = "$2" ] && grep -qE -- "$3" <<< "$out"; then pass "$1"
    else fail "$1 (rc=$rc: $out)"; fi
}

expect "a release merge names its head, staging's run and the pull request" 0 \
    "^head=$H\$" "$M"
out="$(cd "$R" && "$SCRIPT" "$M" 2>&1)"
[ "$out" = "$(printf 'head=%s\nrun=4242\npr=70' "$H")" ] && pass "it prints only the three outputs" \
    || fail "it prints only the three outputs ($out)"

expect "a commit that is no merge is refused" 1 "is not a merge commit" "$H"

# prod moved while the pull request was open: the merge brings that
# commit in too, so its tree is not the tested head's.
g switch -q -c moved "$M~1"
echo hotfix > "$R/hotfix"; g add hotfix; g commit -q -m "fix: straight onto prod"
g merge -q --no-ff "$H" -m "Merge pull request #70 from empathic/release/0.7.0"
expect "a merge whose tree is not the head's is refused" 1 \
    "tree .* is not the tested head $H's .*prod moved while the release pull request was open" "$(g rev-parse HEAD)"

rm "$T/state/run-$H"
expect "a head with no successful staging run is refused" 1 \
    "no staging.yml run on a push of $H succeeded" "$M"
echo 4242 > "$T/state/run-$H"

: > "$T/state/pr"
expect "a merge GitHub names no pull request for is refused" 1 \
    "GitHub names no pull request into prod for $M" "$M"

if (cd "$R" && "$SCRIPT") >/dev/null 2>&1; then fail "no argument is a usage error"
else [ "$?" = 2 ] && pass "no argument is a usage error" || fail "no argument exits 2"; fi

exit "$FAILED"
