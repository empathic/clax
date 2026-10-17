#!/usr/bin/env bash
# Prints a one-use link, good for ten minutes, that signs your browser in as
# the owner of a deployed hosted Clax. It uses the owner token deploy.mjs kept
# in ~/.config/clax/deploy/<public host>.json. Run it from any directory:
#
#   scripts/clax-owner-signin.sh              # the newest deploy record
#   scripts/clax-owner-signin.sh clax.dev     # that host's record
set -euo pipefail
records="${CLAX_DEPLOY_HOME:-${XDG_CONFIG_HOME:-$HOME/.config}/clax/deploy}"
if [ $# -gt 0 ]; then record="$records/$1.json"; else record="$(ls -t "$records"/*.json 2>/dev/null | sed -n 1p)"; fi
[ -n "${record:-}" ] && [ -f "$record" ] || { echo "clax-owner-signin: no deploy record in $records" >&2; exit 1; }
node --input-type=module - "$record" <<'JS'
import { readFileSync } from "node:fs";
const r = JSON.parse(readFileSync(process.argv[2], "utf8"));
const res = await fetch(`${r.public_origin}/api/hosted/owner-signin-link`, {
  method: "POST",
  headers: { authorization: `Bearer ${r.owner_token}`, "content-type": "application/json" },
  body: "{}",
});
const body = await res.text();
if (!res.ok) { console.error(`clax-owner-signin: ${res.status} ${body.slice(0, 300)}`); process.exit(1); }
const { url, expires_at } = JSON.parse(body);
console.log(`Open this in your browser to sign in as the owner (once, until ${expires_at}):\n  ${url}`);
JS
