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
# The instance's settings come from your deploy configuration, the one
# scripts/cloudflare-setup.sh keeps: setup.env (PUBLIC_HOST, SITE_DOMAIN,
# STORE, GITHUB_CLIENT_ID, OWNER_GITHUB; nothing else in it is read) and the
# deploy record <public host>.json (its `account` and
# `retired_content_origin`), in $CLAX_DEPLOY_HOME, else
# $XDG_CONFIG_HOME/clax/deploy, else ~/.config/clax/deploy. A flag takes the
# place of each; a setting found in neither fails the run, naming its flag.
# None is written into this repository.
#
# Options:
#   --repo <owner/name>             empathic/clax
#   --commit <sha>                  where `prod` starts, only when it does not
#                                   exist (default: the `commit` that
#                                   https://<public host>/healthz reports);
#                                   it must be on main
#   --public-host <host>
#   --site-domain <domain>
#   --store <artifacts|memory>      (artifacts when setup.env names none)
#   --owner-github <login>          the owner, who approves production
#                                   deploys; with --github-client-id
#   --github-client-id <ID>
#   --staging-host-zone <zone>      clax-staging.dev
#   --staging-site-zone <zone>      clax-staging.run
#   --cloudflare-account-id <ID>
#   --retired-content-origin <URL>  the record's `retired_content_origin`, else
#                                   none (an existing variable is kept)
#
# In order, it
#
# 1. creates the branch `prod` at the commit, unless it exists;
# 2. creates, or updates, the repository ruleset `prod` on refs/heads/prod:
#    a pull request, merged with a merge commit, with CI's `bazel`, `worker`
#    and `version` checks passed; no deletion, no force push, signed commits;
#    updates restricted to the bypass actors, the repository admins, who
#    alone merge into prod (anyone with write access opens pull requests)
#    and may push to it directly (no release step does: a release reaches it
#    by its pull request's merge alone);
# 3. creates the environment `production`, deployable from `prod` only, and
#    sets its variables (PUBLIC_HOST, SITE_DOMAIN, STORE and
#    RETIRED_CONTENT_ORIGIN), removing any variable named as one of its
#    secrets;
# 4. sets the repository variable PUBLIC_HOST, which the hourly drift check
#    (.github/workflows/deploy-prod.yml) reads outside the environment;
# 5. makes the environment `npm` deployable from `prod` and `main` alone
#    (publish-pi.yml and publish-sdk.yml run in it from main);
# 6. creates, or updates, the repository ruleset `release` on
#    refs/heads/release/*: only its bypass actors, the repository admins,
#    create, update or delete a release branch, so only they run code with
#    the staging environment's secrets;
# 7. makes a production deploy wait for the owner's approval (the
#    environment's required reviewer, OWNER_GITHUB);
# 8. creates the environment `staging`, deployable from release/* and main
#    alone (GitHub matches the rule against a run's GITHUB_REF, which a
#    pull_request run has as refs/pull/<N>/merge, so no pull request's run
#    reaches it), with the variables STAGING_HOST_ZONE and STAGING_SITE_ZONE;
# 9. creates the environment `release`, deployable from prod alone, for the
#    release GitHub App's secrets;
# 10. prints the `gh secret set` commands for the environments' secrets,
#    which it never runs: production's CLOUDFLARE_ACCOUNT_ID,
#    OAUTH_CLIENT_ID and OWNER_GITHUB with their values from the deploy
#    configuration, and the rest, whose values are yours to paste.
#
# The REST API shapes are GitHub's:
#   refs          https://docs.github.com/en/rest/git/refs#create-a-reference
#   compare       https://docs.github.com/en/rest/commits/commits#compare-two-commits
#   rulesets      https://docs.github.com/en/rest/repos/rules#create-a-repository-ruleset
#                 https://docs.github.com/en/rest/repos/rules#update-a-repository-ruleset
#   restrict updates (rule type `update`)
#                 https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets#restrict-updates
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
# The production environment's settings, which the deploy workflow passes
# to scripts/cloudflare-setup.sh; steps 3 and 6 read them by name. The
# first four are variables; OWNER_GITHUB, OAUTH_CLIENT_ID and
# CLOUDFLARE_ACCOUNT_ID are secrets. The GitHub OAuth App's client ID is
# OAUTH_CLIENT_ID: GitHub refuses a secret whose name starts with GITHUB_.
PUBLIC_HOST=""
SITE_DOMAIN=""
STORE=""
OWNER_GITHUB=""
OAUTH_CLIENT_ID=""
CLOUDFLARE_ACCOUNT_ID=""
RETIRED_CONTENT_ORIGIN=""
STAGING_HOST_ZONE="clax-staging.dev"
STAGING_SITE_ZONE="clax-staging.run"
usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d'; }
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
        --staging-host-zone) STAGING_HOST_ZONE="$2"; shift ;;
        --staging-site-zone) STAGING_SITE_ZONE="$2"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "setup-prod-github: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

# The deploy configuration's settings, each only where no flag gave it.
deploy_home="${CLAX_DEPLOY_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/clax/deploy}"
# setting FILE KEY: the value of KEY=… in setup.env, or nothing. Reads that
# one line, so the file's GITHUB_CLIENT_SECRET is never read into the run.
setting() {
    [ -f "$1" ] || return 0
    sed -n "s/^$2=//p" "$1" | tail -n 1
}
env_file="$deploy_home/setup.env"
[ -n "$PUBLIC_HOST" ] || PUBLIC_HOST="$(setting "$env_file" PUBLIC_HOST)"
[ -n "$SITE_DOMAIN" ] || SITE_DOMAIN="$(setting "$env_file" SITE_DOMAIN)"
[ -n "$STORE" ] || STORE="$(setting "$env_file" STORE)"
[ -n "$STORE" ] || STORE="artifacts"
[ -n "$OWNER_GITHUB" ] || OWNER_GITHUB="$(setting "$env_file" OWNER_GITHUB)"
[ -n "$OAUTH_CLIENT_ID" ] || OAUTH_CLIENT_ID="$(setting "$env_file" GITHUB_CLIENT_ID)"
# record KEY: KEY's value in the deploy record, or nothing; the record's
# other fields (its secrets among them) are not printed.
record() {
    [ -n "$PUBLIC_HOST" ] && [ -f "$deploy_home/$PUBLIC_HOST.json" ] || return 0
    python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get(sys.argv[2]) or "")' "$deploy_home/$PUBLIC_HOST.json" "$1"
}
[ -n "$CLOUDFLARE_ACCOUNT_ID" ] || CLOUDFLARE_ACCOUNT_ID="$(record account)"
[ -n "$RETIRED_CONTENT_ORIGIN" ] || RETIRED_CONTENT_ORIGIN="$(record retired_content_origin)"
missing=""
[ -n "$PUBLIC_HOST" ] || missing="$missing --public-host"
[ -n "$SITE_DOMAIN" ] || missing="$missing --site-domain"
[ -n "$CLOUDFLARE_ACCOUNT_ID" ] || missing="$missing --cloudflare-account-id"
# The owner approves production deploys (step 7), so there is one.
[ -n "$OWNER_GITHUB" ] || missing="$missing --owner-github"
if [ -n "$OWNER_GITHUB" ] && [ -z "$OAUTH_CLIENT_ID" ]; then missing="$missing --github-client-id"; fi
if [ -n "$missing" ]; then
    echo "setup-prod-github: not in $deploy_home and not given:$missing" >&2
    exit 2
fi
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
# integration_id 15368 is GitHub Actions, so only its checks count. The
# `update` rule lets only the bypass actors update prod, so only an admin
# merges a pull request into it: GitHub cannot narrow a bypass to one
# person, so every admin can. No approval is required: the owner cannot
# approve the release pull request scripts/release.sh opens as them.
ruleset='{"name":"prod","target":"branch","enforcement":"active",'\
'"conditions":{"ref_name":{"include":["refs/heads/prod"],"exclude":[]}},'\
'"bypass_actors":[{"actor_id":5,"actor_type":"RepositoryRole","bypass_mode":"always"}],'\
'"rules":[{"type":"deletion"},{"type":"non_fast_forward"},{"type":"required_signatures"},'\
'{"type":"update","parameters":{"update_allows_fetch_and_merge":false}},'\
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

# branch_policy ENV BRANCH...: makes ENV deployable from the BRANCHes alone.
branch_policy() {
    local env="$1" e custom policies pid name b have
    shift
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
    for b in "$@"; do
        have=0
        while read -r pid name; do
            if [ -n "$pid" ] && [ "$name" = "branch:$b" ]; then have=1; fi
        done <<< "$policies"
        if [ "$have" = 1 ]; then info "$env: branch $b is allowed"
        else write POST "repos/$repo/environments/$env/deployment-branch-policies" "{\"name\":\"$b\",\"type\":\"branch\"}"; fi
    done
    while read -r pid name; do
        [ -n "$pid" ] || continue
        case " $* " in *" ${name#branch:} "*) [ "${name%%:*}" = branch ] && continue ;; esac
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
branch_policy production prod
for name in PUBLIC_HOST RETIRED_CONTENT_ORIGIN SITE_DOMAIN STORE; do
    [ -z "${!name}" ] || set_var "repos/$repo/environments/production/variables" "$name" "${!name}"
done
# The deploy reads these as secrets (step 6); a variable of the same name is
# an earlier setup's, and is removed.
for name in CLOUDFLARE_ACCOUNT_ID OAUTH_CLIENT_ID OWNER_GITHUB; do
    if get "repos/$repo/environments/production/variables/$name" >/dev/null; then
        info "$name is a variable; removed (it is a secret, step 6)"
        write DELETE "repos/$repo/environments/production/variables/$name"
    fi
done
[ -n "$RETIRED_CONTENT_ORIGIN" ] || info "RETIRED_CONTENT_ORIGIN: not given, so left as it is (pass --retired-content-origin when the instance's deploy record names one)"

# The drift job runs outside the environment, so it reads the repository's.
say "4. Repository variable PUBLIC_HOST (the drift check's host)"
set_var "repos/$repo/actions/variables" PUBLIC_HOST "$PUBLIC_HOST"

say "5. Environment npm"
branch_policy npm prod main

say "6. Ruleset release on refs/heads/release/*"
# Creation, update and deletion of a release branch are the bypass
# actors' alone: the repository admins (actor_id 5, RepositoryRole), as
# prod's ruleset.
release_ruleset='{"name":"release","target":"branch","enforcement":"active",'\
'"conditions":{"ref_name":{"include":["refs/heads/release/*"],"exclude":[]}},'\
'"bypass_actors":[{"actor_id":5,"actor_type":"RepositoryRole","bypass_mode":"always"}],'\
'"rules":[{"type":"creation"},{"type":"update","parameters":{"update_allows_fetch_and_merge":false}},{"type":"deletion"}]}'
id="$(get "repos/$repo/rulesets?includes_parents=false" | json 'next((r["id"] for r in j if r["name"] == "release"), None)')"
if [ -n "$id" ]; then
    info "exists (ID $id); set to the rules above"
    write PUT "repos/$repo/rulesets/$id" "$release_ruleset"
else
    write POST "repos/$repo/rulesets" "$release_ruleset"
fi

say "7. Environment production: the owner approves each deploy"
owner_id="$(get "users/$OWNER_GITHUB" | json 'j["id"]')" || die "GitHub has no user $OWNER_GITHUB"
have="$(get "repos/$repo/environments/production" \
    | json '[r["reviewer"]["id"] for p in j.get("protection_rules", []) if p["type"] == "required_reviewers" for r in p["reviewers"]]')" || have="[]"
if [ "$have" = "[$owner_id]" ]; then
    info "$OWNER_GITHUB (ID $owner_id) approves already"
else
    # The branch rule goes with it, so the write never leaves production
    # deployable from other branches.
    write PUT "repos/$repo/environments/production" \
        "{\"reviewers\":[{\"type\":\"User\",\"id\":$owner_id}],\"prevent_self_review\":false,\"deployment_branch_policy\":{\"protected_branches\":false,\"custom_branch_policies\":true}}"
fi

say "8. Environment staging"
branch_policy staging 'release/*' main
for name in STAGING_HOST_ZONE STAGING_SITE_ZONE; do
    set_var "repos/$repo/environments/staging/variables" "$name" "${!name}"
done

say "9. Environment release"
branch_policy release prod

say "10. Secrets (yours to run; this script never sets them)"
info "gh secret set CLOUDFLARE_ACCOUNT_ID --env production --repo $repo --body $CLOUDFLARE_ACCOUNT_ID"
if [ -n "$OAUTH_CLIENT_ID" ]; then
    info "gh secret set OAUTH_CLIENT_ID --env production --repo $repo --body $OAUTH_CLIENT_ID"
    info "gh secret set OWNER_GITHUB --env production --repo $repo --body $OWNER_GITHUB"
fi
info "gh secret set CLOUDFLARE_API_TOKEN --env production --repo $repo   # the token docs/prod.md describes"
info "gh secret set CLAX_SMOKE_TOKEN --env production --repo $repo       # an agent token named smoke, from https://$public_host/people (Agent tokens)"
info "gh secret set CLOUDFLARE_API_TOKEN --env staging --repo $repo      # prod's account, zones $STAGING_HOST_ZONE and $STAGING_SITE_ZONE (docs/prod.md, \"Staging\")"
info "gh secret set CLOUDFLARE_ACCOUNT_ID --env staging --repo $repo --body $CLOUDFLARE_ACCOUNT_ID"
info "gh secret set OAUTH_CLIENT_ID --env staging --repo $repo           # the staging GitHub OAuth App's client ID"
info "gh secret set OAUTH_CLIENT_SECRET --env staging --repo $repo       # its client secret"
info "gh secret set OWNER_GITHUB --env staging --repo $repo --body $OWNER_GITHUB"
info "gh secret set RELEASE_APP_ID --env release --repo $repo            # the release GitHub App's ID"
info "gh secret set RELEASE_APP_PRIVATE_KEY --env release --repo $repo < <the App's private key, .pem>"
[ "$act" = 1 ] && echo "setup-prod-github: done" || echo "setup-prod-github: dry run done; run again with --i-know-what-i-am-doing to act"
