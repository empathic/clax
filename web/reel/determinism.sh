#!/bin/sh
# Records a vignette in deterministic mode twice and fails unless the two
# recordings are the same: as many frames, every frame byte for byte
# (sha256), and the same beats. On a difference it names the first frame
# that differs, for a person to look at the pair.
#
#   sh reel/determinism.sh [<vignette>]     (from web/; realtime by default)
#
# //web/reel:<vignette>_determinism runs it (pnpm run reel:determinism) with
# what the recorder reads, as web/reel/reel.bzl gives a recording.
set -eu

vignette=${1:-realtime}
tmp=$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/clax-reel-determinism.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

for run in a b; do
  node reel/record.ts --vignette "$vignette" --mode deterministic --out "$tmp/$run"
done

# "<frame> <sha256>" per frame, in order.
sums() {
  (cd "$tmp/$1/frames" && shasum -a 256 -- *.png) | awk '{ print $2, $1 }'
}
sums a > "$tmp/a.sums"
sums b > "$tmp/b.sums"

na=$(wc -l < "$tmp/a.sums" | tr -d ' ')
nb=$(wc -l < "$tmp/b.sums" | tr -d ' ')
if [ "$na" != "$nb" ]; then
  echo "error: $vignette: one recording has $na frames, the other $nb" >&2
  exit 1
fi
# The same frame names in both, in order: a gap in one is named, not diffed.
cut -d' ' -f1 "$tmp/a.sums" > "$tmp/a.names"
cut -d' ' -f1 "$tmp/b.sums" > "$tmp/b.names"
if ! cmp -s "$tmp/a.names" "$tmp/b.names"; then
  missing=$(diff "$tmp/a.names" "$tmp/b.names" | sed -n 's/^[<>] //p' | head -n 1)
  echo "error: $vignette: frame $missing is in one recording only" >&2
  exit 1
fi
paste -d' ' "$tmp/a.sums" "$tmp/b.sums" | awk '$2 != $4 { print $1 }' > "$tmp/differ"
first=$(head -n 1 "$tmp/differ")
if [ -n "$first" ]; then
  echo "error: $vignette: frame $first differs between two recordings ($na frames; $(wc -l < "$tmp/differ" | tr -d ' ') differ: $(head -n 8 "$tmp/differ" | tr '\n' ' '))" >&2
  # Under Bazel the pair is kept, in the test's outputs (bazel-testlogs/.../test.outputs).
  if [ -n "${TEST_UNDECLARED_OUTPUTS_DIR:-}" ]; then
    cp "$tmp/a/frames/$first" "$TEST_UNDECLARED_OUTPUTS_DIR/a-$first"
    cp "$tmp/b/frames/$first" "$TEST_UNDECLARED_OUTPUTS_DIR/b-$first"
    cp "$tmp/differ" "$TEST_UNDECLARED_OUTPUTS_DIR/differ.txt"
    echo "the two frames $first are in $TEST_UNDECLARED_OUTPUTS_DIR" >&2
  fi
  exit 1
fi
if ! cmp -s "$tmp/a/beats.json" "$tmp/b/beats.json"; then
  echo "error: $vignette: the beats differ between two recordings" >&2
  diff "$tmp/a/beats.json" "$tmp/b/beats.json" >&2 || true
  exit 1
fi
echo "$vignette: two deterministic recordings, $na frames each, identical"
