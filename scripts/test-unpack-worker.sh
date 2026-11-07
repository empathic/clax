#!/usr/bin/env bash
# Checks scripts/unpack-worker.py as whoever wrote the pull request sees it: a
# Worker's build unpacks into a checkout, tracked placeholder and all, and a
# download or archive with anything else in it, however it is spelled, writes
# nothing at all.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
unpack="$PWD/scripts/unpack-worker.py"
# macOS's tar otherwise adds a `._` file beside each with extended attributes.
export COPYFILE_DISABLE=1
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

# A tree with a Worker's build in it, web/dist's tracked placeholder too.
mkdir -p "$d/b/target/clax-worker/build/worker" "$d/b/web/dist"
: >"$d/b/target/clax-worker/build/index_bg.wasm"
: >"$d/b/target/clax-worker/build/worker/shim.mjs"
echo '{}' >"$d/b/target/clax-worker/built-from.json"
echo '<html>' >"$d/b/web/dist/index.html"
: >"$d/b/web/dist/.gitkeep"
good=(target/clax-worker/build target/clax-worker/built-from.json web/dist)
tar -C "$d/b" -cf "$d/good.tar" "${good[@]}"

# _download NAME ARCHIVE: a directory as download-artifact leaves it.
_download() {
    mkdir "$d/dl-$1"
    cp "$2" "$d/dl-$1/worker.tar"
    echo "$d/dl-$1"
}
# _checkout NAME: a fresh checkout of the default branch, as far as this goes.
_checkout() {
    local c="$d/c-$1"
    mkdir -p "$c/web/dist" "$c/scripts"
    : >"$c/web/dist/.gitkeep"
    echo main >"$c/scripts/run.sh"
    echo "$c"
}

c="$(_checkout ok)"
mkdir -p "$c/target/clax-worker/build"
echo stale >"$c/target/clax-worker/build/stale.js"
out="$(python3 "$unpack" "$(_download ok "$d/good.tar")" "$c" 2>&1)" || _fail "a Worker's build was refused: $out"
[ -f "$c/target/clax-worker/build/worker/shim.mjs" ] || _fail "the build was not unpacked"
[ -f "$c/web/dist/index.html" ] || _fail "the web UI was not unpacked"
[ ! -e "$c/target/clax-worker/build/stale.js" ] || _fail "what stood there before was kept"
[ "$(cat "$c/scripts/run.sh")" = main ] || _fail "the checkout's scripts changed"
[ -x "$c/web/dist/index.html" ] && _fail "an unpacked file is executable"

# _unchanged C: C is as _checkout left it.
_unchanged() {
    [ "$(find "$1" -type f | wc -l | tr -d ' ')" = 2 ] && [ -f "$1/web/dist/.gitkeep" ]
}
# _refused NAME DOWNLOAD TEXT: refused, naming TEXT, the checkout left as it was.
_refused() {
    local out st c
    c="$(_checkout "$1")"
    out="$(python3 "$unpack" "$2" "$c" 2>&1)"
    st=$?
    [ "$st" != 0 ] || _fail "$1: unpacked"
    grep -qF "$3" <<<"$out" || _fail "$1: said $out"
    _unchanged "$c" || _fail "$1: changed the checkout"
}

# What a download may hold: worker.tar, a plain file, alone.
x="$(_download extra "$d/good.tar")"
mkdir -p "$x/clax-deploy"
echo '{"retired_content_origin":"https://evil.example"}' >"$x/clax-deploy/pr-1.example.json"
_refused extra-file "$x" "exactly one file"
y="$d/dl-link"
mkdir "$y"
ln -s "$d/good.tar" "$y/worker.tar"
_refused linked-download "$y" "exactly one file"
mkdir "$d/dl-empty"
_refused empty-download "$d/dl-empty" "exactly one file"

# An archive of the good tree with one member more, made by Python, which
# writes what tar would not: PATH, of KIND file, link, hardlink or sparse.
_with() {
    python3 - "$d/good.tar" "$1" "$2" "$3" <<'EOF'
import io, shutil, sys, tarfile
base, out, name, kind = sys.argv[1:5]
shutil.copy(base, out)
with tarfile.open(out, "a", format=tarfile.GNU_FORMAT) as t:
    i = tarfile.TarInfo(name)
    if kind == "file":
        i.size = 1
        t.addfile(i, io.BytesIO(b"x"))
    elif kind == "sparse":
        i.type = tarfile.GNUTYPE_SPARSE
        t.addfile(i)
    else:
        i.type = tarfile.SYMTYPE if kind == "link" else tarfile.LNKTYPE
        i.linkname = "../../../scripts"
        t.addfile(i)
EOF
}

_with "$d/a.tar" crates/clax-worker/deploy.mjs file
_refused deploy-script "$(_download a "$d/a.tar")" "no part of a Worker's build"
_with "$d/b.tar" crates/clax-worker/node_modules/.bin/wrangler file
_refused wrangler "$(_download b "$d/b.tar")" "no part of a Worker's build"
_with "$d/c.tar" web/dist/../../scripts/run.sh file
_refused dot-dot "$(_download c "$d/c.tar")" "leaves where it may go"
_with "$d/d.tar" /etc/passwd file
_refused absolute "$(_download d "$d/d.tar")" "leaves where it may go"
_with "$d/e.tar" web/dist/x link
_refused symlink "$(_download e "$d/e.tar")" "not a plain file"
_with "$d/f.tar" web/dist/y hardlink
_refused hard-link-member "$(_download f "$d/f.tar")" "not a plain file"
_with "$d/g.tar" web/dist/z sparse
_refused sparse "$(_download g "$d/g.tar")" "not a plain file"

# A path twice, or a path under a file: found before anything is cleared.
_with "$d/h.tar" web/dist/index.html file
_refused duplicate "$(_download h "$d/h.tar")" "more than once"
_with "$d/i.tar" web/dist/index.html/under file
_refused under-a-file "$(_download i "$d/i.tar")" "under a file"

# A build whose directory is a link, as a plain `tar -c` keeps Bazel's.
mkdir -p "$d/l/target/clax-worker" "$d/l/web/dist"
ln -s /nowhere "$d/l/target/clax-worker/build"
echo '{}' >"$d/l/target/clax-worker/built-from.json"
: >"$d/l/web/dist/index.html"
tar -C "$d/l" -cf "$d/link.tar" "${good[@]}"
_refused linked-build "$(_download l "$d/link.tar")" "not a plain file"

# The limits, lowered: a member too many, a byte too many.
_limited() {
    python3 - "$unpack" "$1" "$2" "$3" "$4" <<'EOF'
import importlib.machinery, importlib.util, sys
path, name, value, download, checkout = sys.argv[1:6]
loader = importlib.machinery.SourceFileLoader("unpack_worker", path)
spec = importlib.util.spec_from_loader("unpack_worker", loader)
m = importlib.util.module_from_spec(spec)
loader.exec_module(m)
setattr(m, name, int(value))
sys.exit(m.main(["unpack-worker.py", download, checkout]))
EOF
}
c="$(_checkout members)"
out="$(_limited MAX_MEMBERS 2 "$(_download members "$d/good.tar")" "$c" 2>&1)" && _fail "too many members passed"
grep -qF "too many" <<<"$out" || _fail "members: said $out"
_unchanged "$c" || _fail "members: changed the checkout"
c="$(_checkout bytes)"
out="$(_limited MAX_BYTES 2 "$(_download bytes "$d/good.tar")" "$c" 2>&1)" && _fail "too many bytes passed"
grep -qF "too big" <<<"$out" || _fail "bytes: said $out"
_unchanged "$c" || _fail "bytes: changed the checkout"

# What a Worker cannot go without.
rm "$d/b/target/clax-worker/build/index_bg.wasm"
tar -C "$d/b" -cf "$d/nowasm.tar" "${good[@]}"
_refused no-wasm "$(_download nowasm "$d/nowasm.tar")" "index_bg.wasm"

[ "$fails" = 0 ] && echo "unpack-worker: ok"
exit "$((fails > 0))"
