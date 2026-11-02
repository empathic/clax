#!/usr/bin/env bash
# Deploys the clax-fleet image IMAGE to a kind cluster of its own, the way
# docs/kubernetes.md tells a customer to (the release's clax-fleet.yaml from
# scripts/render-fleet-manifests.sh, an overlay that generates the catalog,
# the clax-secrets Secret), and checks that it serves: two shards come up
# ready, each instance answers on its own shard and through the other one
# (a forward between pods), and each shard runs only its own instance.
# .github/workflows/fleet-image.yml runs it on every image it publishes.
#
# Usage: fleet-kind-smoke.sh IMAGE
#   IMAGE is a tagged image in the local Docker (not :latest), which kind
#   loads into the cluster: build one with
#   docker build -f deploy/Dockerfile -t clax-fleet:smoke .
# Needs docker, kind, kubectl, curl and openssl. The cluster is deleted on
# exit; CLAX_SMOKE_KEEP=1 keeps it for a look.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
[ $# = 1 ] || { echo "usage: fleet-kind-smoke.sh IMAGE" >&2; exit 2; }
image="$1"
cluster="clax-smoke-$$"
ctx="kind-$cluster"
ns=clax-smoke
work="$(mktemp -d)"
pids=()

say() { echo "fleet-kind-smoke: $*" >&2; }
k() { kubectl --context "$ctx" --namespace "$ns" "$@"; }
cleanup() {
    rc=$?
    for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; wait "$p" 2>/dev/null || true; done
    if [ "$rc" != 0 ] && kind get clusters 2>/dev/null | grep -x "$cluster" >/dev/null; then
        say "failed; the cluster's state:"
        k get pods,svc,endpoints -o wide >&2 || true
        k describe pods >&2 || true
        for pod in clax-0 clax-1; do
            say "logs of $pod:"
            k logs "$pod" --tail=80 >&2 || true
        done
    fi
    if [ "${CLAX_SMOKE_KEEP:-}" = 1 ]; then
        say "kept the cluster: kubectl --context $ctx -n $ns get pods; kind delete cluster --name $cluster"
    else
        kind delete cluster --name "$cluster" >/dev/null 2>&1 || true
    fi
    rm -rf "$work"
    exit "$rc"
}
trap cleanup EXIT

# Waits up to $1 seconds for the command after it to succeed.
until_ok() {
    local deadline=$((SECONDS + $1)); shift
    until "$@"; do
        [ "$SECONDS" -lt "$deadline" ] || return 1
        sleep 1
    done
}

docker image inspect "$image" >/dev/null || { say "$image is not in the local Docker"; exit 2; }

# What a customer writes: the release's manifests, their catalog in an
# overlay, and the Secret.
"$here/scripts/render-fleet-manifests.sh" "$image" "$work/clax-fleet.yaml"
cat > "$work/fleet.toml" <<'EOF'
shards = 2

[[instance]]
name = "alpha"
shard = 0
create = true
config = '''
[hosted]
public_origin = "https://alpha.clax.test"
site_domain = "alpha-sites.test"
setup_secret_env = "CLAX_SETUP_SECRET_ALPHA"
'''

[[instance]]
name = "beta"
shard = 1
create = true
config = '''
[hosted]
public_origin = "https://beta.clax.test"
site_domain = "beta-sites.test"
setup_secret_env = "CLAX_SETUP_SECRET_BETA"
'''
EOF
cat > "$work/kustomization.yaml" <<EOF
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
namespace: $ns
resources:
  - clax-fleet.yaml
configMapGenerator:
  - name: clax-fleet-catalog
    files:
      - fleet.toml
    options:
      disableNameSuffixHash: true
EOF

say "creating kind cluster $cluster"
kind create cluster --name "$cluster" --wait 120s >&2
kind load docker-image "$image" --name "$cluster" >&2
kubectl --context "$ctx" create namespace "$ns" >/dev/null
k create secret generic clax-secrets \
    --from-literal=CLAX_FLEET_PEER_KEY="$(openssl rand -hex 32)" \
    --from-literal=CLAX_SETUP_SECRET_ALPHA="$(openssl rand -hex 16)" \
    --from-literal=CLAX_SETUP_SECRET_BETA="$(openssl rand -hex 16)" >/dev/null
kubectl --context "$ctx" apply -k "$work" >&2
say "waiting for both shards"
k rollout status statefulset/clax --timeout=300s >&2

# Forwards a local port to pod $1's port 7480, sets $2 to it, and adds the
# port-forward's PID to pids.
port="" p0="" p1=""
port_in() { port="$(sed -n '/^Forwarding from 127\.0\.0\.1:/{s/^Forwarding from 127\.0\.0\.1:\([0-9]*\) .*/\1/p;q;}' "$1")"; [ -n "$port" ]; }
forward() {
    local log="$work/forward-$1.log"
    k port-forward "pod/$1" :7480 > "$log" 2>&1 &
    pids+=("$!")
    until_ok 30 port_in "$log" || { say "port-forward to $1 did not start:"; cat "$log" >&2; return 1; }
    printf -v "$2" '%s' "$port"
}
forward clax-0 p0
forward clax-1 p1

# Each instance answers by Host on its own shard and, forwarded, on the other.
check() {
    local pod="$1" port="$2" host="$3" code
    code="$(curl -sS -o "$work/body" -w '%{http_code}' --max-time 60 -H "Host: $host" "http://127.0.0.1:$port/healthz")" || code=000
    if [ "$code" != 200 ]; then
        say "$host through $pod answered $code: $(head -c 300 "$work/body" 2>/dev/null)"
        return 1
    fi
    say "$host through $pod: 200"
}
check clax-0 "$p0" alpha.clax.test
check clax-1 "$p1" beta.clax.test
check clax-1 "$p1" alpha.clax.test
check clax-0 "$p0" beta.clax.test

# Each shard runs its own instance, and only it.
status="$(k exec clax-0 -- clax-fleet status)"
echo "$status" >&2
if ! grep -qF '(clax-0): ready true, running ["alpha"]' <<<"$status" \
    || ! grep -qF '(clax-1): ready true, running ["beta"]' <<<"$status"; then
    say "the shards do not run alpha on clax-0 and beta on clax-1"
    exit 1
fi
say "passed"
