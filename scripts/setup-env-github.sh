#!/usr/bin/env bash
# Sets up the GitHub side of the dev and preview environments
# (docs/environments.md): the GitHub environment `dev`, which
# .github/workflows/deploy-dev.yml deploys from, or `preview`, which
# .github/workflows/preview.yml and preview-reaper.yml run in, with its
# branch policy and variables. Run it yourself, from any directory, with a gh
# signed in as a repository admin. It is safe to run again: it reads what
# exists first and changes only what differs from what it sets up. It never
# touches the `production` or `npm` environments, rulesets, branches or
# repository variables (scripts/setup-prod-github.sh sets those up).
#
# Without --i-know-what-i-am-doing it is a dry run: it makes the same reads
# and prints every write (`gh api --method ...` with its body) it would
# make, changing nothing.
#
#   scripts/setup-env-github.sh --env dev                            # dry run
#   scripts/setup-env-github.sh --env dev --i-know-what-i-am-doing   # act
#   scripts/setup-env-github.sh --env preview --preview-host-zone <zone> \
#       --preview-site-zone <zone> --github-client-id <ID> \
#       --owner-github <login> --cloudflare-account-id <ID>          # dry run
#
# --env dev makes `dev` deployable from `main` only and sets its variables
# PUBLIC_HOST, SITE_DOMAIN, SITE_ZONE, STORE and OWNER_GITHUB. Each setting comes from its flag, else from the
# deploy configuration `scripts/cloudflare-setup.sh --env dev` keeps:
# setup.dev.env (PUBLIC_HOST, SITE_DOMAIN, SITE_ZONE, STORE,
# GITHUB_CLIENT_ID, OWNER_GITHUB; nothing else in it is read) and the deploy
# record <public host>.json (its `account`), in $CLAX_DEPLOY_HOME, else
# $XDG_CONFIG_HOME/clax/deploy, else ~/.config/clax/deploy. A setting found
# in neither fails the run, naming its flag, before any gh call; SITE_ZONE
# alone is optional (an existing variable is kept when none is given).
#
# --env preview makes `preview` deployable from any branch (no branch
# policy: a pull request's run is on its own branch) and sets its variables
# PREVIEW_HOST_ZONE, PREVIEW_SITE_ZONE and OWNER_GITHUB, from flags alone.
# The two zones must differ, and neither may be under the other.
#
# OAUTH_CLIENT_ID and CLOUDFLARE_ACCOUNT_ID are each environment's secrets,
# not variables: the script prints their `gh secret set` commands, with
# the values found, beside the other secrets'.
#
# Every environment has GitHub sign-in: --github-client-id and
# --owner-github (or their setup.dev.env lines, for dev) are required.
#
# Options:
#   --env <dev|preview>             the environment (required)
#   --repo <owner/name>             empathic/clax
#  dev:
#   --public-host <host>
#   --site-domain <domain>
#   --site-zone <zone>              optional
#   --store <artifacts|memory>      (artifacts when setup.dev.env names none)
#  preview:
#   --preview-host-zone <zone>      previews at pr-<N>.<zone>
#   --preview-site-zone <zone>      their sites at <ID>.pr-<N>.<zone>
#  both:
#   --github-client-id <ID>         the environment's GitHub OAuth App
#   --owner-github <login>
#   --cloudflare-account-id <ID>
#
# In order, it
#
# 1. creates, or updates, the environment and its branch policy: `dev`
#    deployable from `main` alone (any other allowed branch removed),
#    `preview` with none;
# 2. sets the environment's variables;
# 3. prints the `gh secret set` commands for the environment's secrets,
#    which it never runs: their values are yours to paste.
#
# The REST API shapes are GitHub's:
#   environments  https://docs.github.com/en/rest/deployments/environments#create-or-update-an-environment
#   branch policy https://docs.github.com/en/rest/deployments/branch-policies
#   variables     https://docs.github.com/en/rest/actions/variables#create-an-environment-variable
#                 https://docs.github.com/en/rest/actions/variables#update-an-environment-variable
# The variables' settings are read by name (${!name}), out of shellcheck's sight.
# shellcheck disable=SC2034
set -euo pipefail

act=0
repo="empathic/clax"
env_name=""
# The environments' settings, which the workflows read; step 2 reads the
# variables among them by name. The GitHub OAuth App's client ID is
# OAUTH_CLIENT_ID: GitHub refuses a secret whose name starts with GITHUB_.
PUBLIC_HOST=""
SITE_DOMAIN=""
SITE_ZONE=""
STORE=""
PREVIEW_HOST_ZONE=""
PREVIEW_SITE_ZONE=""
OWNER_GITHUB=""
OAUTH_CLIENT_ID=""
CLOUDFLARE_ACCOUNT_ID=""
usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d'; }
while [ $# -gt 0 ]; do
    case "$1" in
        --i-know-what-i-am-doing) act=1 ;;
        --dry-run) act=0 ;;
        --env) env_name="${2:-}"; shift ;;
        --repo) repo="$2"; shift ;;
        --public-host) PUBLIC_HOST="$2"; shift ;;
        --site-domain) SITE_DOMAIN="$2"; shift ;;
        --site-zone) SITE_ZONE="$2"; shift ;;
        --store) STORE="$2"; shift ;;
        --preview-host-zone) PREVIEW_HOST_ZONE="$2"; shift ;;
        --preview-site-zone) PREVIEW_SITE_ZONE="$2"; shift ;;
        --owner-github) OWNER_GITHUB="$2"; shift ;;
        --github-client-id) OAUTH_CLIENT_ID="$2"; shift ;;
        --cloudflare-account-id) CLOUDFLARE_ACCOUNT_ID="$2"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "setup-env-github: unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

deploy_home="${CLAX_DEPLOY_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/clax/deploy}"
missing=""
case "$env_name" in
    dev)
        # The deploy configuration's settings, each only where no flag gave it.
        # setting FILE KEY: the value of KEY=… in the file, or nothing. Reads
        # that one line, so the file's GITHUB_CLIENT_SECRET is never read
        # into the run.
        setting() {
            [ -f "$1" ] || return 0
            sed -n "s/^$2=//p" "$1" | tail -n 1
        }
        env_file="$deploy_home/setup.dev.env"
        [ -n "$PUBLIC_HOST" ] || PUBLIC_HOST="$(setting "$env_file" PUBLIC_HOST)"
        [ -n "$SITE_DOMAIN" ] || SITE_DOMAIN="$(setting "$env_file" SITE_DOMAIN)"
        [ -n "$SITE_ZONE" ] || SITE_ZONE="$(setting "$env_file" SITE_ZONE)"
        [ -n "$STORE" ] || STORE="$(setting "$env_file" STORE)"
        [ -n "$STORE" ] || STORE="artifacts"
        [ -n "$OWNER_GITHUB" ] || OWNER_GITHUB="$(setting "$env_file" OWNER_GITHUB)"
        [ -n "$OAUTH_CLIENT_ID" ] || OAUTH_CLIENT_ID="$(setting "$env_file" GITHUB_CLIENT_ID)"
        # The deploy record's `account`, or nothing; the record's other
        # fields (its secrets among them) are not printed.
        if [ -z "$CLOUDFLARE_ACCOUNT_ID" ] && [ -n "$PUBLIC_HOST" ] && [ -f "$deploy_home/$PUBLIC_HOST.json" ]; then
            CLOUDFLARE_ACCOUNT_ID="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get("account") or "")' "$deploy_home/$PUBLIC_HOST.json")"
        fi
        [ -n "$PUBLIC_HOST" ] || missing="$missing --public-host"
        [ -n "$SITE_DOMAIN" ] || missing="$missing --site-domain"
        vars="OWNER_GITHUB PUBLIC_HOST SITE_DOMAIN SITE_ZONE STORE"
        where="not in $deploy_home/setup.dev.env and not given"
        ;;
    preview)
        [ -n "$PREVIEW_HOST_ZONE" ] || missing="$missing --preview-host-zone"
        [ -n "$PREVIEW_SITE_ZONE" ] || missing="$missing --preview-site-zone"
        vars="OWNER_GITHUB PREVIEW_HOST_ZONE PREVIEW_SITE_ZONE"
        where="not given"
        ;;
    "") echo "setup-env-github: --env dev or --env preview is required" >&2; exit 2 ;;
    *) echo "setup-env-github: --env $env_name is not dev or preview" >&2; exit 2 ;;
esac
[ -n "$CLOUDFLARE_ACCOUNT_ID" ] || missing="$missing --cloudflare-account-id"
# GitHub sign-in on every environment: both its settings, always.
[ -n "$OAUTH_CLIENT_ID" ] || missing="$missing --github-client-id"
[ -n "$OWNER_GITHUB" ] || missing="$missing --owner-github"
if [ -n "$missing" ]; then
    echo "setup-env-github: $where:$missing" >&2
    exit 2
fi
# The values go into JSON bodies and printed commands as they are:
# hostnames, IDs and logins only, so none can carry a quote or a backslash
# into one.
for name in $vars CLOUDFLARE_ACCOUNT_ID OAUTH_CLIENT_ID; do
    case "${!name}" in
        *[!A-Za-z0-9._-]*) echo "setup-env-github: $name '${!name}' is not a hostname, an ID or a login" >&2; exit 2 ;;
    esac
done
if [ "$env_name" = preview ]; then
    host_zone="$(printf '%s' "$PREVIEW_HOST_ZONE" | tr '[:upper:]' '[:lower:]')"
    site_zone="$(printf '%s' "$PREVIEW_SITE_ZONE" | tr '[:upper:]' '[:lower:]')"
    case "$host_zone" in "$site_zone"|*".$site_zone")
        echo "setup-env-github: the preview host zone $PREVIEW_HOST_ZONE is, or is under, the preview site zone $PREVIEW_SITE_ZONE: sites would share the previews' hosts" >&2; exit 2 ;;
    esac
    case "$site_zone" in *".$host_zone")
        echo "setup-env-github: the preview site zone $PREVIEW_SITE_ZONE is under the preview host zone $PREVIEW_HOST_ZONE: sites would share the previews' hosts" >&2; exit 2 ;;
    esac
fi

say() { printf '\n== %s\n' "$*"; }
info() { printf '   %s\n' "$*"; }
die() { printf 'setup-env-github: %s\n' "$*" >&2; exit 1; }
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

[ "$act" = 1 ] && echo "setup-env-github: acting on $repo, environment $env_name" || echo "setup-env-github: dry run on $repo, environment $env_name (nothing changes; --i-know-what-i-am-doing acts)"

# branch_policy ENV BRANCH: makes ENV deployable from BRANCH alone.
branch_policy() {
    local env="$1" branch="$2" e custom policies keep=0 pid name
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
        if [ "$name" = "branch:$branch" ]; then keep=1; info "$env: branch $branch is allowed"; fi
    done <<< "$policies"
    [ "$keep" = 1 ] || write POST "repos/$repo/environments/$env/deployment-branch-policies" "{\"name\":\"$branch\",\"type\":\"branch\"}"
    while read -r pid name; do
        [ -n "$pid" ] && [ "$name" != "branch:$branch" ] || continue
        info "$env: $name is allowed; removed"
        write DELETE "repos/$repo/environments/$env/deployment-branch-policies/$pid"
    done <<< "$policies"
}

# no_branch_policy ENV: makes ENV deployable from any branch.
no_branch_policy() {
    local env="$1" e policy
    if e="$(get "repos/$repo/environments/$env")"; then
        policy="$(json 'j.get("deployment_branch_policy")' <<< "$e")"
        if [ -z "$policy" ]; then info "$env deploys from any branch"; return 0; fi
        info "$env deploys from selected or protected branches; set to any branch"
    else
        info "$env does not exist"
    fi
    write PUT "repos/$repo/environments/$env" '{"deployment_branch_policy":null}'
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

if [ "$env_name" = dev ]; then
    say "1. Environment dev, deployable from main only"
    branch_policy dev main
else
    say "1. Environment preview, deployable from any branch"
    no_branch_policy preview
fi

say "2. Environment $env_name's variables"
for name in $vars; do
    [ -z "${!name}" ] || set_var "repos/$repo/environments/$env_name/variables" "$name" "${!name}"
done
if [ "$env_name" = dev ] && [ -z "$SITE_ZONE" ]; then
    info "SITE_ZONE: not given, so left as it is (pass --site-zone when dev's site domain is under a zone of another name)"
fi

say "3. Secrets (yours to run; this script never sets them)"
info "gh secret set CLOUDFLARE_ACCOUNT_ID --env $env_name --repo $repo --body $CLOUDFLARE_ACCOUNT_ID"
info "gh secret set OAUTH_CLIENT_ID --env $env_name --repo $repo --body $OAUTH_CLIENT_ID"
if [ "$env_name" = dev ]; then
    info "gh secret set CLOUDFLARE_API_TOKEN --env dev --repo $repo   # dev's Cloudflare API token (docs/environments.md, \"Credentials\")"
    info "gh secret set OAUTH_CLIENT_SECRET --env dev --repo $repo    # dev's GitHub OAuth App's client secret"
    info "gh secret set CLAX_SMOKE_TOKEN --env dev --repo $repo       # an agent token named smoke, from https://$PUBLIC_HOST's settings"
else
    info "gh secret set CLOUDFLARE_API_TOKEN --env preview --repo $repo   # the previews' Cloudflare API token (docs/environments.md, \"Credentials\")"
    info "gh secret set OAUTH_CLIENT_SECRET --env preview --repo $repo    # the preview OAuth App's client secret; its callback URL is https://$PREVIEW_HOST_ZONE/auth/github/callback, with wildcard matching on"
fi
[ "$act" = 1 ] && echo "setup-env-github: done" || echo "setup-env-github: dry run done; run again with --i-know-what-i-am-doing to act"
