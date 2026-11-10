#!/usr/bin/env bash
# Tests scripts/carry-release.sh in a scratch repository with a fake gh on
# PATH; the real gh is never run. The scratch repository has prod with a
# release merged and main moved on since the cut; its scripts/check-version.sh
# and scripts/pin-release.sh are stand-ins (the version is VERSION; the pin
# writes PINNED into the executable scripts/ensure-clax.sh), and
# carry-release.sh and wait-ci.sh are links to the real ones.
#
# The fake gh plays GitHub against the scratch repository's own objects: a
# blob, tree or commit made through `gh api repos/R/git/...` is made with
# git, so the tree the carry builds on GitHub is checked against git's
# (paths, contents and modes). Pull requests live in $FAKE_STATE/prs.json;
# `pr merge` squashes onto main, as GitHub would. A commit is "verified"
# unless $FAKE_STATE/unsigned exists. CI's bazel job on every commit has the
# conclusion in $FAKE_STATE/ci (success by default). Every call is logged to
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

mkdir -p "$T/bin" "$T/state"
fake_exe "$T/bin/gh" <<'PY'
#!/usr/bin/env python3
import base64, json, os, subprocess, sys, tempfile
state = os.environ["FAKE_STATE"]
repo = os.environ["FAKE_REPO"]
args = sys.argv[1:]
open(os.path.join(state, "calls"), "a").write(" ".join(args) + "\n")
def git(*a, inp=None, env=None):
    return subprocess.run(["git", "-C", repo, *a], input=inp, capture_output=True, check=True,
                          env={**os.environ, **(env or {})}).stdout.decode().strip()
def flag(k, default=None):
    return args[args.index(k) + 1] if k in args else default
prs_path = os.path.join(state, "prs.json")
prs = json.load(open(prs_path)) if os.path.exists(prs_path) else []
def save():
    json.dump(prs, open(prs_path, "w"))

def jq(value):
    q = flag("--jq")
    if q is None:
        print(json.dumps(value)); return
    out = subprocess.run(["jq", "-r", q], input=json.dumps(value).encode(), capture_output=True, check=True)
    sys.stdout.write(out.stdout.decode())

if args[0] == "api":
    path = args[1]
    method = flag("--method", "GET")
    body = json.load(sys.stdin) if flag("--input") == "-" else None
    p = path.split("/")[3:]  # after repos/<owner>/<name>
    if p[:2] == ["git", "blobs"]:
        data = base64.b64decode(body["content"])
        jq({"sha": git("hash-object", "-w", "--stdin", inp=data)})
    elif p[:2] == ["git", "trees"]:
        with tempfile.NamedTemporaryFile() as idx:
            env = {"GIT_INDEX_FILE": idx.name}
            git("read-tree", body["base_tree"], env=env)
            for e in body["tree"]:
                if e["sha"] is None:
                    git("update-index", "--force-remove", e["path"], env=env)
                else:
                    git("update-index", "--add", "--cacheinfo", f'{e["mode"]},{e["sha"]},{e["path"]}', env=env)
            jq({"sha": git("write-tree", env=env)})
    elif p[:2] == ["git", "commits"]:
        sha = git("-c", "user.name=app", "-c", "user.email=app@example.com", "commit-tree", body["tree"],
                  *sum((["-p", x] for x in body["parents"]), []), "-m", body["message"])
        jq({"sha": sha, "verification": {"verified": not os.path.exists(os.path.join(state, "unsigned"))}})
    elif p[:2] == ["git", "ref"]:
        ref = "/".join(p[2:])
        r = subprocess.run(["git", "-C", repo, "rev-parse", "--verify", "--quiet", "refs/" + ref], capture_output=True)
        if r.returncode:
            print("gh: Not Found (HTTP 404)", file=sys.stderr); sys.exit(1)
        jq({"object": {"sha": r.stdout.decode().strip()}})
    elif p[:2] == ["git", "refs"] and method == "POST":
        git("update-ref", body["ref"], body["sha"]); jq({})
    elif p[:2] == ["git", "refs"] and method == "PATCH":
        git("update-ref", "refs/" + "/".join(p[2:]), body["sha"]); jq({})
    else:
        sys.exit("fake gh: unexpected api " + " ".join(args))
elif args[:2] == ["pr", "list"]:
    want = flag("--state")
    jq([{"number": x["number"], "headRefOid": x["head"], "mergeCommit": {"oid": x.get("merged")}}
        for x in prs if x["branch"] == flag("--head") and x["state"] == want])
elif args[:2] == ["pr", "create"]:
    head = git("rev-parse", "refs/heads/" + flag("--head"))
    prs.append({"number": 80 + len(prs), "branch": flag("--head"), "head": head, "state": "open", "title": flag("--title")})
    save(); print(f"https://github.com/empathic/clax/pull/{prs[-1]['number']}")
elif args[:2] == ["pr", "merge"]:
    pr = next(x for x in prs if x["number"] == int(args[2]))
    assert "--squash" in args and flag("--match-head-commit") == pr["head"], args
    tree = git("rev-parse", pr["head"] + "^{tree}")
    main = git("rev-parse", "refs/heads/main")
    sha = git("-c", "user.name=gh", "-c", "user.email=gh@example.com", "commit-tree", tree, "-p", main, "-m", flag("--subject"))
    git("update-ref", "refs/heads/main", sha)
    pr.update(state="merged", merged=sha); save()
elif args[:2] == ["run", "list"]:
    jq([{"databaseId": 7000}])
elif args[:2] == ["run", "view"]:
    c = os.path.join(state, "ci")
    jq({"jobs": [{"name": "bazel", "status": "completed", "conclusion": open(c).read().strip() if os.path.exists(c) else "success"}]})
else:
    sys.exit("fake gh: unexpected " + " ".join(args))
PY

# The scratch repository, which is its own origin: origin/main is main.
R="$T/repo"
git init -q -b prod "$R"
g() { git -C "$R" -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false "$@"; }
mkdir -p "$R/scripts"
fake_exe "$R/scripts/check-version.sh" <<'SH'
#!/usr/bin/env bash
cd "$(dirname "$0")/.."
if [ "${1:-}" = --print ]; then cat VERSION; fi
SH
fake_exe "$R/scripts/pin-release.sh" <<'SH'
#!/usr/bin/env bash
cd "$(dirname "$0")/.."
sed -i.bak "s/^PINNED=.*/PINNED=$1/" scripts/ensure-clax.sh && rm scripts/ensure-clax.sh.bak
SH
ln -s "$HERE/scripts/carry-release.sh" "$R/scripts/carry-release.sh"
ln -s "$HERE/scripts/wait-ci.sh" "$R/scripts/wait-ci.sh"
printf '#!/bin/sh\nPINNED=v0.6.0\n' > "$R/scripts/ensure-clax.sh"
chmod 755 "$R/scripts/ensure-clax.sh"
echo 0.6.0 > "$R/VERSION"
g add -A; g commit -q -m "Release 0.6.0"
g switch -q -c main
echo feature > "$R/feature"; g add feature; g commit -q -m "feat: a feature"
g switch -q -c release/0.7.0 prod
g merge -q --no-ff main -m "Merge origin/main into release/0.7.0"
echo 0.7.0 > "$R/VERSION"; g commit -q -am "Release 0.7.0"
echo fixed > "$R/fix"; g add fix; g commit -q -m "fix: found on staging"
g switch -q prod
g merge -q --no-ff release/0.7.0 -m "Merge pull request #70 from empathic/release/0.7.0"
M="$(g rev-parse HEAD)"
# main moves on after the cut.
g switch -q main
echo later > "$R/later"; g add later; g commit -q -m "feat: after the cut"
g switch -q --detach prod
g remote add origin "$R"
g fetch -q origin
MAIN_BEFORE="$(g rev-parse main)"
export PATH="$T/bin:$PATH" FAKE_STATE="$T/state" FAKE_REPO="$R" GH_REPO=empathic/clax CLAX_WAIT_POLL_SECONDS=0

out="$("$R/scripts/carry-release.sh" 0.7.0 "$M" 2>&1)"; rc=$?
new_main="$(g rev-parse main)"
if [ "$rc" = 0 ] && grep -qx "commit=$new_main" <<< "$out"; then pass "the carry merges into main and prints main's new commit"
else fail "the carry merges into main and prints main's new commit (rc=$rc: $out)"; fi
[ "$(g rev-parse "$new_main^")" = "$MAIN_BEFORE" ] && pass "main gains one commit, on its tip" \
    || fail "main gains one commit, on its tip"
[ "$(g show "$new_main:VERSION")" = 0.7.0 ] && [ "$(g show "$new_main:fix")" = fixed ] \
    && [ "$(g show "$new_main:later")" = later ] \
    && pass "it carries the release commit and the branch's fix, and keeps main's later work" \
    || fail "it carries the release commit and the branch's fix, and keeps main's later work"
body="$(g log -1 --format=%B "$(g rev-parse refs/heads/carry/0.7.0)")"
if grep -q "Release 0.7.0" <<< "$body" && grep -q "fix: found on staging" <<< "$body" && ! grep -q "Release 0.6.0\|feat: a feature\|after the cut" <<< "$body"; then
    pass "the carry's commit lists the release branch's commits alone"
else fail "the carry's commit lists the release branch's commits alone ($body)"; fi
grep -qx "PINNED=v0.7.0" <(g show "$new_main:scripts/ensure-clax.sh") && pass "the plugins are pinned in the same commit" \
    || fail "the plugins are pinned in the same commit"
[ "$(g ls-tree "$new_main" scripts/ensure-clax.sh | cut -d' ' -f1)" = 100755 ] && pass "an executable file stays executable" \
    || fail "an executable file stays executable ($(g ls-tree "$new_main" scripts/ensure-clax.sh))"
g log -1 --format=%s "$new_main" | grep -x 'chore(release): 0.7.0, with the plugins pinned to it (#80)' >/dev/null \
    && pass "the squash is titled as a conventional commit with the pull request" \
    || fail "the squash is titled ($(g log -1 --format=%s "$new_main"))"
grep -q "^pr create --repo empathic/clax --base main --head carry/0.7.0" "$T/state/calls" \
    && pass "the pull request is from carry/0.7.0 into main" || fail "the pull request is from carry/0.7.0 into main"
[ "$(git -C "$R" worktree list | wc -l | tr -d ' ')" = 1 ] \
    && pass "the temporary worktree is removed" || fail "the temporary worktree is removed"

: > "$T/state/calls"
out="$("$R/scripts/carry-release.sh" 0.7.0 "$M" 2>&1)"; rc=$?
if [ "$rc" = 0 ] && grep -qx "commit=$new_main" <<< "$out" && ! grep -q "git/commits\|pr create\|pr merge" "$T/state/calls"; then
    pass "run again, it finds the carry merged and makes nothing"
else fail "run again, it finds the carry merged and makes nothing (rc=$rc: $out)"; fi

# A fresh carry, each time from main before the first.
reset() { g update-ref refs/heads/main "$MAIN_BEFORE"; g fetch -q origin; rm -f "$T/state/prs.json" "$T/state/unsigned" "$T/state/ci"; : > "$T/state/calls"; g update-ref -d refs/heads/carry/0.7.0; }

reset
touch "$T/state/unsigned"
out="$("$R/scripts/carry-release.sh" 0.7.0 "$M" 2>&1)"; rc=$?
if [ "$rc" = 1 ] && grep -q "GitHub did not sign" <<< "$out" && [ "$(g rev-parse main)" = "$MAIN_BEFORE" ]; then
    pass "a commit GitHub did not sign is refused before any pull request"
else fail "a commit GitHub did not sign is refused before any pull request (rc=$rc: $out)"; fi

reset
echo failure > "$T/state/ci"
out="$("$R/scripts/carry-release.sh" 0.7.0 "$M" 2>&1)"; rc=$?
if [ "$rc" = 1 ] && grep -q "bazel job on .* ended failure" <<< "$out" && [ "$(g rev-parse main)" = "$MAIN_BEFORE" ]; then
    pass "a carry CI fails on is not merged"
else fail "a carry CI fails on is not merged (rc=$rc: $out)"; fi
rm "$T/state/ci"
out="$("$R/scripts/carry-release.sh" 0.7.0 "$M" 2>&1)"; rc=$?
if [ "$rc" = 0 ] && [ "$(g rev-parse "$(g rev-parse main)^")" = "$MAIN_BEFORE" ] && [ "$(grep -c '^pr create' "$T/state/calls")" = 1 ]; then
    pass "run again once CI passes, it merges the open pull request without opening another"
else fail "run again once CI passes, it merges the open pull request (rc=$rc: $out)"; fi

# main changed the version file after the cut: the release conflicts.
reset
g switch -q main; echo 0.6.1 > "$R/VERSION"; g commit -q -am "chore: a version on main"; g switch -q --detach prod
g fetch -q origin
out="$("$R/scripts/carry-release.sh" 0.7.0 "$M" 2>&1)"; rc=$?
if [ "$rc" = 1 ] && grep -q "does not merge into main's tip .*: VERSION conflicts" <<< "$out" && ! grep -q "git/commits" "$T/state/calls"; then
    pass "a release that conflicts with main stops the carry, naming the file"
else fail "a release that conflicts with main stops the carry, naming the file (rc=$rc: $out)"; fi
[ "$(git -C "$R" worktree list | wc -l | tr -d ' ')" = 1 ] \
    && pass "a carry that stops removes its worktree" || fail "a carry that stops removes its worktree"

# 0.7.0 was merged into prod and never carried (its Deploy prod run failed
# before the carry); 0.7.1 is cut from that prod, with main merged in. Its
# carry brings main both releases: all that prod has and main lacks.
g update-ref refs/heads/main "$MAIN_BEFORE"
rm -f "$T/state/prs.json"; : > "$T/state/calls"; g update-ref -d refs/heads/carry/0.7.0
g switch -q -c release/0.7.1 prod
g merge -q --no-ff main -m "Merge origin/main into release/0.7.1"
echo 0.7.1 > "$R/VERSION"; g commit -q -am "Release 0.7.1"
g switch -q prod
g merge -q --no-ff release/0.7.1 -m "Merge pull request #90 from empathic/release/0.7.1"
M2="$(g rev-parse HEAD)"
g switch -q --detach prod
g fetch -q origin
out="$("$R/scripts/carry-release.sh" 0.7.1 "$M2" 2>&1)"; rc=$?
new_main="$(g rev-parse main)"
if [ "$rc" = 0 ] && grep -qx "commit=$new_main" <<< "$out" && [ "$(g rev-parse "$new_main^")" = "$MAIN_BEFORE" ] \
    && [ "$(g show "$new_main:VERSION")" = 0.7.1 ] && [ "$(g show "$new_main:fix")" = fixed ] \
    && [ "$(g show "$new_main:later")" = later ] && grep -qx "PINNED=v0.7.1" <(g show "$new_main:scripts/ensure-clax.sh"); then
    pass "a release after one never carried carries both: the earlier branch's fix, the version and the pin"
else fail "a release after one never carried carries both (rc=$rc: $out)"; fi
body="$(g log -1 --format=%B "$(g rev-parse refs/heads/carry/0.7.1)")"
if grep -q "Release 0.7.1" <<< "$body" && grep -q "fix: found on staging" <<< "$body" && grep -q "Release 0.7.0" <<< "$body" \
    && ! grep -q "Release 0.6.0\|feat: a feature" <<< "$body"; then
    pass "the carry's commit lists the commits of both releases, and none main has"
else fail "the carry's commit lists the commits of both releases, and none main has ($body)"; fi

if "$R/scripts/carry-release.sh" 0.7 "$M" >/dev/null 2>&1; then fail "a version that is not X.Y.Z is a usage error"
else [ "$?" = 2 ] && pass "a version that is not X.Y.Z is a usage error" || fail "a version that is not X.Y.Z exits 2"; fi

exit "$FAILED"
