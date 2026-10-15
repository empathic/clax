#!/usr/bin/env bash
# Sets up and deploys hosted Clax on your Cloudflare account. Run it yourself,
# from any directory; it is safe to run again at any time.
#
# Without --i-know-what-i-am-doing it is a dry run: it checks, installs and
# builds locally and ends at `wrangler deploy --dry-run`, changing nothing on
# the account.
#
#   scripts/cloudflare-setup.sh                            # dry run
#   scripts/cloudflare-setup.sh --i-know-what-i-am-doing   # deploy
#   scripts/cloudflare-setup.sh --smoke --i-know-what-i-am-doing  # deploy, then smoke it
#   scripts/cloudflare-setup.sh --login                    # only sign in to Cloudflare
#
# The first run asks, on a terminal, for the public hostname, the content
# hostname, where versions live, and (optionally) a GitHub OAuth App's
# Client ID and secret and your GitHub login, and keeps the answers in
# ~/.config/clax/deploy/setup.env (readable only by you; $CLAX_DEPLOY_HOME or
# $XDG_CONFIG_HOME/clax/deploy when set).
# Later runs need no flags. To change a setting, pass it once:
# --public <host>, --content <host>, --zone <domain> (public <domain>,
# content content.<domain>), --store artifacts|memory, --github-client-id
# <ID>, --owner-github <login>; or edit or delete setup.env.
#
# Every other option is passed to crates/clax-worker/deploy.mjs (see the top of
# that file), e.g. --store memory, --account <ID>, --rotate-secrets, --yes,
# --github-client-id <ID> --owner-github <login> (GitHub sign-in; the client
# secret from GITHUB_CLIENT_SECRET, else asked).
#
# You sign in yourself, in your browser. In order, it
#
# 1. checks the tools it needs (node 26, rustup with the wasm32 target, an
#    LLVM whose clang targets wasm32, git) and says how to get any that is
#    missing, adding only the rustup target itself, which is per-user;
# 2. installs the web UI's and the Worker's npm packages when their
#    lockfiles changed since the last install;
# 3. checks `wrangler whoami`: when you are not signed in and the script runs
#    on a terminal, it starts `wrangler login` for you to complete in the
#    browser (`--login` does only that); otherwise it stops and says so (a
#    dry run goes on without a login);
# 4. runs deploy.mjs, which creates only what is missing (the R2 bucket, the
#    Queue, the Artifacts namespace), keeps the secrets it made before,
#    builds, deploys and waits for /healthz;
# 5. with --smoke, runs smoke-remote.mjs against the deployment, and then
#    deploys again without its diagnostics routes (not in a dry run).

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
worker="$root/crates/clax-worker"

records="${CLAX_DEPLOY_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/clax/deploy}"
settings="$records/setup.env"
# Settings an earlier version kept under crates/ move here once.
[ -f "$worker/.deploy/setup.env" ] && [ ! -f "$settings" ] && { mkdir -p "$records" && chmod 700 "$records" && mv "$worker/.deploy/setup.env" "$settings"; }

smoke=0
dry=1
login_only=0
args=()
set_public="" set_content="" set_store="" set_gh_id="" set_owner=""
while [ $# -gt 0 ]; do
  case "$1" in
    --smoke) smoke=1 ;;
    --login) login_only=1 ;;
    --i-know-what-i-am-doing) dry=0 ;;
    --dry-run) ;;
    --public) set_public="$2"; shift ;;
    --content) set_content="$2"; shift ;;
    --store) set_store="$2"; shift ;;
    --github-client-id) set_gh_id="$2"; shift ;;
    --owner-github) set_owner="$2"; shift ;;
    --zone) set_public="$2"; set_content="content.$2"; shift ;;
    -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
    *) args+=("$1") ;;
  esac
  shift
done

say() { printf '\n== %s\n' "$*"; }
fail() { printf 'cloudflare-setup: %s\n' "$*" >&2; exit 1; }

# The settings this script keeps between runs, in a git-ignored file only you
# can read. A flag given on the command line replaces its saved value.
PUBLIC_HOST="" CONTENT_HOST="" STORE="" GITHUB_CLIENT_ID="" OWNER_GITHUB="" GITHUB_CLIENT_SECRET="${GITHUB_CLIENT_SECRET:-}"
if [ -f "$settings" ]; then
  while IFS='=' read -r k v; do
    case "$k" in
      PUBLIC_HOST|CONTENT_HOST|STORE|GITHUB_CLIENT_ID|OWNER_GITHUB) printf -v "$k" '%s' "$v" ;;
      GITHUB_CLIENT_SECRET) [ -n "$GITHUB_CLIENT_SECRET" ] || GITHUB_CLIENT_SECRET="$v" ;;
    esac
  done < "$settings"
fi
[ -n "$set_public" ] && PUBLIC_HOST="$set_public"
[ -n "$set_content" ] && CONTENT_HOST="$set_content"
[ -n "$set_store" ] && STORE="$set_store"
[ -n "$set_gh_id" ] && GITHUB_CLIENT_ID="$set_gh_id"
[ -n "$set_owner" ] && OWNER_GITHUB="$set_owner"

# Asks on a terminal for a value not saved yet; `$3` is the default.
ask() {
  local var="$1" question="$2" default="${3:-}" secret="${4:-}" answer
  [ -n "${!var}" ] && return 0
  [ -t 0 ] || { [ -n "$default" ] && printf -v "$var" '%s' "$default"; return 0; }
  if [ -n "$secret" ]; then
    read -r -s -p "$question: " answer; echo
  else
    read -r -p "$question${default:+ [$default]}: " answer
  fi
  printf -v "$var" '%s' "${answer:-$default}"
}
if [ "$login_only" = 0 ]; then
  ask PUBLIC_HOST "Public hostname (where people reach Clax)"
  [ -n "$PUBLIC_HOST" ] || fail "no public hostname: run on a terminal, or pass --public <host> once"
  ask CONTENT_HOST "Content hostname (a host of its own for artifact content)" "content.$PUBLIC_HOST"
  ask STORE "Where versions live: artifacts (Cloudflare Artifacts) or memory" "artifacts"
  ask GITHUB_CLIENT_ID "GitHub OAuth App Client ID (blank: no GitHub sign-in)"
  if [ -n "$GITHUB_CLIENT_ID" ]; then
    ask OWNER_GITHUB "Your GitHub login (that account signs in as the owner)"
    ask GITHUB_CLIENT_SECRET "GitHub OAuth App client secret" "" secret
  fi
  mkdir -p "$records" && chmod 700 "$records"
  ( umask 077
    { echo "PUBLIC_HOST=$PUBLIC_HOST"; echo "CONTENT_HOST=$CONTENT_HOST"; echo "STORE=$STORE"
      echo "GITHUB_CLIENT_ID=$GITHUB_CLIENT_ID"; echo "OWNER_GITHUB=$OWNER_GITHUB"
      echo "GITHUB_CLIENT_SECRET=$GITHUB_CLIENT_SECRET"; } > "$settings" )
  args+=(--public "$PUBLIC_HOST" --content "$CONTENT_HOST" --store "${STORE:-artifacts}")
  if [ -n "$GITHUB_CLIENT_ID" ]; then
    args+=(--github-client-id "$GITHUB_CLIENT_ID")
    [ -n "$OWNER_GITHUB" ] && args+=(--owner-github "$OWNER_GITHUB")
    export GITHUB_CLIENT_SECRET
  fi
  echo "Settings: https://$PUBLIC_HOST, content https://$CONTENT_HOST, store ${STORE:-artifacts}, GitHub sign-in ${GITHUB_CLIENT_ID:+on (owner @$OWNER_GITHUB)}${GITHUB_CLIENT_ID:-off} (saved in $settings)"
fi

[ "$dry" = 1 ] && [ "$login_only" = 0 ] && args+=("--dry-run")
[ "$dry" = 1 ] && [ "$login_only" = 0 ] && echo "Dry run: nothing changes on the Cloudflare account. Add --i-know-what-i-am-doing to deploy."

say "Checking tools"
command -v git >/dev/null || fail "git is missing"
command -v node >/dev/null || fail "node is missing: install Node 26 (https://nodejs.org)"
node_major="$(node -p 'process.versions.node.split(".")[0]')"
[ "$node_major" -ge 26 ] || fail "node $node_major found; Node 26 or later is needed"
command -v rustup >/dev/null || fail "rustup is missing: install it from https://rustup.rs"
(cd "$root" && rustup target list --installed) | grep -qx wasm32-unknown-unknown ||
  (cd "$root" && rustup target add wasm32-unknown-unknown)
# The same LLVM search scripts/check-wasm.sh and build.sh use.
llvm_ok=0
for d in "${LLVM_BIN:-}" "$HOME/.local/homebrew/opt/llvm/bin" /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin /usr/lib/llvm-*/bin; do
  [ -n "$d" ] && [ -x "$d/clang" ] && [ -x "$d/llvm-ar" ] &&
    "$d/clang" --print-targets 2>/dev/null | grep -q '^ *wasm32 ' && { llvm_ok=1; break; }
done
if [ "$llvm_ok" = 0 ] && command -v clang >/dev/null && clang --print-targets 2>/dev/null | grep -q '^ *wasm32 '; then llvm_ok=1; fi
[ "$llvm_ok" = 1 ] || fail "no clang that targets wasm32: install LLVM (macOS: brew install llvm; Debian/Ubuntu: apt install clang lld llvm), or set LLVM_BIN"
echo "ok"

# Installs a package directory's npm packages when its lockfile changed
# since the last install here.
npm_install() {
  local dir="$1" stamp="$1/node_modules/.cloudflare-setup-lock"
  if [ -d "$dir/node_modules" ] && [ -f "$stamp" ] && cmp -s "$dir/package-lock.json" "$stamp"; then
    echo "$dir: packages up to date"
  else
    (cd "$dir" && npm ci)
    cp "$dir/package-lock.json" "$stamp"
  fi
}

say "Installing packages"
npm_install "$root/web"
npm_install "$worker"

wrangler() { (cd "$worker" && ./node_modules/.bin/wrangler "$@"); }
signed_in() { ! wrangler whoami 2>&1 | grep -qi "not authenticated\|You are not logged in"; }

say "Checking the Cloudflare login"
if signed_in; then
  echo "signed in"
elif [ "$dry" = 1 ] && [ "$login_only" = 0 ]; then
  echo "not signed in; a dry run goes on without account calls"
elif [ -t 0 ] && [ -t 1 ]; then
  echo "not signed in: starting wrangler login; finish it in your browser"
  wrangler login
  signed_in || fail "still not signed in to Cloudflare"
else
  fail "not signed in to Cloudflare, and not on a terminal to sign in. Run: $0 --login"
fi
[ "$login_only" = 1 ] && { say "Done"; exit 0; }

say "Deploying"
(cd "$worker" && node deploy.mjs ${args[@]+"${args[@]}"})

if [ "$smoke" = 1 ] && [ "$dry" = 0 ]; then
  record="$records/$PUBLIC_HOST.json"
  [ -f "$record" ] || fail "no deploy record $record to smoke"
  say "Smoking the deployment (diagnostics routes on for the probe)"
  (cd "$worker" && node deploy.mjs ${args[@]+"${args[@]}"} --diag --no-build --yes)
  smoke_status=0
  (cd "$worker" && node smoke-remote.mjs --deploy "$record") || smoke_status=$?
  say "Deploying again without the diagnostics routes"
  (cd "$worker" && node deploy.mjs ${args[@]+"${args[@]}"} --no-diag --no-build --yes)
  [ "$smoke_status" = 0 ] || fail "the smoke failed (exit $smoke_status); see its output above"
fi

say "Done"
if [ "$dry" = 1 ]; then
  echo "That was a dry run. To deploy: $0 --i-know-what-i-am-doing"
else
  echo "Open the setup page deploy.mjs printed, or the sign-in link the smoke printed, to make your browser the owner."
fi
