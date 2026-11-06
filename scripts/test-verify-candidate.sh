#!/usr/bin/env bash
# Tests scripts/verify-candidate.sh with a fake gh on PATH; the real gh is
# never run. The fake's `gh attestation verify <file>` passes when
# $FAKE_STATE/attested has the line "<file's base name> <signer workflow>
# <source digest>" for the flags it was given, as GitHub's attestation for
# that file would, and fails otherwise.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
# shellcheck source=scripts/fake-exe.sh
. "$HERE/scripts/fake-exe.sh"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
SCRIPT="$HERE/scripts/verify-candidate.sh"
HEAD_SHA=0123456789abcdef0123456789abcdef01234567
OTHER=89abcdef0123456789abcdef0123456789abcdef
R=empathic/clax

mkdir -p "$T/bin" "$T/state"
fake_exe "$T/bin/gh" <<'PY'
#!/usr/bin/env python3
import os, sys
state = os.environ["FAKE_STATE"]
args = sys.argv[1:]
if args[:2] != ["attestation", "verify"]:
    sys.exit("fake gh: unexpected " + " ".join(args))
f = args[2]
flag = lambda k: args[args.index(k) + 1]
if flag("--repo") != "empathic/clax":
    sys.exit("fake gh: --repo is not empathic/clax")
line = f"{os.path.basename(f)} {flag('--signer-workflow')} {flag('--source-digest')}"
attested = open(os.path.join(state, "attested")).read().splitlines()
if line not in attested:
    sys.exit(f"Error: verifying with issuer \"sigstore.dev\": no attestation matches {line}")
print("Verification succeeded!")
PY
export PATH="$T/bin:$PATH" FAKE_STATE="$T/state" GH_REPO="$R"

# A candidate as staging.yml uploads it, every file attested by its workflow.
C="$T/candidate"
mkdir -p "$C/release-dist" "$C/worker"
for f in clax-0.7.0-aarch64-apple-darwin.tar.gz SHA256SUMS install.sh SOURCE_TREE; do
    echo "$f" > "$C/release-dist/$f"
    echo "$f $R/.github/workflows/release.yml $HEAD_SHA" >> "$T/state/attested"
done
echo worker > "$C/worker/worker.tar"
echo "worker.tar $R/.github/workflows/staging.yml $HEAD_SHA" >> "$T/state/attested"
cp "$T/state/attested" "$T/attested.all"

# `expect <case> <want exit> <output pattern> [<head>]`
expect() {
    local out rc
    out="$("$SCRIPT" "${4:-$HEAD_SHA}" "$C" 2>&1)"; rc=$?
    if [ "$rc" = "$2" ] && grep -qE -- "$3" <<< "$out"; then pass "$1"
    else fail "$1 (rc=$rc: $out)"; fi
}

expect "a candidate whose every file is attested passes" 0 "5 files verified"
expect "it names each file it verified and the workflow" 0 "verified worker/worker.tar \(staging.yml at 0123456789ab\)"

expect "a candidate built from another commit is refused" 1 \
    "release-dist/SHA256SUMS is not attested as release.yml's at $OTHER" "$OTHER"

grep -v '^SHA256SUMS ' "$T/attested.all" > "$T/state/attested"
expect "a file with no attestation is refused, by name" 1 \
    "release-dist/SHA256SUMS is not attested as release.yml's"

{ grep -v '^worker.tar ' "$T/attested.all"; echo "worker.tar $R/.github/workflows/release.yml $HEAD_SHA"; } > "$T/state/attested"
expect "a Worker attested by another workflow is refused" 1 \
    "worker/worker.tar is not attested as staging.yml's"
cp "$T/attested.all" "$T/state/attested"

echo extra > "$C/worker/extra.js"
echo "extra.js $R/.github/workflows/staging.yml $HEAD_SHA" >> "$T/state/attested"
expect "a file no candidate has is refused, even attested" 1 "worker/extra.js is no file of a candidate"
rm "$C/worker/extra.js"
cp "$T/attested.all" "$T/state/attested"

ln -s ../worker/worker.tar "$C/release-dist/link"
expect "a symbolic link is refused" 1 "release-dist/link is a symbolic link"
rm "$C/release-dist/link"

mv "$C/worker/worker.tar" "$T/worker.tar"
expect "a candidate without the Worker is refused" 1 "no $C/worker/worker.tar"
mv "$T/worker.tar" "$C/worker/worker.tar"

expect "a head that is not a full commit ID is a usage error" 2 "is not a full commit ID" 0123456

exit "$FAILED"
