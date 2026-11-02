#!/usr/bin/env bash
# Tests scripts/setup-env-github.sh against a fake gh on PATH; the real gh
# is never run. The fake gh plays GitHub's REST API from
# $FAKE_STATE/github.json (environments with their branch policies and
# variables), applies every write to it, and logs each call ("METHOD path
# [body]") to $FAKE_STATE/calls. A GET of something that does not exist
# fails as gh does, with "HTTP 404".
# The fake's helper functions run through the cases, out of shellcheck's sight.
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
SCRIPT="$HERE/scripts/setup-env-github.sh"
R=empathic/clax

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

p = path.split("?")[0]
m = re.fullmatch(r"repos/[^/]+/[^/]+/(.*)", p)
if not m: missing()
rest = m.group(1)
envs = db.setdefault("envs", {})
if method == "GET":
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
if method == "PUT" and (m := re.fullmatch(r"environments/([^/]+)", rest)):
    e = envs.setdefault(m.group(1), {"branches": [], "vars": {}})
    e["policy"] = body["deployment_branch_policy"]
    if e["policy"] is None: e["branches"] = []
    save()
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

# A case: a state directory with GitHub as it is before any setup (no dev
# or preview environment; production and npm as setup-prod-github.sh left
# them), and a deploy configuration as `scripts/cloudflare-setup.sh --env
# dev` keeps it, its client secret and owner token included, which the
# script must never read out.
C=""
new_case() {
    C="$T/case-$1"
    mkdir -p "$C"
    cat > "$C/github.json" <<JSON
{"envs": {"production": {"policy": {"protected_branches": false, "custom_branch_policies": true},
                         "branches": [{"id": 21, "name": "prod", "type": "branch"}],
                         "vars": {"PUBLIC_HOST": "clax.example"}},
          "npm": {"policy": {"protected_branches": false, "custom_branch_policies": true},
                  "branches": [{"id": 11, "name": "prod", "type": "branch"}], "vars": {}}}}
JSON
    mkdir -p "$C/deploy"
    cat > "$C/deploy/setup.dev.env" <<ENV
PUBLIC_HOST=dev.example
SITE_DOMAIN=sites.dev.example
SITE_ZONE=dev.example
STORE=artifacts
GITHUB_CLIENT_ID=Iv1.devclient
OWNER_GITHUB=octo-owner
GITHUB_CLIENT_SECRET=$SECRET
ENV
    echo "{\"account\": \"$ACC\", \"owner_token\": \"$SECRET\", \"setup_secret\": \"$SECRET\"}" > "$C/deploy/dev.example.json"
    : > "$C/calls"
}
SECRET=never-printed-0f0f0f
ACC=00000000000000000000000000acc0de
PREVIEW=(--preview-host-zone preview.example --preview-site-zone previewsites.example
         --github-client-id Iv1.previewclient --owner-github octo-owner --cloudflare-account-id "$ACC")
run() { (cd "$T" && FAKE_STATE="$C" CLAX_DEPLOY_HOME="$C/deploy" PATH="$T/bin:$PATH" "$SCRIPT" "$@" > "$C/out" 2>&1); }
# q EXPR: the Python expression EXPR over the case's GitHub, bound to `g`.
q() { python3 -c 'import json, sys; g = json.load(open(sys.argv[1])); print(eval(sys.argv[2]))' "$C/github.json" "$1"; }
writes() { grep -v '^GET ' "$C/calls"; }

# Dev, dry run: reads only, and every write printed.
new_case dev-dry
cp "$C/github.json" "$C/before.json"
if run --env dev; then pass "dev: the dry run succeeds"; else fail "dev: the dry run succeeds ($(cat "$C/out"))"; fi
if [ -z "$(writes)" ] && cmp -s "$C/github.json" "$C/before.json"; then pass "dev: the dry run makes no write"
else fail "dev: the dry run makes no write ($(writes))"; fi
for want in \
    "would run: gh api --method PUT repos/$R/environments/dev --input - <<< '{\"deployment_branch_policy\":{\"protected_branches\":false,\"custom_branch_policies\":true}}'" \
    "would run: gh api --method POST repos/$R/environments/dev/deployment-branch-policies --input - <<< '{\"name\":\"main\",\"type\":\"branch\"}'" \
    "would run: gh api --method POST repos/$R/environments/dev/variables --input - <<< '{\"name\":\"PUBLIC_HOST\",\"value\":\"dev.example\"}'" \
    "would run: gh api --method POST repos/$R/environments/dev/variables --input - <<< '{\"name\":\"SITE_ZONE\",\"value\":\"dev.example\"}'" \
    "gh secret set CLOUDFLARE_ACCOUNT_ID --env dev --repo $R --body $ACC" \
    "gh secret set OAUTH_CLIENT_ID --env dev --repo $R --body Iv1.devclient" \
    "gh secret set CLOUDFLARE_API_TOKEN --env dev --repo $R" \
    "gh secret set OAUTH_CLIENT_SECRET --env dev --repo $R" \
    "gh secret set CLAX_SMOKE_TOKEN --env dev --repo $R"; do
    if grep -qF -- "$want" "$C/out"; then pass "dev: the dry run prints: ${want:0:90}"; else fail "dev: the dry run prints: $want ($(cat "$C/out"))"; fi
done

# Dev, acting: dev deployable from main only, with the configuration's
# variables; a branch allowed before is removed.
new_case dev-act
python3 -c 'import json, sys; p = sys.argv[1]; g = json.load(open(p))
g["envs"]["dev"] = {"policy": {"protected_branches": False, "custom_branch_policies": True},
                    "branches": [{"id": 31, "name": "feature", "type": "branch"}], "vars": {}}
json.dump(g, open(p, "w"))' "$C/github.json"
if run --env dev --i-know-what-i-am-doing; then pass "dev: acting succeeds"; else fail "dev: acting succeeds ($(cat "$C/out"))"; fi
if [ "$(q 'g["envs"]["dev"]["policy"]["custom_branch_policies"]')" = True ] \
    && [ "$(q '[b["name"] for b in g["envs"]["dev"]["branches"]]')" = "['main']" ] \
    && grep -q "^DELETE repos/$R/environments/dev/deployment-branch-policies/31$" "$C/calls"; then
    pass "dev: deploys from main only, the branch allowed before removed"
else fail "dev: deploys from main only ($(q 'g["envs"]["dev"]'))"; fi
if [ "$(q 'sorted(g["envs"]["dev"]["vars"].items())')" = "[('OWNER_GITHUB', 'octo-owner'), ('PUBLIC_HOST', 'dev.example'), ('SITE_DOMAIN', 'sites.dev.example'), ('SITE_ZONE', 'dev.example'), ('STORE', 'artifacts')]" ]; then
    pass "dev: its variables are the deploy configuration's"; else fail "dev: its variables ($(q 'g["envs"]["dev"]["vars"]'))"; fi
if grep -q "UNEXPECTED\|secret" "$C/calls"; then fail "dev: no gh call but gh api, and no secret set ($(cat "$C/calls"))"; else pass "dev: no gh call but gh api, and no secret set"; fi

# Again: nothing is written.
: > "$C/calls"
if run --env dev --i-know-what-i-am-doing && [ -z "$(writes)" ]; then pass "dev: a second run makes no write"
else fail "dev: a second run makes no write ($(writes))"; fi

# Flags over the configuration: only the changed variables are written.
: > "$C/calls"
if run --env dev --i-know-what-i-am-doing --site-domain sites.other.example --store memory \
    && writes | grep -F "PATCH repos/$R/environments/dev/variables/SITE_DOMAIN {\"name\":\"SITE_DOMAIN\",\"value\":\"sites.other.example\"}" >/dev/null \
    && writes | grep -F "PATCH repos/$R/environments/dev/variables/STORE {\"name\":\"STORE\",\"value\":\"memory\"}" >/dev/null \
    && [ "$(writes | wc -l | tr -d ' ')" = 2 ]; then
    pass "dev: flags take the configuration's place, and only changed variables are written"
else fail "dev: flags take the configuration's place ($(writes))"; fi

# No deploy configuration: the run names the missing flags, before any call.
new_case dev-noconfig
rm -rf "$C/deploy"
if run --env dev; then fail "dev: with no deploy configuration and no flags the run fails"
elif grep -q "not given: --public-host --site-domain --cloudflare-account-id --github-client-id --owner-github" "$C/out" && [ ! -s "$C/calls" ]; then
    pass "dev: with no deploy configuration the run names the missing flags, before any call"
else fail "dev: with no deploy configuration the run names the missing flags ($(cat "$C/out"); $(cat "$C/calls"))"; fi
# GitHub sign-in is required: settings without it are refused.
: > "$C/calls"
if run --env dev --public-host flags.example --site-domain sites.flags --cloudflare-account-id f1a65; then
    fail "dev: no GitHub sign-in is refused"
elif grep -q "not given: --github-client-id --owner-github" "$C/out" && [ ! -s "$C/calls" ]; then
    pass "dev: no GitHub sign-in is refused, naming --github-client-id and --owner-github"
else fail "dev: no GitHub sign-in is refused ($(cat "$C/out"))"; fi
: > "$C/calls"
if run --env dev --public-host flags.example --site-domain sites.flags --cloudflare-account-id f1a65 --github-client-id Iv1.x; then
    fail "dev: a client ID without an owner login is refused"
elif grep -q "not given: --owner-github" "$C/out" && [ ! -s "$C/calls" ]; then pass "dev: a client ID without an owner login is refused, naming --owner-github"
else fail "dev: a client ID without an owner login is refused ($(cat "$C/out"))"; fi
: > "$C/calls"
if run --env dev --public-host flags.example --site-domain sites.flags --cloudflare-account-id f1a65 \
    --github-client-id Iv1.x --owner-github octo \
    && grep -qF "gh secret set CLOUDFLARE_ACCOUNT_ID --env dev --repo $R --body f1a65" "$C/out" && ! grep -qF '"name":"SITE_ZONE"' "$C/out"; then
    pass "dev: flags alone set it up, SITE_ZONE left without one"
else fail "dev: flags alone set it up ($(cat "$C/out"))"; fi

# No --env, or another: refused before any call.
new_case noenv
if run; then fail "no --env is refused"
elif grep -q "\-\-env dev or --env preview is required" "$C/out" && [ ! -s "$C/calls" ]; then pass "no --env is refused, before any call"
else fail "no --env is refused ($(cat "$C/out"))"; fi
if run --env production; then fail "--env production is refused"
elif grep -q "is not dev or preview" "$C/out" && [ ! -s "$C/calls" ]; then pass "--env production is refused, before any call"
else fail "--env production is refused ($(cat "$C/out"))"; fi

# Preview, dry run.
new_case preview-dry
cp "$C/github.json" "$C/before.json"
if run --env preview "${PREVIEW[@]}"; then pass "preview: the dry run succeeds"; else fail "preview: the dry run succeeds ($(cat "$C/out"))"; fi
if [ -z "$(writes)" ] && cmp -s "$C/github.json" "$C/before.json"; then pass "preview: the dry run makes no write"
else fail "preview: the dry run makes no write ($(writes))"; fi
for want in \
    "would run: gh api --method PUT repos/$R/environments/preview --input - <<< '{\"deployment_branch_policy\":null}'" \
    "would run: gh api --method POST repos/$R/environments/preview/variables --input - <<< '{\"name\":\"PREVIEW_HOST_ZONE\",\"value\":\"preview.example\"}'" \
    "would run: gh api --method POST repos/$R/environments/preview/variables --input - <<< '{\"name\":\"PREVIEW_SITE_ZONE\",\"value\":\"previewsites.example\"}'" \
    "gh secret set CLOUDFLARE_ACCOUNT_ID --env preview --repo $R --body $ACC" \
    "gh secret set OAUTH_CLIENT_ID --env preview --repo $R --body Iv1.previewclient" \
    "gh secret set CLOUDFLARE_API_TOKEN --env preview --repo $R" \
    "gh secret set OAUTH_CLIENT_SECRET --env preview --repo $R" \
    "https://preview.example/auth/github/callback, with wildcard matching on"; do
    if grep -qF -- "$want" "$C/out"; then pass "preview: the dry run prints: ${want:0:90}"; else fail "preview: the dry run prints: $want ($(cat "$C/out"))"; fi
done
if grep -q "deployment-branch-policies\|CLAX_SMOKE_TOKEN" "$C/out"; then fail "preview: no branch policy and no smoke token ($(cat "$C/out"))"
else pass "preview: no branch policy and no smoke token"; fi

# Preview, acting: from any branch, a branch policy set before removed.
new_case preview-act
python3 -c 'import json, sys; p = sys.argv[1]; g = json.load(open(p))
g["envs"]["preview"] = {"policy": {"protected_branches": True, "custom_branch_policies": False}, "branches": [], "vars": {}}
json.dump(g, open(p, "w"))' "$C/github.json"
if run --env preview --i-know-what-i-am-doing "${PREVIEW[@]}"; then pass "preview: acting succeeds"; else fail "preview: acting succeeds ($(cat "$C/out"))"; fi
if [ "$(q 'g["envs"]["preview"]["policy"]')" = None ] && [ "$(q 'g["envs"]["preview"]["branches"]')" = "[]" ]; then
    pass "preview: deploys from any branch, the protected-branches policy removed"
else fail "preview: deploys from any branch ($(q 'g["envs"]["preview"]'))"; fi
if [ "$(q 'sorted(g["envs"]["preview"]["vars"].items())')" = "[('OWNER_GITHUB', 'octo-owner'), ('PREVIEW_HOST_ZONE', 'preview.example'), ('PREVIEW_SITE_ZONE', 'previewsites.example')]" ]; then
    pass "preview: its variables are the flags'"; else fail "preview: its variables ($(q 'g["envs"]["preview"]["vars"]'))"; fi
: > "$C/calls"
if run --env preview --i-know-what-i-am-doing "${PREVIEW[@]}" && [ -z "$(writes)" ]; then pass "preview: a second run makes no write"
else fail "preview: a second run makes no write ($(writes))"; fi

# Preview settings are flags alone, and the zones may not overlap.
new_case preview-missing
if run --env preview; then fail "preview: no flags is refused"
elif grep -q "not given: --preview-host-zone --preview-site-zone --cloudflare-account-id --github-client-id --owner-github" "$C/out" && [ ! -s "$C/calls" ]; then
    pass "preview: no flags is refused, naming them all, before any call (the dev configuration is not read)"
else fail "preview: no flags is refused ($(cat "$C/out"))"; fi
for zones in "preview.example preview.example" "pr.preview.example preview.example" "preview.example sites.Preview.example"; do
    read -r hz sz <<< "$zones"
    : > "$C/calls"
    if run --env preview "${PREVIEW[@]}" --preview-host-zone "$hz" --preview-site-zone "$sz"; then
        fail "preview: zones $hz and $sz are refused"
    elif grep -q "sites would share the previews' hosts" "$C/out" && [ ! -s "$C/calls" ]; then
        pass "preview: zones $hz and $sz are refused, before any call"
    else fail "preview: zones $hz and $sz are refused ($(cat "$C/out"))"; fi
done
: > "$C/calls"
if run --env preview "${PREVIEW[@]}" --owner-github 'octo","x'; then fail "a value that is not a hostname, ID or login is refused"
elif grep -q "is not a hostname, an ID or a login" "$C/out" && [ ! -s "$C/calls" ]; then pass "a value that is not a hostname, ID or login is refused, before any call"
else fail "a value that is not a hostname, ID or login is refused ($(cat "$C/out"))"; fi

# Neither environment touches production or npm.
for c in "$T"/case-*; do
    [ -f "$c/calls" ] || continue
    if grep -q "environments/production\|environments/npm\|rulesets\|git/ref\|actions/variables" "$c/calls"; then
        fail "production, npm, rulesets, branches and repository variables are never touched ($(basename "$c"): $(grep "production\|npm\|rulesets\|git/ref\|actions/variables" "$c/calls"))"
        TOUCHED=1
    fi
done
[ "${TOUCHED:-0}" = 1 ] || pass "production, npm, rulesets, branches and repository variables are never touched"

# The deploy configuration's secrets are never read out, in any case.
if grep -rqF "$SECRET" "$T"/case-*/out "$T"/case-*/calls "$T"/case-*/github.json; then
    fail "the client secret and the record's secrets never reach gh or the output"
else pass "the client secret and the record's secrets never reach gh or the output"; fi

exit "$FAILED"
