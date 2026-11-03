#!/usr/bin/env bash
# Renders deploy/k8s/release with the clax-fleet image IMAGE: the file each
# release attaches as clax-fleet.yaml (.github/workflows/fleet-image.yml),
# and what scripts/fleet-kind-smoke.sh applies. IMAGE names a tag or a
# digest:
#
#   ghcr.io/empathic/clax-fleet@sha256:<64 hex digits>   a release, pinned
#   clax-fleet:smoke                                     a local build
#
# Usage: render-fleet-manifests.sh IMAGE [OUT]   (standard output without OUT)
# Needs kubectl, whose kustomize builds the overlay.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
die() { echo "render-fleet-manifests: $*" >&2; exit 2; }
[ $# -ge 1 ] && [ $# -le 2 ] || die "usage: render-fleet-manifests.sh IMAGE [OUT]"
image="$1"

case "$image" in
    *@*)
        [[ "${image#*@}" =~ ^sha256:[0-9a-f]{64}$ ]] || die "$image: a digest is sha256: and 64 hex digits"
        ;;
    *)
        # The tag follows the last colon, unless a slash does (a registry's port).
        tag="${image##*:}"
        { [ "$tag" != "$image" ] && [[ "$tag" != */* ]] && [ -n "$tag" ]; } || die "$image: name a tag or a digest"
        [ "$tag" != latest ] || die "$image: name a version, not latest (kubelet would pull it again)"
        ;;
esac

out="$(mktemp)"
trap 'rm -f "$out"' EXIT
version="$("$here/scripts/check-version.sh" --print)"
# The overlay runs ghcr.io/empathic/clax-fleet:<version> in one container;
# that one line becomes IMAGE. (A kustomization of our own cannot name the
# overlay by absolute path, which kustomize refuses.)
{
    echo "# Clax fleet $version, running $image."
    echo "# Apply it from an overlay that generates the clax-fleet-catalog ConfigMap,"
    echo "# with the clax-secrets Secret created (docs/kubernetes.md, Deploying)."
    kubectl kustomize "$here/deploy/k8s/release" | IMAGE="$image" awk '
        /^ +image: ghcr\.io\/empathic\/clax-fleet:/ { sub(/image: .*/, "image: " ENVIRON["IMAGE"]); n++ }
        { print }
        END { if (n != 1) { print "render-fleet-manifests: expected 1 clax-fleet image, found " n > "/dev/stderr"; exit 1 } }'
} > "$out"
if [ $# = 2 ]; then cp "$out" "$2"; else cat "$out"; fi
