#!/usr/bin/env bash
# Sets up the GitHub side of releases and deploys from the `prod` branch
# (docs/prod.md): the branch, its ruleset, the `production` environment and
# its variables, and the `npm` environment's branch policy. Run it yourself,
# from any directory, with a gh signed in as a repository admin. It is safe
# to run again: it reads what exists first and changes only what differs
# from what it sets up, except the ruleset, which it writes whole each time.
#
# Without --i-know-what-i-am-doing it is a dry run: it makes the same reads
# and prints every write (`gh api --method ...` with its body) it would
# make, changing nothing.
#
#   scripts/setup-prod-github.sh                            # dry run
#   scripts/setup-prod-github.sh --i-know-what-i-am-doing   # act
#
# Options (defaults are clax.dev's):
#   --repo <owner/name>             empathic/clax
#   --commit <sha>                  where `prod` starts, only when it does not
#                                   exist (default: the `commit` that
#                                   https://<public host>/healthz reports);
#                                   it must be on main
#   --public-host <host>            clax.dev
#   --site-domain <domain>          clax.run
#   --store <artifacts|memory>      artifacts
#   --owner-github <login>          akesling
#   --github-client-id <ID>         Iv1.exampleclient
#   --cloudflare-account-id <ID>    00000000000000000000000000000000
#   --retired-content-origin <URL>  none (an existing variable is kept)
#
# In order, it
#
# 1. creates the branch `prod` at the commit, unless it exists;
# 2. creates, or updates, the repository ruleset `prod` on refs/heads/prod:
#    a pull request, merged with a merge commit, with CI's `bazel`, `worker`
#    and `version` checks passed; no deletion, no force push, signed commits;
#    repository admins bypass it (scripts/release.sh pushes the pin commit);
# 3. creates the environment `production`, deployable from `prod` only, and
#    sets its variables;
# 4. sets the repository variable PUBLIC_HOST, which the hourly drift check
#    (.github/workflows/deploy-prod.yml) reads outside the environment;
# 5. makes the environment `npm` deployable from `prod` in place of `main`;
# 6. prints the `gh secret set` commands for the production environment's
#    two secrets, which it never runs: their values are yours to paste.
#
# The REST API shapes are GitHub's:
#   refs          https://docs.github.com/en/rest/git/refs#create-a-reference
#   compare       https://docs.github.com/en/rest/commits/commits#compare-two-commits
#   rulesets      https://docs.github.com/en/rest/repos/rules#create-a-repository-ruleset
#                 https://docs.github.com/en/rest/repos/rules#update-a-repository-ruleset
#   environments  https://docs.github.com/en/rest/deployments/environments#create-or-update-an-environment
#   branch policy https://docs.github.com/en/rest/deployments/branch-policies
#   variables     https://docs.github.com/en/rest/actions/variables#create-an-environment-variable
#                 https://docs.github.com/en/rest/actions/variables#update-an-environment-variable
#                 https://docs.github.com/en/rest/actions/variables#create-a-repository-variable
#                 https://docs.github.com/en/rest/actions/variables#update-a-repository-variable
# The variables' settings are read by name (${!name}), out of shellcheck's sight.
# shellcheck disable=SC2034
set -euo pipefail

act=0
repo="empathic/clax"
commit=""
# The production environment's variables, which the deploy workflow passes
# to scripts/cloudflare-setup.sh; step 3 reads them by name. The GitHub
# OAuth App's client ID is OAUTH_CLIENT_ID: GitHub refuses a variable whose
# name starts with GITHUB_.
PUBLIC_HOST="clax.dev"
SITE_DOMAIN="clax.run"
STORE="artifacts"
OWNER_GITHUB="akesling"
OAUTH_CLIENT_ID="Iv1.exampleclient"
CLOUDFLARE_ACCOUNT_ID="00000000000000000000000000000000"
RETIRED_CONTENT_ORIGIN=""
usage() { sed -n '2,33p' "$0"; }
while [ $# -gt 0 ]; do
    case "$1" in
        --i-know-what-i-am-doing) act=1 ;;
        --dry-run) act=0 ;;
        --repo) repo="$2"; shift ;;
        --commit) commit="$2"; shift ;;
        --public-host) PUBLIC_HOST="$2"; shift ;;
        --site-domain) SITE_DOMAIN="$2"; shift ;;
        --store) STORE="$2"; shift ;;
        --owner-github) OWNER_GITHUB="$2"; shift ;;
        --github-client-id) OAUTH_CLIENT_ID="$2"; shift ;;
        --cloudflare-account-id) CLOUDFLARE_ACCOUNT_ID="$2"; shift ;;
        --retired-content-origin) RETIRED_CONTENT_ORIGIN="$2"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "setup-prod-github: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done
public_host="$PUBLIC_HOST"

say() { printf '\n== %s\n' "$*"; }
info() { printf '   %s\n' "$*"; }
die() { printf 'setup-prod-github: %s\n' "$*" >&2; exit 1; }
command -v gh >/dev/null || die "gh is not installed (https://cli.github.com), or not on PATH"
command -v python3 >/dev/null || die "python3 is not on PATH"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# get PATH: prints the JSON GitHub answers; returns 1 on a 404 (it does not
# exist) and fails the script on any other error.
get() {
    local out
    if out="$(gh api "$1" 2>"$work/err")"; then
        printf '%s\n' "$out"
        return 0
    fi
    grep -q 'HTTP 404' "$work/err" && return 1
    die "gh api $1: $(cat "$work/err")"
}

# write METHOD PATH [JSON]: the write, made when acting, printed otherwise.
write() {
    local method="$1" path="$2" body="${3:-}"
    if [ "$act" = 0 ]; then
        if [ -n "$body" ]; then info "would run: gh api --method $method $path --input - <<< '$body'"
        else info "would run: gh api --method $method $path"; fi
        return 0
    fi
    info "gh api --method $method $path"
    if [ -n "$body" ]; then
        printf '%s' "$body" > "$work/body"
        gh api --method "$method" "$path" --input "$work/body" >/dev/null
    else
        gh api --method "$method" "$path" >/dev/null
    fi
}

# json EXPR: evaluates the Python expression EXPR over the JSON on stdin,
# bound to `j`, and prints the result ("" for None).
json() { python3 -c 'import json, sys; j = json.load(sys.stdin); r = eval(sys.argv[1]); print("" if r is None else r)' "$1"; }

[ "$act" = 1 ] && echo "setup-prod-github: acting on $repo" || echo "setup-prod-github: dry run on $repo (nothing changes; --i-know-what-i-am-doing acts)"

say "1. Branch prod"
if ref="$(get "repos/$repo/git/ref/heads/prod")"; then
    info "exists at $(json 'j["object"]["sha"]' <<< "$ref"); left as it is"
else
    if [ -z "$commit" ]; then
        health="$(curl -fsS --max-time 30 "https://$public_host/healthz")" \
            || die "cannot read https://$public_host/healthz; pass --commit <sha>"
        commit="$(json 'j.get("commit")' <<< "$health")"
        [ -n "$commit" ] || die "https://$public_host/healthz reports no commit; pass --commit <sha>"
        info "https://$public_host/healthz reports commit $commit"
    fi
    sha="$(get "repos/$repo/commits/$commit" | json 'j["sha"]')" || die "$repo has no commit $commit"
    # main...sha is "behind" or "identical" exactly when sha is on main.
    status="$(get "repos/$repo/compare/main...$sha" | json 'j["status"]')"
    case "$status" in
        behind|identical) info "$sha is on main" ;;
        *) die "$sha is not on main (main...$sha is $status); pass --commit <a commit on main>" ;;
    esac
    write POST "repos/$repo/git/refs" "{\"ref\":\"refs/heads/prod\",\"sha\":\"$sha\"}"
fi

say "2. Ruleset prod on refs/heads/prod"
# actor_id 5 with actor_type RepositoryRole is the repository's admin role;
# integration_id 15368 is GitHub Actions, so only its checks count.
ruleset='{"name":"prod","target":"branch","enforcement":"active",'\
'"conditions":{"ref_name":{"include":["refs/heads/prod"],"exclude":[]}},'\
'"bypass_actors":[{"actor_id":5,"actor_type":"RepositoryRole","bypass_mode":"always"}],'\
'"rules":[{"type":"deletion"},{"type":"non_fast_forward"},{"type":"required_signatures"},'\
'{"type":"pull_request","parameters":{"required_approving_review_count":0,"dismiss_stale_reviews_on_push":false,'\
'"require_code_owner_review":false,"require_last_push_approval":false,"required_review_thread_resolution":false,'\
'"allowed_merge_methods":["merge"]}},'\
'{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,'\
'"required_status_checks":[{"context":"bazel","integration_id":15368},{"context":"worker","integration_id":15368},'\
'{"context":"version","integration_id":15368}]}}]}'
id="$(get "repos/$repo/rulesets?includes_parents=false" | json 'next((r["id"] for r in j if r["name"] == "prod"), None)')"
if [ -n "$id" ]; then
    info "exists (ID $id); set to the rules above"
    write PUT "repos/$repo/rulesets/$id" "$ruleset"
else
    write POST "repos/$repo/rulesets" "$ruleset"
fi

# branch_policy ENV: makes ENV deployable from `prod` alone.
branch_policy() {
    local env="$1" e custom policies keep=0 pid name
    e="$(get "repos/$repo/environments/$env")" || e='{}'
    custom="$(json '(j.get("deployment_branch_policy") or {}).get("custom_branch_policies") is True' <<< "$e")"
    if [ "$custom" = True ]; then
        info "$env deploys from selected branches"
    else
        if [ "$e" = '{}' ]; then info "$env does not exist"; else info "$env deploys from any branch, or protected branches"; fi
        write PUT "repos/$repo/environments/$env" \
            '{"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}'
    fi
    policies=""
    [ "$custom" = True ] && policies="$(get "repos/$repo/environments/$env/deployment-branch-policies" \
        | json '"\n".join("%s %s:%s" % (p["id"], p.get("type", "branch"), p["name"]) for p in j["branch_policies"])')"
    while read -r pid name; do
        [ -n "$pid" ] || continue
        if [ "$name" = branch:prod ]; then keep=1; info "$env: branch prod is allowed"; fi
    done <<< "$policies"
    [ "$keep" = 1 ] || write POST "repos/$repo/environments/$env/deployment-branch-policies" '{"name":"prod","type":"branch"}'
    while read -r pid name; do
        [ -n "$pid" ] && [ "$name" != branch:prod ] || continue
        info "$env: $name is allowed; removed"
        write DELETE "repos/$repo/environments/$env/deployment-branch-policies/$pid"
    done <<< "$policies"
}

# set_var VARIABLES NAME VALUE: creates the variable NAME under the
# collection path VARIABLES, or updates it when its value differs.
set_var() {
    local at="$1" name="$2" value="$3" have body
    body="{\"name\":\"$name\",\"value\":\"$value\"}"
    if have="$(get "$at/$name")"; then
        have="$(json 'j["value"]' <<< "$have")"
        if [ "$have" = "$value" ]; then info "$name=$value already"; return 0; fi
        info "$name is $have; set to $value"
        write PATCH "$at/$name" "$body"
    else
        write POST "$at" "$body"
    fi
}

say "3. Environment production"
branch_policy production
for name in CLOUDFLARE_ACCOUNT_ID OAUTH_CLIENT_ID OWNER_GITHUB PUBLIC_HOST RETIRED_CONTENT_ORIGIN SITE_DOMAIN STORE; do
    [ -z "${!name}" ] || set_var "repos/$repo/environments/production/variables" "$name" "${!name}"
done
[ -n "$RETIRED_CONTENT_ORIGIN" ] || info "RETIRED_CONTENT_ORIGIN: not given, so left as it is (pass --retired-content-origin when the instance's deploy record names one)"

# The drift job runs outside the environment, so it reads the repository's.
say "4. Repository variable PUBLIC_HOST (the drift check's host)"
set_var "repos/$repo/actions/variables" PUBLIC_HOST "$PUBLIC_HOST"

say "5. Environment npm"
branch_policy npm

say "6. Secrets (yours to run; this script never sets them)"
info "gh secret set CLOUDFLARE_API_TOKEN --env production --repo $repo   # the token docs/prod.md describes"
info "gh secret set CLAX_SMOKE_TOKEN --env production --repo $repo       # an agent token named smoke, from https://$public_host's settings"
[ "$act" = 1 ] && echo "setup-prod-github: done" || echo "setup-prod-github: dry run done; run again with --i-know-what-i-am-doing to act"
