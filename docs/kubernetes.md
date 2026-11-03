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
instance can be the default, which answers every other host: for a fleet
of one tenant. With more, leave none, so that a host no instance names
gets 404 rather than one tenant's instance.

The fleet adds instances, not throughput for any one of them: one instance
serves as many requests as one pod can. To grow, run more instances (one
per team, customer or project) on more shards.

## The image

Each release tag publishes `ghcr.io/empathic/clax-fleet:<version>` for
linux/amd64 and linux/arm64, each built on a runner of its own
architecture, and tags it `:<major>.<minor>` (the newest patch of that
series) and `:sha-<commit>` too. There is no `:latest`. Before any tag
names it, the image is deployed to a kind cluster and has to serve
(`scripts/fleet-kind-smoke.sh`, below). The overlay names it through
kustomize's `images` (`newTag`), which `scripts/bump-version.sh` writes
with every other version, so the manifests at a release's tag name the
image that tag publishes, and anyone can pull it. **No release publishes
the image or ships `deploy/k8s` yet**: the first to do both is the first
release after 0.6.0, whose tag predates them. Until then, and for your own
changes, build the image and push it to a registry your cluster can pull
from:

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

In a cluster of your own, deploy from an overlay that names a release's
manifests and your catalog, so upgrading never conflicts with local edits.
Each release attaches them as `clax-fleet.yaml`: `deploy/k8s/release` (the
sqlite overlay without the example catalog) with the image pinned by
digest, so it runs the bytes that were tested and signed even if a tag
moves (`scripts/render-fleet-manifests.sh` makes it). Nothing to clone:

```yaml
# my-clax/kustomization.yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  # The release's manifests. Before the first release that attaches them,
  # deploy/k8s/release at a commit of main instead:
  # github.com/empathic/clax//deploy/k8s/release?ref=<commit>
  - https://github.com/empathic/clax/releases/download/v0.6.0/clax-fleet.yaml
images:
  # The name the manifests give the image, not `clax-fleet`: kustomize
  # matches what it has already rewritten, so `name: clax-fleet` here
  # would match nothing and the release's image would stay.
  - name: ghcr.io/empathic/clax-fleet
    newName: <registry>/clax-fleet
    newTag: <tag>
replicas:
  # As many as the catalog's `shards`.
  - name: clax
    count: 2
configMapGenerator:
  - name: clax-fleet-catalog
    files:
      - fleet.toml
    options:
      # Pods reload the catalog in place, so its name never changes.
      disableNameSuffixHash: true
```

Leave out `images` to run the release's own image, by its digest. To
upgrade, change the version in the URL and apply again.

**Checking an image.** Each image is signed with cosign, keylessly, as
the `fleet-image` workflow of `empathic/clax`, and carries an SBOM and
build provenance:

```
cosign verify ghcr.io/empathic/clax-fleet:0.6.0 \
  --certificate-identity-regexp '^https://github\.com/empathic/clax/\.github/workflows/fleet-image\.yml@refs/tags/v' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
docker buildx imagetools inspect ghcr.io/empathic/clax-fleet:0.6.0 --format '{{json .SBOM}}'
```

A cluster that admits only signed images (Kyverno's `verifyImages`, or
Sigstore's policy-controller) takes the same identity and issuer.

**Trying it on kind.** `scripts/fleet-kind-smoke.sh` deploys a local image
to a kind cluster of its own as the Deploying steps do, with two instances
on two shards. It checks that each instance answers through either pod,
which tests the forward between them, and then deletes the cluster
(`CLAX_SMOKE_KEEP=1` keeps it):

```
docker build -f deploy/Dockerfile --build-arg CLAX_BUILD_COMMIT="$(git rev-parse HEAD)" -t clax-fleet:dev .
scripts/fleet-kind-smoke.sh clax-fleet:dev
```

## The catalog

`deploy/k8s/base/fleet.toml` becomes the `clax-fleet-catalog` ConfigMap,
mounted at `/etc/clax/fleet.toml`:

```toml
shards = 2               # the StatefulSet's replicas

[[instance]]
name = "acme"            # lowercase letters, digits and inner dashes
shard = 0                # the pod (clax-0) whose volume keeps it
create = true            # may make its home: a new instance's first start only
default = false          # true: answers hosts no instance names (one tenant only)
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
effect, and so does one that gives an instance another shard while it has
`create = true` (Moving an instance). The reload error is logged once,
`/status` shows it, and `clax_fleet_catalog_stale` is 1. A pod that starts while the mounted
catalog is bad exits saying why, so fix the ConfigMap. Kubernetes takes up
to a minute to update a mounted ConfigMap, and until then a pod that lacks
a new instance answers its host with the default instance (or 404).

Check a catalog before applying it, with the image (your own build's name
until a release publishes it):

```
docker run --rm -v "$PWD":/c ghcr.io/empathic/clax-fleet:0.6.0 check-catalog /c/fleet.toml
```

**`create = true` is for a new instance's first start.** It lets the
instance's shard make an empty home where it finds none, which the pod
logs at `warn` (`making a new, empty home`). Once the instance has
started, remove it from the entry: left on, a shard that finds no home
(a wrong `shard`, a copy put in the wrong place) makes an empty instance
that answers the tenant's hosts, its `/setup` open to the setup secret.

**An instance's data is kept under its name.** Removing an entry stops the
instance and keeps its home. Never give a removed instance's name to
another tenant. The fleet records the public origin each instance's data
was made for, and refuses to start a name whose origin changed
(`instance_reused` in the log).

**Instances' secrets.** An instance reads two secrets, both from the
`clax-secrets` Secret, which every instance of a pod sees whole:

| `[hosted]` setting | the secret | default variable |
|---|---|---|
| `setup_secret_env` | what the first visit to `/setup` asks for | `CLAX_SETUP_SECRET` |
| `github_client_secret_env` | its GitHub OAuth app's client secret, with `github_client_id` | `GITHUB_CLIENT_SECRET` |

Every instance names variables of its own (`CLAX_SETUP_SECRET_<NAME>`,
`CLAX_GITHUB_SECRET_<NAME>`). The catalog is refused when two instances
read one variable, both leaving it to the default included: either's
setup secret would open the other's `/setup`, and each instance's OAuth
app has a secret of its own. It is refused too when an instance reads
`CLAX_KEK` or `CLAX_FLEET_PEER_KEY`. A hosted instance reads no `[bot]`
or `[sample]` key variable: ClaxBot's model keys are each instance's own,
stored sealed in its home (CLAX_KEK).

The StatefulSet mounts the Secret at `/etc/clax-secrets`
(`CLAX_FLEET_SECRETS_DIR`), one file per key, which the kubelet updates in
place within a minute or two of a change. An instance reads its secrets
from those files when it starts, and only from them: a key with no file
there is not set, whatever the environment the pod started with holds, so
a key removed from the Secret is gone at the instance's next start. (The
environment is read only by a fleet run without a secrets directory.) A
pod whose `CLAX_FLEET_SECRETS_DIR` is not a directory does not start, and
a file that cannot be read is logged. So **adding a tenant restarts
nothing**: add its keys to the Secret, then its entry to the catalog:

```
kubectl patch secret clax-secrets --type merge \
  -p '{"stringData":{"CLAX_SETUP_SECRET_GLOBEX":"'"$(openssl rand -hex 16)"'"}}'
```

To **rotate** an instance's secret, change it in the Secret, wait for the
pod to see it, then pause and unpause the instance, which reads it again as
it starts. To **revoke** one (close `/setup` once it is done, or drop a
leaked OAuth secret), remove its key from the Secret the same way. An
instance whose setup secret is missing logs that `/setup` stays closed. `CLAX_FLEET_PEER_KEY` and `CLAX_KEK` are the process's own,
read from the environment as the pod starts: changing either needs
`kubectl rollout restart sts clax`.

## Deploying

1. **The image.** Use a release's (Checking an image, under The image),
   or build and push your own (The image).
2. **Your overlay.** Make `my-clax/` with the `kustomization.yaml` of The
   image and a `fleet.toml` of your own instances (The catalog), each
   with its own origin and its own `setup_secret_env` (and
   `github_client_secret_env`, signing in with GitHub). Check it:
   `docker run --rm -v "$PWD/my-clax":/c <image> check-catalog /c/fleet.toml`.
   Applying `deploy/k8s/sqlite` itself deploys the example catalog's
   instance at `https://clax.example.com`, whose name is then that
   origin's for good.
3. **The Secret**, in the namespace you deploy to: the peer key, and one
   setup secret for each instance, under the variable its
   `setup_secret_env` names:

   ```
   kubectl create secret generic clax-secrets \
     --from-literal=CLAX_FLEET_PEER_KEY="$(openssl rand -hex 32)" \
     --from-literal=CLAX_SETUP_SECRET_ACME="$(openssl rand -hex 16)"
   ```

4. **Let the Ingress controller in.** The NetworkPolicy admits clients
   only through it (Security):
   `kubectl label namespace <controller's namespace> clax.empathic.dev/ingress=true`.
5. **Apply:** `kubectl apply -k my-clax`, then
   `kubectl rollout status sts/clax`.
6. **The Ingress.** Copy `deploy/k8s/examples/ingress.yaml`, put each
   instance's origin host and site-domain wildcard in it, and apply it
   (Ingress, DNS and TLS).
7. **Check it:** `kubectl exec clax-0 -- clax-fleet status` lists each
   shard as ready, and `curl https://<origin>/healthz` answers through the
   Ingress. Then open `https://<origin>/setup` with the instance's setup
   secret.

The StatefulSet `clax` runs pods `clax-0`, `clax-1`, …, each with a 10 Gi
volume at `/var/lib/clax`, spread across nodes where it can. The headless
Service `clax-peers` names each one, and `CLAX_FLEET_PEER`
(`clax-{shard}.clax-peers:7480`) tells pods where to forward requests for
each shard. `CLAX_FLEET_PEER_KEY` must be the same on every shard, so that
a pod can tell a real forward from a client's request. A shard without it
does not start, and says why; the StatefulSet requires the `clax-secrets`
Secret.

**Scaling out:** raise `shards` in `my-clax/fleet.toml` and `count` under
`replicas` in `my-clax/kustomization.yaml` together, to the same number,
and apply. Give new instances the new shards with `create = true`, and
remove it once each has started. Change
the replicas there, not with `kubectl scale`: the next apply (or a GitOps
controller) would put them back, taking the new shards' instances down. A
pod whose ordinal is past `shards` serves nothing of its own and logs so.

**Scaling in** to N shards:

1. Move every instance on shard N or above to a lower one (Moving an
   instance). The catalog refuses an instance on a shard past `shards`, so
   this comes first.
2. Lower `shards` and the replicas' `count` to N together, and apply.
3. The removed pods' volumes stay (`data-clax-N`, …, kept by the
   StatefulSet's default retention). Delete each with
   `kubectl delete pvc data-clax-<ordinal>` once nothing on it is needed.

**Moving an instance** to another shard means moving its home with it.
The new shard never makes an empty instance in its place, because without
`create = true` a missing home keeps the instance down. A catalog that
changes an instance's shard while it still has `create = true` is refused
(the previous one stays in effect, as for one that does not parse), but
only by pods running when it arrives: a pod that starts with it takes it,
so remove `create = true` before the move, not with it:

1. Set `paused = true` on the instance, remove `create = true` if it is
   still there, apply, and wait until
   `kubectl exec clax-0 -- clax-fleet status` no longer lists it among
   its shard's running instances. The ConfigMap can take a minute to
   reach the pod. Its hosts answer 503 meanwhile, never another instance.
2. Copy its home between the pods, into no home of that name: `tar -x`
   keeps the files already there that the copy lacks, and a `-wal` left
   beside a copied database is replayed onto it. If the new shard has
   one (from an earlier move), move it aside first:

   ```
   kubectl exec clax-1 -- mv /var/lib/clax/instances/acme /var/lib/clax/instances/acme.before-move
   kubectl exec clax-0 -- tar -C /var/lib/clax/instances -cf - acme \
     | kubectl exec -i clax-1 -- tar -C /var/lib/clax/instances -xf -
   ```

3. Set its new `shard`, remove `paused`, and apply. Leave `create` out.
4. Once it serves from the new shard, move the old copy aside, so that a
   `shard` set back by mistake finds no stale home to start:
   `kubectl exec clax-0 -- mv /var/lib/clax/instances/acme /var/lib/clax/instances/acme.moved`.
   A name with a dot is no instance's; delete it when you no longer need
   it.

**Draining.** On SIGTERM a pod answers `/readyz` with 503 at once, and
keeps serving for `CLAX_FLEET_DRAIN_DELAY` (5 s) while the Service drops
it. It then stops every instance and ends the open connections within
10 s. `terminationGracePeriodSeconds` is 45. An instance whose store takes
more than 10 s to stop goes on stopping, and is not started again on that
pod until it has. An instance still starting (a long migration) is waited
for 10 s too; past that the pod logs `the instance is still starting` and
exits without stopping it, inside its grace period rather than killed at
its end.

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
  assumes clients come through an in-cluster Ingress controller on the pod
  network. **If every request times out**, they come from addresses no
  selector matches, and on a CNI that enforces policies they are dropped.
  Add a rule for 7480 with an `ipBlock` of those addresses (the comment in
  the policy shows one):
  - a `LoadBalancer` or `NodePort` Service in front: the clients'
    addresses with `externalTrafficPolicy: Local`, else the nodes';
  - an Ingress controller with `hostNetwork: true` (common for
    ingress-nginx on bare metal): the nodes' addresses;
  - a cloud load balancer that sends straight to pod IPs: for GKE's
    container-native Ingress, `130.211.0.0/22` and `35.191.0.0/16`; for
    the AWS Load Balancer Controller in IP mode, its subnets.

  It admits 7481 only from the fleet's pods and namespaces labelled
  `clax.empathic.dev/metrics=true`; kubelet probes pass on common CNIs.
- **The peer key.** A forward carries `x-clax-fleet-key`, and without it
  the instance it names is a client's and ignored: the `Host` decides. Its
  hop count is believed from anyone, so a client can only keep its own
  request from being forwarded, and shards whose keys differ (a rotation
  under way) still stop a request after two hops. Every shard needs the
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
  for every client (and, for a request another pod forwarded, that pod's).
  All of an instance's users then share one address's limits: one person's
  failed sign-ins (429 `rate_limited`) lock everyone out for a while, and
  128 event streams and WebSockets in all are open at once (429
  `too_many_streams` past them). For an instance with more than a few
  users, put Cloudflare in front. Hosted mode does not yet believe a
  forwarded-for header from a trusted proxy.

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
under the pod's limit. Each pod requests 512 Mi and is limited to 2 Gi;
raise them with the instances a shard keeps, with a patch to the
StatefulSet's container in your overlay. A full volume takes every
instance of its shard down, so alert on it
(`kubelet_volume_stats_used_bytes` over `kubelet_volume_stats_capacity_bytes`).

**The volume** is 10 Gi, of the cluster's default StorageClass. A
StatefulSet's volume claims cannot be changed once it exists, so before
the first apply choose a class that can grow (`allowVolumeExpansion:
true`), and a size, in your overlay:

```yaml
patches:
  - target:
      kind: StatefulSet
      name: clax
    patch: |-
      - op: add
        path: /spec/volumeClaimTemplates/0/spec/storageClassName
        value: <a class with allowVolumeExpansion>
      - op: replace
        path: /spec/volumeClaimTemplates/0/spec/resources/requests/storage
        value: 20Gi
```

To grow a shard's volume later, patch its claim, which the class then
expands while the pod runs:
`kubectl patch pvc data-clax-0 -p '{"spec":{"resources":{"requests":{"storage":"40Gi"}}}}'`.
To give new shards that size too, change it in the overlay, delete the
StatefulSet but not its pods (`kubectl delete sts clax --cascade=orphan`),
and apply: the new StatefulSet adopts the running pods.

## Upgrading

The StatefulSet restarts one shard at a time. Each shard's instances are
down while its pod restarts, and migrate when they start. A pod is ready
before any of its instances starts, so the rollout goes on through every
shard even when a release's migration fails. **Upgrade one shard first:**

1. Hold the rollout at the highest shard (here, of 3):
   `kubectl patch sts clax -p '{"spec":{"updateStrategy":{"rollingUpdate":{"partition":2}}}}'`.
2. Apply the new release (its version in your overlay's URL, The image)
   or image. Only `clax-2` restarts.
3. Start each of its instances (`curl https://<origin>/healthz`), and check
   that `kubectl exec clax-2 -- clax-fleet status` shows none `failing`.
4. Release the rest:
   `kubectl patch sts clax -p '{"spec":{"updateStrategy":{"rollingUpdate":{"partition":0}}}}'`.

**Rolling back.** An older image refuses a database a newer one migrated,
so setting the image back is not enough for an instance that already
started on the new one. Before migrating, each home keeps a copy of its
database as it was, in `backups/clax-<schema version>-<UTC time>.db` (the
newest three). For each such instance:

1. Pause it, apply, and wait until it no longer runs (Moving an instance).
2. Set its database aside, and put the newest copy in its place:

   ```
   kubectl exec clax-0 -- sh -c 'cd /var/lib/clax/instances \
     && mkdir acme.rolled-back \
     && for f in clax.db clax.db-wal clax.db-shm; do [ ! -e acme/$f ] || mv acme/$f acme.rolled-back/; done \
     && cp "acme/backups/$(ls acme/backups | grep "^clax-.*\.db$" | sort -t- -k3,3 | tail -n 1)" acme/clax.db'
   ```

3. Once every such instance is done, set the release (or image) back,
   apply, and unpause them.

What an instance stored after its migration is lost.

## Backups and restore

A volume snapshot of a live home is only crash-consistent. For a
consistent copy of one instance:

1. Pause it, apply, and wait until it no longer runs, as in Moving an
   instance. A copy taken sooner is of a store still writing.
2. Copy its home out:
   `kubectl exec clax-0 -- tar -C /var/lib/clax/instances -cf - acme > acme.tar`.
3. Remove `paused`, and apply.

To restore, pause it and wait the same way, then move the current home
aside before extracting the copy, so that none of its files (a `-wal`
beside the database) is left among the restored ones. Then unpause it:

```
kubectl exec clax-0 -- mv /var/lib/clax/instances/acme /var/lib/clax/instances/acme.before-restore
kubectl exec -i clax-0 -- tar -C /var/lib/clax/instances -xf - < acme.tar
```

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
failures, catalog reload failures, requests carrying another peer key
(`clax_fleet_peer_key_mismatches_total`: shards hold different
`CLAX_FLEET_PEER_KEY` values, so they do not believe each other's
forwards), and gauges of running instances and readiness.

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
| `CLAX_FLEET_SECRETS_DIR` | `--secrets-dir` | where instances' secrets are read, a file per variable, and the environment not at all (the StatefulSet: `/etc/clax-secrets`; none by default, which reads the environment) |
| `POD_NAME` | `--pod` | this pod's name in the fleet |
| `POD_IP`, `CLAX_FLEET_ADVERTISE` | `--pod-ip`, `--advertise` | where other pods reach this one (`$POD_IP:<listen port>`) |
| `CLAX_FLEET_ORDINAL` | `--ordinal` | this process's shard, outside a StatefulSet (by default, its name's suffix; one that disagrees with it is refused) |
| `CLAX_FLEET_PEER` | `--peer` | where shard `{shard}` is reached, as `host:port` (no scheme): refused without `{shard}` when there are several shards, or without a port, and a warning at start when this shard's host does not begin with the pod's name (a kustomize `namePrefix` renames the pods, not this value) |
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
  shards, a shard changed under `create = true`), hop limits (a client's,
  and between pods whose keys and catalogs disagree), forwards for an
  instance the owner lacks, shards without a home, a failed start's
  backoff, and the admin port's key;
- the proxy (WebSockets, streaming, hop limits, the peer key);
- checks that the manifests set only variables the binary reads, and read
  only Secrets that the docs say to create or a kustomization generates.

The image and the manifests are tested together on a cluster by
`scripts/fleet-kind-smoke.sh` ([The image](#the-image)). The `fleet-image`
workflow runs it on every image before tagging it, and on pull requests
that change the Dockerfile, its context or the manifests.
