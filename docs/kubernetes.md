# Clax on Kubernetes

`clax-fleet` serves hosted Clax instances from a StatefulSet's pods, each
instance's data on its pod's volume, and scales by adding pods.
`deploy/k8s` holds the manifests (kustomize), `deploy/Dockerfile` builds
the image (published by `.github/workflows/fleet-image.yml` for each
release), and `crates/clax-fleet` is the server; its crate docs cover the
design in depth.

## How it scales

A Clax instance has one writer. Its store's write connection, its realtime
state (event streams, presence, seats, long polls) and its background tasks
all live in one process, the way a Durable Object holds an instance in the
hosted Worker. The fleet keeps that rule: **each instance runs in exactly
one pod, and the fleet scales by spreading instances across pods.** Any
pod takes any request. A pod that runs the request's instance answers it
in process; any other pod forwards it to the one that does, WebSockets and
event streams included. So the Service needs no session affinity, and an
Ingress can send anything anywhere.

Each pod is a shard: `clax-0`, `clax-1`, …, each with a volume of its own.
Each instance names the shard whose volume keeps its home, a home like
`clax serve`'s (WAL, reader pool, checkpoints), and runs on that pod. A pod
that dies takes its shard's instances down until it comes back, with its
volume.

Requests are matched to instances by `Host`: an instance's public origin,
or a site host under its site domain (`<artifact ID>.<site_domain>`). One
instance can be the default, which answers every other host.

The fleet adds instances, not throughput for any one of them: one instance
serves as many requests as one pod can. To grow, run more instances (one
per team, customer or project) on more shards.

## The image

Each release tag publishes `ghcr.io/empathic/clax-fleet:<version>` (and
`:sha-<commit>`) for linux/amd64 and linux/arm64, each built on a runner of
its own architecture. The overlay names it through kustomize's `images`
(`newTag`), which `scripts/bump-version.sh` writes with every other
version, so the manifests at a release's tag name the image that tag
publishes. The first published image is the first release after 0.6.0,
whose tag predates the workflow; until then, and for your own changes,
build it:

```
docker build -f deploy/Dockerfile \
  --build-arg CLAX_BUILD_COMMIT="$(git rev-parse HEAD)" \
  -t <registry>/clax-fleet:<tag> .
```

This builds the web UI with the pinned Node and pnpm, then the release
`clax-fleet` with the pinned Rust (with BuildKit cache mounts, one per
target platform, so later builds rebuild only what changed), then a slim
Debian runtime that runs as UID 10001. Base images are pinned by digest.
`CLAX_BUILD_COMMIT` is the commit `/status` reports; without it, `unknown`.

In a cluster of your own, deploy from an overlay that names your image and
your catalog, so upgrading the repository never conflicts with local
edits:

```yaml
# my-clax/kustomization.yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  # The release this checkout is at (deploy/k8s ships from the first
  # release after 0.6.0).
  - github.com/empathic/clax//deploy/k8s/sqlite?ref=v0.6.0
images:
  - name: clax-fleet
    newName: <registry>/clax-fleet
    newTag: <tag>
configMapGenerator:
  - name: clax-fleet-catalog
    behavior: replace
    files:
      - fleet.toml
    options:
      disableNameSuffixHash: true
```

To try it on kind with a local build, load the image and point the overlay
at it:

```
docker build -f deploy/Dockerfile -t clax-fleet:dev .
kind load docker-image clax-fleet:dev
cd deploy/k8s/sqlite && kustomize edit set image clax-fleet=clax-fleet:dev
```

## The catalog

`deploy/k8s/base/fleet.toml` becomes the `clax-fleet-catalog` ConfigMap,
mounted at `/etc/clax/fleet.toml`:

```toml
shards = 2               # the StatefulSet's replicas

[[instance]]
name = "acme"            # lowercase letters, digits and inner dashes
shard = 0                # the pod (clax-0) whose volume keeps it
create = true            # may make its home (a new instance only)
default = true           # answers hosts no instance names
paused = false           # true: stopped, its hosts answer 503
config = '''
[hosted]
public_origin = "https://acme.clax.example.com"
site_domain = "acme-sites.example.net"
setup_secret_env = "CLAX_SETUP_SECRET_ACME"
'''
```

`config` is the instance's `config.toml`. It must have a `[hosted]` table:
a fleet only serves hosted instances, which sign people in through the
browser. No two instances may share an origin, and no instance's site
domain may be, be under or be above another instance's site domain or
public host, so no tenant's artifact host is another tenant's host.

That check does not make tenants cross-site. A browser decides what is
same-site by registrable domain, from the Public Suffix List: two hosts
under one registrable domain (`example.com`) are same-site however they
nest. An artifact page under `acme-sites.example.com` could then set a
`Domain=example.com` cookie that every tenant's origin under
`example.com` receives, and `SameSite` cookies would not separate them.
So give each tenant's site domain a registrable domain of its own, shared
with no other tenant's origin or site domain (as `acme-sites.example.net`
is above), or one on the Public Suffix List
([docs/hosted/public-suffix.md](hosted/public-suffix.md)). The fleet does
not check this.

Every pod reloads the catalog every 5 s. When an instance is added it
starts on its first request. When one is removed, paused, or its `config`
or shard changes, it stops, and its next request starts it again from the
new entry. A catalog that does not parse leaves the previous one in
effect. The reload error is logged once, `/status` shows it, and
`clax_fleet_catalog_stale` is 1. A pod that starts while the mounted
catalog is bad exits saying why, so fix the ConfigMap. Kubernetes takes up
to a minute to update a mounted ConfigMap, and until then a pod that lacks
a new instance answers its host with the default instance (or 404).

Check a catalog before applying it, with the image:

```
docker run --rm -v "$PWD":/c ghcr.io/empathic/clax-fleet:0.6.0 check-catalog /c/fleet.toml
```

**An instance's data is kept under its name.** Removing an entry stops the
instance and keeps its home. Never give a removed instance's name to
another tenant. The fleet records the public origin each instance's data
was made for, and refuses to start a name whose origin changed
(`instance_reused` in the log).

**Instances' secrets.** Each instance's setup secret (its `[hosted]
setup_secret_env`) and any GitHub client secret come from the
`clax-secrets` Secret, which pods read when they start. Every instance
names a variable of its own (`CLAX_SETUP_SECRET_<NAME>`): the catalog is
refused when two instances read the same one, or both leave it to the
default `CLAX_SETUP_SECRET`, since either's secret would then open the
other's `/setup`, and when one names `CLAX_KEK` or `CLAX_FLEET_PEER_KEY`.
After adding an instance's secret, or rotating one, run `kubectl rollout
restart sts clax`. An instance whose setup secret is missing logs that
`/setup` stays closed.

## Deploying

```
kubectl create secret generic clax-secrets \
  --from-literal=CLAX_FLEET_PEER_KEY="$(openssl rand -hex 32)" \
  --from-literal=CLAX_SETUP_SECRET_EXAMPLE="$(openssl rand -hex 16)"
kubectl apply -k deploy/k8s/sqlite
```

The StatefulSet `clax` runs pods `clax-0`, `clax-1`, …, each with a 10 Gi
volume at `/var/lib/clax`, spread across nodes where it can. The headless
Service `clax-peers` names each one, and `CLAX_FLEET_PEER`
(`clax-{shard}.clax-peers:7480`) tells pods where to forward requests for
each shard. `CLAX_FLEET_PEER_KEY` must be the same on every shard, so that
a pod can tell a real forward from a client's request. A shard without it
does not start, and says why; the StatefulSet requires the `clax-secrets`
Secret.

**Scaling out:** raise `shards` in the catalog first (every pod reloads it,
so nothing restarts), then `kubectl scale sts clax --replicas=N`, and give
new instances the new shards with `create = true`. A pod whose ordinal is
past `shards` serves nothing of its own and logs so.

**Moving an instance** to another shard means moving its home with it.
The new shard never makes an empty instance in its place, because without
`create = true` a missing home keeps the instance down:

1. Set `paused = true` on the instance and wait for its pod to log
   `instance stopped`. Its hosts answer 503 meanwhile, never another
   instance.
2. Copy its home between the pods:
   `kubectl exec clax-0 -- tar -C /var/lib/clax/instances -cf - acme | kubectl exec -i clax-1 -- tar -C /var/lib/clax/instances -xf -`
3. Set its new `shard`, and remove `paused`.

**Draining.** On SIGTERM a pod answers `/readyz` with 503 at once, and
keeps serving for `CLAX_FLEET_DRAIN_DELAY` (5 s) while the Service drops
it. It then stops every instance and ends the open connections within
10 s. `terminationGracePeriodSeconds` is 45. An instance whose store takes
more than 10 s to stop goes on stopping, and is not started again on that
pod until it has.

## CLAX_KEK

The key-encryption key seals instances' secrets (ClaxBot's model keys). It
is optional: without it, each home makes its own key on first start, which
its volume keeps. With it:

- Choose it before an instance first starts. A home's key is never
  replaced. An instance whose home holds a different key than `CLAX_KEK`
  does not start, and its log names both keys' fingerprints. Setting
  `CLAX_KEK` later therefore needs it to be one home's key, or homes with
  their own keys.
- Keep it safe and backed up: without it, the secrets sealed under it
  cannot be read. There is no rotation yet.

## Security

- **Pod network.** A forward between pods carries the client's request as
  it came, with its cookies and tokens, in plain HTTP. Run the fleet on an
  encrypted pod network (a WireGuard CNI) or a mesh with mTLS.
- **NetworkPolicy.** `base/networkpolicy.yaml` admits port 7480 only from
  the fleet's pods and from namespaces labelled
  `clax.empathic.dev/ingress=true` (label your Ingress controller's). It
  assumes clients come through an in-cluster Ingress controller. A
  `LoadBalancer` or `NodePort` Service in front instead delivers traffic
  from outside addresses (the clients' with `externalTrafficPolicy: Local`,
  else the nodes'), which no selector matches, so on a CNI that enforces
  policies every request times out: add a rule for 7480 with an `ipBlock`
  of those addresses (the comment in the policy shows one). It
  admits 7481 only from the fleet's pods and namespaces labelled
  `clax.empathic.dev/metrics=true`; kubelet probes pass on common CNIs.
- **The peer key.** A forward carries `x-clax-fleet-key`, and without it
  the forwarding headers are a client's and ignored. Every shard needs the
  same one.
- **What clients see.** A 503 says only that the instance is unavailable.
  Why it is (an error, a pod's name) goes to the log. On port 7481,
  `/status` (which names every tenant's instance) answers only to a caller
  with the peer key, whatever its address: under a sidecar mesh every
  connection reaches the pod from loopback. `/metrics` holds counters and
  gauges only, no instance names, and answers whoever the NetworkPolicy
  admits. Port 7482, bound to the pod's loopback alone
  (`CLAX_FLEET_LOCAL_ADMIN_LISTEN`), serves the same routes with `/status`
  open, for `kubectl port-forward`.
- **Client addresses.** Hosted mode's abuse limits count requests by client
  address. Behind Cloudflare, set `[hosted] behind_cloudflare = true` and
  the address in `CF-Connecting-IP` counts. That header is then believed
  from every peer, so the Ingress must accept only Cloudflare: its published
  ranges (https://www.cloudflare.com/ips/) in the controller's allow list
  (ingress-nginx: the `nginx.ingress.kubernetes.io/whitelist-source-range`
  annotation), or Authenticated Origin Pulls. Without Cloudflare, the peer
  address counts, and behind an Ingress that is the controller's address
  for every client.

## Ingress, DNS and TLS

`deploy/k8s/examples/ingress.yaml` routes an instance's origin and the
wildcard for its site domain to the `clax` Service. Each site domain needs
a wildcard DNS record and a wildcard certificate (cert-manager with a
DNS-01 issuer). Raise the controller's read and send timeouts for event
streams and WebSockets (the example's are ingress-nginx's), and its body
limit to above the 16 MB that a publish may send.

## Sizing

An instance's footprint is its store's (WAL, reader pool, page cache) and
its realtime state. `CLAX_FLEET_SQLITE_HEAP_MIB` (1024) bounds every SQLite
database of a pod together: past it, page caches give memory back. Keep it
under the pod's limit. Each pod requests 512 Mi and is limited to 2 Gi, and
its volume is 10 Gi; raise them with the instances a shard keeps.

## Upgrading

The StatefulSet restarts one shard at a time. Each shard's instances are
down while its pod restarts, and migrate when they start. `clax serve`'s
copy of a database before a migration is kept in each home.

## Backups and restore

A volume snapshot of a live home is only crash-consistent. For a
consistent copy of one instance, pause it (`paused = true`), copy its home
out (`kubectl exec … tar`, as in Moving an instance), and unpause it. To
restore, pause it, copy the home back, and unpause.

## Operating it

Each pod serves the instances on port 7480, and `/healthz` (alive),
`/readyz` (taking requests), `/status` (with the peer key) and `/metrics`
on 7481; on 7482, its loopback alone, the same with `/status` open:
`kubectl port-forward pod/clax-0 7482` then `curl localhost:7482/status`.
`/status` is JSON: the pod, its version and commit, its shard, the catalog
in effect and its reload state, the instances running here, and those
whose last start failed (`failing`: the error, the failures in a row, and
when the next start is tried). `/metrics` is Prometheus text: requests
answered here, forwarded and refused, forward errors, instance starts and
failures, catalog reload failures, and gauges of running instances and
readiness.

**An instance that fails to start** (a failing migration, a home holding
another `CLAX_KEK`, a missing home on its shard) is not started again for
every request: requests answer 503 at once, and the next start is tried
after 2 s, doubling with each failure in a row up to a minute. A change to
its catalog entry is tried at once.

**Scraping `/metrics`.** Label the Prometheus namespace
`clax.empathic.dev/metrics=true`, which `base/networkpolicy.yaml` admits
to 7481, and scrape the pods' `admin` port. With the Prometheus Operator:

```yaml
apiVersion: monitoring.coreos.com/v1
kind: PodMonitor
metadata:
  name: clax
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: clax
      app.kubernetes.io/component: fleet
  podMetricsEndpoints:
    - port: admin
      path: /metrics
```

Prometheus needs no credential, and must never be given the peer key: it
is what makes a forward believed on 7480. An instance's own `/healthz`
answers on 7480 at its origin, with the version.

**Which pod runs which instance:** `kubectl exec clax-0 -- clax-fleet
status` lists each shard's running instances.

**Logs** are text by default; set `CLAX_FLEET_LOG_FORMAT=json` for one JSON
object per line, with each request's `instance` in its span. `RUST_LOG`
sets the filter (`info`).

| Variable | Flag | Meaning |
|---|---|---|
| `CLAX_FLEET_CATALOG` | `--catalog` | the catalog (`/etc/clax/fleet.toml`) |
| `CLAX_FLEET_DATA_DIR` | `--data-dir` | the homes go under `<dir>/instances` (`/var/lib/clax`) |
| `CLAX_FLEET_LISTEN` | `--listen` | the instances' address (`0.0.0.0:7480`) |
| `CLAX_FLEET_ADMIN_LISTEN` | `--admin-listen` | health, status (with the peer key) and metrics (`0.0.0.0:7481`) |
| `CLAX_FLEET_LOCAL_ADMIN_LISTEN` | `--local-admin-listen` | the same, `/status` without the key, on a loopback address (`127.0.0.1:7482`) |
| `POD_NAME` | `--pod` | this pod's name in the fleet |
| `POD_IP`, `CLAX_FLEET_ADVERTISE` | `--pod-ip`, `--advertise` | where other pods reach this one (`$POD_IP:<listen port>`) |
| `CLAX_FLEET_ORDINAL` | `--ordinal` | this pod's shard (by default, its name's suffix) |
| `CLAX_FLEET_PEER` | `--peer` | where shard `{shard}` is reached |
| `CLAX_FLEET_PEER_KEY` | `--peer-key` | the key forwards carry (required, the same on every shard) |
| `CLAX_FLEET_SQLITE_HEAP_MIB` | `--sqlite-heap-mib` | the memory all of a pod's SQLite databases aim to stay under (1024; 0 for none) |
| `CLAX_KEK` | | the key secrets are sealed under (64 hex digits; optional) |
| `CLAX_FLEET_DRAIN_DELAY` | `--drain-delay-secs` | seconds to keep serving after SIGTERM (5) |
| `CLAX_FLEET_LOG_FORMAT` | `--log-format` | `text` or `json` |
| `RUST_LOG` | | the log filter (`info`) |

## Limits

- One instance's work is one pod's. The fleet scales the number of
  instances, not the throughput of one.
- A pod that is down takes its shard's instances down with it until it
  comes back; nothing fails them over to another pod.
- An instance starts on its first request, so its first request after a
  restart waits for its database to open (and to migrate, after an
  upgrade).
- A forward to a pod whose node is lost (no reset) fails with 502 within
  about 30 s, by TCP keepalive and, on Linux, `TCP_USER_TIMEOUT`. An owner
  that is up but sends no response headers is given up on after 61
  minutes, past Clax's longest long poll.
- Forwarded requests reach the owner from another pod. The owner sees that
  pod as the peer, so it counts by `CF-Connecting-IP` only behind
  Cloudflare (Security, above).

## Testing

`just test-affected` runs `clax-fleet`'s tests:

- the catalog;
- two-pod fleets in one process: every pod answering every instance,
  catalog changes (paused and removed instances, changed configs and
  shards), hop limits, forwards for an instance the owner lacks, shards
  without a home, a failed start's backoff, and the admin port's key;
- the proxy (WebSockets, streaming, hop limits, the peer key);
- checks that the manifests set only variables the binary reads, and read
  only Secrets that the docs say to create or a kustomization generates.
