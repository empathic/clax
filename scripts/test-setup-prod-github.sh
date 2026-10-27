#!/usr/bin/env bash
# Tests scripts/setup-prod-github.sh against a fake gh and a fake curl on
# PATH; the real gh and curl are never run. The fake gh plays GitHub's REST
# API from $FAKE_STATE/github.json (refs, commits, what main holds,
# rulesets, environments with their branch policies and variables), applies
# every write to it, and logs each call ("METHOD path [body]") to
# $FAKE_STATE/calls. A GET of something that does not exist fails as gh
# does, with "HTTP 404"; $FAKE_STATE/fail-<path with / as _> makes a GET of
# that path fail with HTTP 403. The fake curl prints $FAKE_STATE/healthz.
# The fakes' helper functions run through the cases, out of shellcheck's sight.
# shellcheck disable=SC2329
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
# shellcheck source=scripts/fake-exe.sh
. "$HERE/scripts/fake-exe.sh"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
SCRIPT="$HERE/scripts/setup-prod-github.sh"
R=empathic/clax
FULL=0123456789abcdef0123456789abcdef01234567

mkdir -p "$T/bin"
fake_exe "$T/bin/gh" <<'PY'
#!/usr/bin/env python3
import json, os, re, sys
state = os.environ["FAKE_STATE"]
db_path = os.path.join(state, "github.json")
db = json.load(open(db_path))
args = sys.argv[1:]
method, body, path = "GET", None, None
if not args or args[0] != "api":
    open(os.path.join(state, "calls"), "a").write("UNEXPECTED gh " + " ".join(args) + "\n")
    sys.exit(3)
i = 1
while i < len(args):
    a = args[i]
    if a == "--method": method = args[i + 1]; i += 2
    elif a == "--input": body = json.load(open(args[i + 1])); i += 2
    else: path = a; i += 1
log = f"{method} {path}" + (" " + json.dumps(body, sort_keys=True, separators=(",", ":")) if body is not None else "")
open(os.path.join(state, "calls"), "a").write(log + "\n")

def out(v):
    print(json.dumps(v))
    sys.exit(0)
def missing():
    print("gh: Not Found (HTTP 404)", file=sys.stderr)
    sys.exit(1)
def save():
    json.dump(db, open(db_path, "w"))
    sys.exit(0)

if os.path.exists(os.path.join(state, "fail-" + path.replace("/", "_"))):
    print("gh: Resource not accessible by integration (HTTP 403)", file=sys.stderr)
    sys.exit(1)
p = path.split("?")[0]
m = re.fullmatch(r"repos/[^/]+/[^/]+/(.*)", p)
if not m: missing()
rest = m.group(1)
envs = db.setdefault("envs", {})
if method == "GET":
    if rest == "git/ref/heads/prod":
        if "prod" in db.get("refs", {}): out({"ref": "refs/heads/prod", "object": {"sha": db["refs"]["prod"]}})
        missing()
    if (m := re.fullmatch(r"commits/(\w+)", rest)):
        full = next((c for c in db.get("commits", []) if c.startswith(m.group(1))), None)
        out({"sha": full}) if full else missing()
    if (m := re.fullmatch(r"compare/main\.\.\.(\w+)", rest)):
        out({"status": "behind" if m.group(1) in db.get("on_main", []) else "diverged"})
    if rest == "rulesets":
        out([{"id": r["id"], "name": r["name"]} for r in db.get("rulesets", [])])
    if (m := re.fullmatch(r"environments/([^/]+)", rest)):
        e = envs.get(m.group(1))
        out({"name": m.group(1), "deployment_branch_policy": e.get("policy")}) if e else missing()
    if (m := re.fullmatch(r"environments/([^/]+)/deployment-branch-policies", rest)):
        e = envs.get(m.group(1)) or missing()
        out({"total_count": len(e["branches"]), "branch_policies": e["branches"]})
    if (m := re.fullmatch(r"environments/([^/]+)/variables/(\w+)", rest)):
        v = (envs.get(m.group(1)) or {}).get("vars", {}).get(m.group(2))
        out({"name": m.group(2), "value": v}) if v is not None else missing()
    missing()
if method == "POST" and rest == "git/refs":
    db.setdefault("refs", {})[body["ref"].removeprefix("refs/heads/")] = body["sha"]; save()
if method == "POST" and rest == "rulesets":
    body["id"] = 40 + len(db.setdefault("rulesets", [])); db["rulesets"].append(body); save()
if method == "PUT" and (m := re.fullmatch(r"rulesets/(\d+)", rest)):
    db["rulesets"] = [dict(body, id=r["id"]) if r["id"] == int(m.group(1)) else r for r in db["rulesets"]]; save()
if method == "PUT" and (m := re.fullmatch(r"environments/([^/]+)", rest)):
    e = envs.setdefault(m.group(1), {"branches": [], "vars": {}})
    e["policy"] = body["deployment_branch_policy"]; save()
if method == "POST" and (m := re.fullmatch(r"environments/([^/]+)/deployment-branch-policies", rest)):
    e = envs[m.group(1)]
    e["branches"].append(dict(body, id=100 + len(e["branches"]))); save()
if method == "DELETE" and (m := re.fullmatch(r"environments/([^/]+)/deployment-branch-policies/(\d+)", rest)):
    e = envs[m.group(1)]
    e["branches"] = [b for b in e["branches"] if b["id"] != int(m.group(2))]; save()
if method == "POST" and (m := re.fullmatch(r"environments/([^/]+)/variables", rest)):
    envs[m.group(1)].setdefault("vars", {})[body["name"]] = body["value"]; save()
if method == "PATCH" and (m := re.fullmatch(r"environments/([^/]+)/variables/(\w+)", rest)):
    envs[m.group(1)]["vars"][m.group(2)] = body["value"]; save()
print(f"fake gh: no such call: {log}", file=sys.stderr)
sys.exit(2)
PY
fake_exe "$T/bin/curl" <<'SH'
#!/bin/sh
echo "curl $*" >> "$FAKE_STATE/calls"
cat "$FAKE_STATE/healthz"
SH

# A case: a state directory with GitHub as it is before any setup (no prod,
# no ruleset, no production environment, npm deployable from main), and
# clax.dev reporting a short commit on main.
C=""
new_case() {
    C="$T/case-$1"
    mkdir -p "$C"
    cat > "$C/github.json" <<JSON
{"refs": {"main": "$FULL"}, "commits": ["$FULL", "fedcba9876543210fedcba9876543210fedcba98"],
 "on_main": ["$FULL"],
 "envs": {"npm": {"policy": {"protected_branches": false, "custom_branch_policies": true},
                  "branches": [{"id": 11, "name": "main", "type": "branch"}], "vars": {}}}}
JSON
    echo '{"version":"0.6.0","commit":"012345678","build":"b-1"}' > "$C/healthz"
    : > "$C/calls"
}
run() { (cd "$T" && FAKE_STATE="$C" PATH="$T/bin:$PATH" "$SCRIPT" "$@" > "$C/out" 2>&1); }
# q EXPR: the Python expression EXPR over the case's GitHub, bound to `g`.
q() { python3 -c 'import json, sys; g = json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$C/github.json" "$1"; }
writes() { grep -v '^GET \|^curl ' "$C/calls"; }

# Dry run: reads only, and every write printed.
new_case dry
cp "$C/github.json" "$C/before.json"
if run; then pass "the dry run succeeds"; else fail "the dry run succeeds ($(cat "$C/out"))"; fi
if [ -z "$(writes)" ] && cmp -s "$C/github.json" "$C/before.json"; then pass "the dry run makes no write"
else fail "the dry run makes no write ($(writes))"; fi
if grep -q "^curl .*https://clax.dev/healthz" "$C/calls" && grep -q "GET repos/$R/commits/012345678" "$C/calls" \
    && grep -q "GET repos/$R/compare/main...$FULL" "$C/calls"; then
    pass "the dry run reads clax.dev's commit and checks it is on main"
else fail "the dry run reads clax.dev's commit ($(cat "$C/calls"))"; fi
for want in \
    "would run: gh api --method POST repos/$R/git/refs --input - <<< '{\"ref\":\"refs/heads/prod\",\"sha\":\"$FULL\"}'" \
    "would run: gh api --method POST repos/$R/rulesets --input -" \
    "would run: gh api --method PUT repos/$R/environments/production --input -" \
    "would run: gh api --method POST repos/$R/environments/production/deployment-branch-policies --input - <<< '{\"name\":\"prod\",\"type\":\"branch\"}'" \
    "would run: gh api --method POST repos/$R/environments/production/variables --input - <<< '{\"name\":\"PUBLIC_HOST\",\"value\":\"clax.dev\"}'" \
    "would run: gh api --method POST repos/$R/environments/production/variables --input - <<< '{\"name\":\"CLOUDFLARE_ACCOUNT_ID\",\"value\":\"00000000000000000000000000000000\"}'" \
    "would run: gh api --method POST repos/$R/environments/npm/deployment-branch-policies --input - <<< '{\"name\":\"prod\",\"type\":\"branch\"}'" \
    "would run: gh api --method DELETE repos/$R/environments/npm/deployment-branch-policies/11" \
    "gh secret set CLOUDFLARE_API_TOKEN --env production --repo $R" \
    "gh secret set CLAX_SMOKE_TOKEN --env production --repo $R"; do
    if grep -qF -- "$want" "$C/out"; then pass "the dry run prints: ${want:0:90}"; else fail "the dry run prints: $want ($(cat "$C/out"))"; fi
done

# Acting: GitHub ends as set up.
new_case act
if run --i-know-what-i-am-doing; then pass "acting succeeds"; else fail "acting succeeds ($(cat "$C/out"))"; fi
if [ "$(q 'g["refs"].get("prod")')" = "$FULL" ]; then pass "prod starts at clax.dev's commit, in full"; else fail "prod starts at clax.dev's commit ($(q 'g["refs"]'))"; fi
rules="$(q 'sorted(r["type"] for r in g["rulesets"][0]["rules"])')"
if [ "$(q 'len(g["rulesets"])')" = 1 ] && [ "$(q 'g["rulesets"][0]["conditions"]["ref_name"]["include"]')" = "['refs/heads/prod']" ] \
    && [ "$rules" = "['deletion', 'non_fast_forward', 'pull_request', 'required_signatures', 'required_status_checks']" ] \
    && [ "$(q 'g["rulesets"][0]["enforcement"]')" = active ]; then
    pass "the ruleset on refs/heads/prod: pull request, checks, signatures, no deletion, no force push"
else fail "the ruleset on refs/heads/prod ($rules)"; fi
if [ "$(q '[c["context"] for r in g["rulesets"][0]["rules"] if r["type"] == "required_status_checks" for c in r["parameters"]["required_status_checks"]]')" = "['bazel', 'worker', 'version']" ] \
    && [ "$(q '[r["parameters"]["allowed_merge_methods"] for r in g["rulesets"][0]["rules"] if r["type"] == "pull_request"]')" = "[['merge']]" ] \
    && [ "$(q 'g["rulesets"][0]["bypass_actors"]')" = "[{'actor_id': 5, 'actor_type': 'RepositoryRole', 'bypass_mode': 'always'}]" ]; then
    pass "the ruleset requires bazel, worker and version, merges with a merge commit, and admins bypass it"
else fail "the ruleset's checks, merge method and bypass ($(q 'g["rulesets"][0]'))"; fi
if [ "$(q 'g["envs"]["production"]["policy"]["custom_branch_policies"]')" = True ] \
    && [ "$(q '[b["name"] for b in g["envs"]["production"]["branches"]]')" = "['prod']" ]; then
    pass "production deploys from prod only"; else fail "production deploys from prod only ($(q 'g["envs"]["production"]'))"; fi
if [ "$(q 'sorted(g["envs"]["production"]["vars"].items())')" = "[('CLOUDFLARE_ACCOUNT_ID', '00000000000000000000000000000000'), ('OAUTH_CLIENT_ID', 'Iv1.exampleclient'), ('OWNER_GITHUB', 'akesling'), ('PUBLIC_HOST', 'clax.dev'), ('SITE_DOMAIN', 'clax.run'), ('STORE', 'artifacts')]" ]; then
    pass "production's variables are clax.dev's"; else fail "production's variables ($(q 'g["envs"]["production"]["vars"]'))"; fi
if [ "$(q '[b["name"] for b in g["envs"]["npm"]["branches"]]')" = "['prod']" ]; then pass "npm deploys from prod in place of main"
else fail "npm deploys from prod ($(q 'g["envs"]["npm"]'))"; fi
if grep -q "UNEXPECTED\|secret" "$C/calls"; then fail "no gh call but gh api, and no secret set ($(cat "$C/calls"))"; else pass "no gh call but gh api, and no secret set"; fi

# Again: only the ruleset is written.
: > "$C/calls"
if run --i-know-what-i-am-doing && [ "$(writes)" = "$(writes | grep "^PUT repos/$R/rulesets/40 ")" ] && [ "$(writes | wc -l | tr -d ' ')" = 1 ] \
    && ! grep -q '^curl ' "$C/calls"; then
    pass "a second run changes nothing but writing the ruleset again"
else fail "a second run changes nothing ($(writes))"; fi

# Settings from flags: a variable changed, the retired origin added, an
# existing variable left as it is when its flag is not given.
: > "$C/calls"
if run --i-know-what-i-am-doing --site-domain example.run --retired-content-origin https://old.example \
    && writes | grep -qF "PATCH repos/$R/environments/production/variables/SITE_DOMAIN {\"name\":\"SITE_DOMAIN\",\"value\":\"example.run\"}" \
    && writes | grep -qF "POST repos/$R/environments/production/variables {\"name\":\"RETIRED_CONTENT_ORIGIN\",\"value\":\"https://old.example\"}" \
    && [ "$(writes | grep -c variables)" = 2 ]; then
    pass "a changed variable is updated and a new one created, the rest left"
else fail "flags update the variables ($(writes))"; fi
: > "$C/calls"
if run --i-know-what-i-am-doing && [ "$(q 'g["envs"]["production"]["vars"]["RETIRED_CONTENT_ORIGIN"]')" = https://old.example ]; then
    pass "a variable with no flag is kept"; else fail "a variable with no flag is kept"; fi

# --commit in place of clax.dev's, and a commit not on main.
new_case commit
if run --i-know-what-i-am-doing --commit fedcba98; then fail "a commit not on main is refused"
elif grep -q "is not on main" "$C/out" && [ -z "$(writes)" ]; then pass "a commit not on main is refused before any write"
else fail "a commit not on main is refused ($(cat "$C/out"))"; fi
python3 -c 'import json, sys; p = sys.argv[1]; g = json.load(open(p)); g["on_main"].append(sys.argv[2]); json.dump(g, open(p, "w"))' \
    "$C/github.json" fedcba9876543210fedcba9876543210fedcba98
: > "$C/calls"
if run --i-know-what-i-am-doing --commit fedcba98 && [ "$(q 'g["refs"]["prod"]')" = fedcba9876543210fedcba9876543210fedcba98 ] \
    && ! grep -q '^curl ' "$C/calls"; then
    pass "--commit names where prod starts, and clax.dev is not asked"
else fail "--commit names where prod starts ($(cat "$C/out"))"; fi

# clax.dev without a commit, and an error from GitHub that is not a 404.
new_case nocommit
echo '{"version":"0.6.0","commit":null}' > "$C/healthz"
if run; then fail "a healthz without a commit is refused"
elif grep -q "reports no commit; pass --commit" "$C/out"; then pass "a healthz without a commit is refused, naming --commit"
else fail "a healthz without a commit is refused ($(cat "$C/out"))"; fi
new_case forbidden
: > "$C/fail-repos_${R//\//_}_rulesets?includes_parents=false"
if run --i-know-what-i-am-doing; then fail "an error other than a 404 stops the run"
elif grep -q "HTTP 403" "$C/out" && ! writes | grep -q "rulesets\|environments"; then pass "an error other than a 404 stops the run where it happens"
else fail "an error other than a 404 stops the run ($(cat "$C/out"); $(writes))"; fi

exit "$FAILED"
