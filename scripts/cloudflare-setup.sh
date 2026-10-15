#!/usr/bin/env bash
# Sets up and deploys hosted Clax on your Cloudflare account. Run it yourself,
# from the repository root; it is safe to run again at any time.
#
#   scripts/cloudflare-setup.sh --zone example.com
#   scripts/cloudflare-setup.sh --public clax.example.com --content clax-content.example.com
#   scripts/cloudflare-setup.sh --zone example.com --dry-run   # changes nothing on the account
#   scripts/cloudflare-setup.sh --zone example.com --smoke     # deploy, then smoke the deployment
#
# Every other option is passed to crates/clax-worker/deploy.mjs (see the top of
# that file), e.g. --store memory, --account <ID>, --rotate-secrets, --yes.
#
# It never signs in for you. In order, it
#
# 1. checks the tools it needs (node 26, rustup with the wasm32 target, an
#    LLVM whose clang targets wasm32, git) and says how to get any that is
#    missing, adding only the rustup target itself, which is per-user;
# 2. installs the web UI's and the Worker's npm packages when their
#    lockfiles changed since the last install;
# 3. checks `wrangler whoami`: when you are not signed in it stops and tells
#    you to run `npx wrangler login` in crates/clax-worker yourself (a
#    --dry-run goes on without a login);
# 4. runs deploy.mjs, which creates only what is missing (the R2 bucket, the
#    Queue, the Artifacts namespace), keeps the secrets it made before,
#    builds, deploys and waits for /healthz;
# 5. with --smoke, runs smoke-remote.mjs against the deployment, and then
#    deploys again without its diagnostics routes.

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
worker="$root/crates/clax-worker"

smoke=0
dry=0
args=()
for a in "$@"; do
  case "$a" in
    --smoke) smoke=1 ;;
    --dry-run) dry=1; args+=("$a") ;;
    -h|--help) sed -n '2,27p' "$0"; exit 0 ;;
    *) args+=("$a") ;;
  esac
done

say() { printf '\n== %s\n' "$*"; }
fail() { printf 'cloudflare-setup: %s\n' "$*" >&2; exit 1; }

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

say "Checking the Cloudflare login"
if (cd "$worker" && ./node_modules/.bin/wrangler whoami 2>&1) | grep -qi "not authenticated\|You are not logged in"; then
  if [ "$dry" = 1 ]; then
    echo "not signed in; a dry run goes on without account calls"
  else
    fail "not signed in to Cloudflare. Run this yourself, then run this script again:
    cd crates/clax-worker && npx wrangler login"
  fi
else
  echo "signed in"
fi

say "Deploying"
(cd "$worker" && node deploy.mjs ${args[@]+"${args[@]}"})

if [ "$smoke" = 1 ] && [ "$dry" = 0 ]; then
  record="$(ls -t "$worker"/.deploy/*.json 2>/dev/null | head -1)"
  [ -n "$record" ] || fail "no deploy record in crates/clax-worker/.deploy to smoke"
  say "Smoking the deployment (diagnostics routes on for the probe)"
  (cd "$worker" && node deploy.mjs ${args[@]+"${args[@]}"} --diag --no-build --yes)
  smoke_status=0
  (cd "$worker" && node smoke-remote.mjs --deploy "$record") || smoke_status=$?
  say "Deploying again without the diagnostics routes"
  (cd "$worker" && node deploy.mjs ${args[@]+"${args[@]}"} --no-diag --no-build --yes)
  [ "$smoke_status" = 0 ] || fail "the smoke failed (exit $smoke_status); see its output above"
fi

say "Done"
echo "Open the setup page deploy.mjs printed, or the sign-in link the smoke printed, to make your browser the owner."
