#!/usr/bin/env bash
# Sets up the GitHub side of the dev and preview environments
# (docs/environments.md): the GitHub environment `preview`, which
# .github/workflows/preview.yml, preview-reaper.yml and deploy-dev.yml run
# in, with its branch policy and variables. dev has no environment of its
# own: dev and the previews need the same Cloudflare account, token and
# OAuth App, and dev's hosts are named from the preview zones. Run it
# yourself, from any directory, with a gh signed in as a repository admin.
# It is safe to run again: it reads what exists first and changes only what
# differs from what it sets up. It never touches the `production` or `npm`
# environments, rulesets, branches or repository variables
# (scripts/setup-prod-github.sh sets those up).
#
# Without --i-know-what-i-am-doing it is a dry run: it makes the same reads
# and prints every write (`gh api --method ...` with its body) it would
# make, changing nothing.
#
#   scripts/setup-env-github.sh --env preview --preview-host-zone <zone> \
#       --preview-site-zone <zone> --github-client-id <ID> \
#       --owner-github <login> --cloudflare-account-id <ID>          # dry run
#
# It makes `preview` deployable from any branch (no branch policy: a pull
# request's run is on its own branch) and sets its variables
# PREVIEW_HOST_ZONE, PREVIEW_SITE_ZONE and OWNER_GITHUB, from flags alone.
# The two zones must differ, and neither may be under the other.
#
# OAUTH_CLIENT_ID and CLOUDFLARE_ACCOUNT_ID are the environment's secrets,
# not variables: the script prints their `gh secret set` commands, with
# the values given, beside the other secrets'.
#
# Options:
#   --env preview                   the environment (required)
#   --repo <owner/name>             empathic/clax
#   --preview-host-zone <zone>      previews at pr-<N>.<zone>, dev at dev.<zone>
#   --preview-site-zone <zone>      their sites at <ID>.pr-<N>.<zone>, <ID>.dev.<zone>
#   --github-client-id <ID>         the preview GitHub OAuth App
#   --owner-github <login>
#   --cloudflare-account-id <ID>
#
# In order, it
#
# 1. creates, or updates, the environment, deployable from any branch;
# 2. sets the environment's variables;
# 3. prints the `gh secret set` commands for the environment's secrets,
#    which it never runs: their values are yours to paste.
#
# The REST API shapes are GitHub's:
#   environments  https://docs.github.com/en/rest/deployments/environments#create-or-update-an-environment
#   variables     https://docs.github.com/en/rest/actions/variables#create-an-environment-variable
#                 https://docs.github.com/en/rest/actions/variables#update-an-environment-variable
# The variables' settings are read by name (${!name}), out of shellcheck's sight.
# shellcheck disable=SC2034
set -euo pipefail

act=0
repo="empathic/clax"
env_name=""
# The environment's settings, which the workflows read; step 2 reads the
# variables among them by name. The GitHub OAuth App's client ID is
# OAUTH_CLIENT_ID: GitHub refuses a secret whose name starts with GITHUB_.
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

case "$env_name" in
    preview) ;;
    dev) echo "setup-env-github: dev deploys in the preview environment (docs/environments.md): run --env preview" >&2; exit 2 ;;
    "") echo "setup-env-github: --env preview is required" >&2; exit 2 ;;
    *) echo "setup-env-github: --env $env_name is not preview" >&2; exit 2 ;;
esac
missing=""
[ -n "$PREVIEW_HOST_ZONE" ] || missing="$missing --preview-host-zone"
[ -n "$PREVIEW_SITE_ZONE" ] || missing="$missing --preview-site-zone"
vars="OWNER_GITHUB PREVIEW_HOST_ZONE PREVIEW_SITE_ZONE"
[ -n "$CLOUDFLARE_ACCOUNT_ID" ] || missing="$missing --cloudflare-account-id"
# GitHub sign-in on every environment: both its settings, always.
[ -n "$OAUTH_CLIENT_ID" ] || missing="$missing --github-client-id"
[ -n "$OWNER_GITHUB" ] || missing="$missing --owner-github"
if [ -n "$missing" ]; then
    echo "setup-env-github: not given:$missing" >&2
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
host_zone="$(printf '%s' "$PREVIEW_HOST_ZONE" | tr '[:upper:]' '[:lower:]')"
site_zone="$(printf '%s' "$PREVIEW_SITE_ZONE" | tr '[:upper:]' '[:lower:]')"
case "$host_zone" in "$site_zone"|*".$site_zone")
    echo "setup-env-github: the preview host zone $PREVIEW_HOST_ZONE is, or is under, the preview site zone $PREVIEW_SITE_ZONE: sites would share the previews' hosts" >&2; exit 2 ;;
esac
case "$site_zone" in *".$host_zone")
    echo "setup-env-github: the preview site zone $PREVIEW_SITE_ZONE is under the preview host zone $PREVIEW_HOST_ZONE: sites would share the previews' hosts" >&2; exit 2 ;;
esac

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

say "1. Environment preview, deployable from any branch"
no_branch_policy preview

say "2. Environment preview's variables"
for name in $vars; do
    set_var "repos/$repo/environments/preview/variables" "$name" "${!name}"
done

say "3. Secrets (yours to run; this script never sets them)"
info "gh secret set CLOUDFLARE_ACCOUNT_ID --env preview --repo $repo --body $CLOUDFLARE_ACCOUNT_ID"
info "gh secret set OAUTH_CLIENT_ID --env preview --repo $repo --body $OAUTH_CLIENT_ID"
info "gh secret set CLOUDFLARE_API_TOKEN --env preview --repo $repo   # the Cloudflare API token of dev and the previews (docs/environments.md, \"Credentials\")"
info "gh secret set OAUTH_CLIENT_SECRET --env preview --repo $repo    # the preview OAuth App's client secret; its callback URL is https://$PREVIEW_HOST_ZONE/auth/github/callback, with wildcard matching on"
info "gh secret set CLAX_DEV_SMOKE_TOKEN --env preview --repo $repo   # after dev's first deploy: an agent token named smoke, from https://dev.$PREVIEW_HOST_ZONE's settings"
[ "$act" = 1 ] && echo "setup-env-github: done" || echo "setup-env-github: dry run done; run again with --i-know-what-i-am-doing to act"
