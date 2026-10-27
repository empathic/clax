# Clax on Kubernetes

`clax-fleet` serves hosted Clax instances from a set of pods, backed by
SQLite or by PostgreSQL, and scales by adding pods. `deploy/k8s` holds the
manifests (kustomize), `deploy/Dockerfile` builds the image (published by
`.github/workflows/fleet-image.yml` for each release), and
`crates/clax-fleet` is the server; its crate docs cover the design in depth.

## How it scales

A Clax instance has one writer. Its store's write connection, its realtime
state (event streams, presence, seats, long polls) and its background tasks
all live in one process, the way a Durable Object holds an instance in the
hosted Worker. The fleet keeps that rule: **each instance runs in exactly
one pod at a time, and the fleet scales by spreading instances across
pods.** Any pod takes any request. A pod that runs the request's instance
answers it in process; any other pod forwards it to the one that does,
WebSockets and event streams included. So the Service needs no session
affinity, and an Ingress can send anything anywhere.

Requests are matched to instances by `Host`: an instance's public origin,
or a site host under its site domain (`<artifact ID>.<site_domain>`). One
instance can be the default, which answers every other host.

The fleet adds instances, not throughput for any one of them: one instance
serves as many requests as one pod can. To grow, run more instances (one
per team, customer or project) on more pods.

## Choosing a backend

| | SQLite | PostgreSQL |
|---|---|---|
| Workload | StatefulSet (`deploy/k8s/sqlite`) | Deployment + HorizontalPodAutoscaler (`deploy/k8s/postgres`) |
| Where an instance's data lives | Its home on its shard pod's volume, as `clax serve` keeps one (WAL, reader pool) | PostgreSQL: the database as SQLite pages, artifact content and app data as rows |
| Which pod runs an instance | The pod whose ordinal is the instance's `shard` in the catalog | Whichever pod holds its lease; with no live lease, the pod that rendezvous hashing prefers |
| A pod dies | Its instances wait for it to come back (its volume holds them) | Its leases expire within 30 s, and the next request starts each instance on another pod |
| Scaling out | Add shards, and put new instances on them; an instance moves only when you move its home | Add pods (or let the autoscaler add them); each pod takes its share of instances once it has been up 30 s |
| Upgrading | Each shard's instances are down while its pod restarts | Instances move to the pods already upgraded |
| Needs | A ReadWriteOnce volume per pod | A PostgreSQL 13 or newer, and `CLAX_KEK` |

SQLite is the simpler choice and keeps each instance's I/O local. With
PostgreSQL, pods are interchangeable and nothing on them has to survive.

## The image

Each release publishes `ghcr.io/empathic/clax-fleet:<version>`, which the
overlays name through kustomize's `images` (`newTag`). To build your own:

```
docker build -f deploy/Dockerfile -t <registry>/clax-fleet:<tag> .
```

This builds the web UI with the pinned Node and pnpm, then the release
`clax-fleet` with the pinned Rust (with BuildKit cache mounts, so later
builds rebuild only what changed), then a slim Debian runtime that runs as
UID 10001. Base images are pinned by digest.

In a cluster of your own, deploy from an overlay that names your image and
your catalog, so upgrading the repository never conflicts with local
edits:

```yaml
# my-clax/kustomization.yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - github.com/empathic/clax//deploy/k8s/postgres?ref=v0.6.0
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

To try it on kind with a local build, `postgres-dev` uses an image that is
already loaded:

```
docker build -f deploy/Dockerfile -t clax-fleet:dev .
kind load docker-image clax-fleet:dev
cd deploy/k8s/postgres-dev && kustomize edit set image clax-fleet=clax-fleet:dev
```

## The catalog

`deploy/k8s/base/fleet.toml` becomes the `clax-fleet-catalog` ConfigMap,
mounted at `/etc/clax/fleet.toml`:

```toml
shards = 2               # SQLite: the StatefulSet's replicas

[[instance]]
name = "acme"            # lowercase letters, digits and inner dashes
shard = 0                # SQLite: the pod (clax-0) whose volume keeps it
create = true            # SQLite: may make its home (a new instance only)
default = true           # answers hosts no instance names
paused = false           # true: stopped, its hosts answer 503
config = '''
[hosted]
public_origin = "https://acme.clax.example.com"
site_domain = "acme-sites.example.com"
setup_secret_env = "CLAX_SETUP_SECRET_ACME"
'''
```

`config` is the instance's `config.toml`. It must have a `[hosted]` table:
a fleet only serves hosted instances, which sign people in through the
browser. No two instances may share an origin, and no instance's site
domain may be, be under or be above another instance's site domain or
public host. Each tenant's pages stay cross-site from every other tenant's.

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
docker run --rm -v "$PWD":/c ghcr.io/empathic/clax-fleet:0.6.0 check-catalog /c/fleet.toml --sqlite
```

**An instance's data is kept under its name.** Removing an entry stops the
instance and keeps its data: its home on SQLite, its rows on PostgreSQL.
Never give a removed instance's name to another tenant. The fleet records
the public origin each instance's data was made for, and refuses to start
a name whose origin changed (`instance_reused` in the log).

**Instances' secrets.** Each instance's setup secret (its `[hosted]
setup_secret_env`) and any GitHub client secret come from the
`clax-secrets` Secret, which pods read when they start. After adding an
instance's secret, or rotating one, run `kubectl rollout restart` on the
workload. An instance whose setup secret is missing logs that `/setup`
stays closed.

## Deploying on SQLite

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
a pod can tell a real forward from a client's request.

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

## Deploying on PostgreSQL

```
kubectl create secret generic clax-secrets \
  --from-literal=CLAX_POSTGRES_URL='postgresql://clax:<password>@<host>:5432/clax' \
  --from-literal=CLAX_KEK="$(openssl rand -hex 32)" \
  --from-literal=CLAX_SETUP_SECRET_EXAMPLE="$(openssl rand -hex 16)"
kubectl apply -k deploy/k8s/postgres
```

To try it out with a database in the cluster, use `deploy/k8s/postgres-dev`
instead (its comments give the secret to create). It runs one PostgreSQL
pod with a fixed password, reachable only from the fleet's pods, so it is
not for production.

**TLS.** `deploy/k8s/postgres` sets `CLAX_POSTGRES_TLS=on`, which requires
TLS: the connection never falls back to plain text, whatever the server
answers. A URL whose `sslmode` says otherwise is refused, as is TLS off
with `sslmode=require`. If the public roots do not cover a managed
database's CA, put the CA in a Secret, mount it with the commented volume
in `deploy/k8s/postgres/deployment.yaml`, and set `CLAX_POSTGRES_CA` to the mounted file.
`postgres-dev` turns TLS off for its in-cluster database.

**Connections.** Each pod holds `CLAX_POSTGRES_POOL` (8) connections for
instances' data and 2 for the coordinator, which never wait behind data
I/O. Keep `pods × (pool + 2)` plus a rollout's surge under the server's
`max_connections`: 10 pods take 100. Alternatively, put PgBouncer in
transaction mode in front (the fleet keeps no session state beyond a
transaction). `postgres-dev` sets `max_connections=200`. Every connection
uses TCP keepalives and a 120 s `statement_timeout`, so a database lost
without a reset fails calls instead of hanging them.

On first start, `clax-fleet` creates its tables (`clax_fleet_*`) under an
advisory lock. Every pod then heartbeats into `clax_fleet_members` every
5 s, and a pod not seen for 20 s is out. A pod that starts frees the leases
its name held before (a container restarted in place). A request for an
instance goes to the pod that holds its lease. When no live lease names
the instance, it goes to the live pod that rendezvous hashing prefers, and
that pod takes the lease (raising its epoch) and starts the instance. A
lease lasts 30 s, and its holder renews all of its leases in one statement
every 5 s.

**Fencing.** Every write an instance makes is fenced by its lease. Each
page commit, content write and app data write starts by locking the lease
row and checking that it still names this pod at the epoch it acquired. A
pod that lost an instance (paused past its lease, or cut off from the
database) therefore can never write to it again, even before it notices.
It stops the instance once its renewal fails.

**Scaling.** The autoscaler (2 to 10 pods, on CPU and memory) adds pods.
Once a new pod has been up 30 s, each instance that rendezvous hashing now
prefers for it is handed over: its old pod stops it and releases the lease,
and its next request starts it on the new pod. Scale-down is slowed to one
pod every two minutes, because each move cuts that instance's open
streams, which reconnect. The Deployment sets no `replicas`, so an apply
never resets what the autoscaler chose.

**Draining.** On SIGTERM a pod answers `/readyz` with 503 at once, and
keeps serving for `CLAX_FLEET_DRAIN_DELAY` (5 s) while the Service drops
it. It then leaves the membership, stops every instance and releases their
leases (the other pods take them on their next request), and ends the open
connections within 10 s. `terminationGracePeriodSeconds` is 45. An
instance whose store takes more than 10 s to stop goes on stopping, and is
not started again on that pod until it has.

## CLAX_KEK

The key-encryption key seals instances' secrets (ClaxBot's model keys).
On PostgreSQL it is required, because instances move between pods. On
SQLite it is optional: without it, each home makes its own key on first
start. Either way:

- Choose it before an instance first starts. A home's key is never
  replaced. An instance whose home holds a different key than `CLAX_KEK`
  does not start, and its log names both keys' fingerprints. Setting
  `CLAX_KEK` later on SQLite therefore needs it to be one home's key, or
  homes with their own keys.
- Keep it safe and backed up: without it, the secrets sealed under it
  cannot be read. There is no rotation yet.

## Security

- **Pod network.** A forward between pods carries the client's request as
  it came, with its cookies and tokens, in plain HTTP. Run the fleet on an
  encrypted pod network (a WireGuard CNI) or a mesh with mTLS.
- **NetworkPolicy.** `base/networkpolicy.yaml` admits port 7480 only from
  the fleet's pods and from namespaces labelled
  `clax.empathic.dev/ingress=true` (label your Ingress controller's). It
  admits 7481 only from the fleet's pods and namespaces labelled
  `clax.empathic.dev/metrics=true`; kubelet probes pass on common CNIs.
  `postgres-dev` admits 5432 only from the fleet's pods.
- **The peer key.** A forward carries `x-clax-fleet-key`, and without it
  the forwarding headers are a client's and ignored. On PostgreSQL the key
  is kept in the database unless `CLAX_FLEET_PEER_KEY` sets it; on SQLite
  every shard needs the same one.
- **What clients see.** A 503 says only that the instance is unavailable.
  Why it is (database errors, pod names) goes to the log. `/status` and
  `/metrics` answer only to a caller on the pod's loopback (`kubectl exec`,
  `kubectl port-forward`) or one with the peer key.
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

On PostgreSQL a running instance holds up to 32 MiB of SQLite page cache
(`CLAX_FLEET_DB_CACHE_MIB`), up to 16 MiB of page read-ahead, up to 64 MiB
of cached artifact files, and up to 64 open app data stores of 2 MiB cache
each. A transaction's writes are buffered until commit, up to
`CLAX_FLEET_DB_MAX_TRANSACTION_MIB` (256; a migration that rebuilds a table
writes it whole). Budget about 250 MiB per busy instance on top of 200 MiB
per pod, and size the 2 Gi limit by the instances a pod is expected to
run. The autoscaler scales on memory as well as CPU. On SQLite an
instance's footprint is its store's (WAL, reader pool) and its realtime
state.

## Upgrading

- **PostgreSQL.** First take a dump: `pg_dump -t 'clax_fleet_*'`. Then roll
  out. Instances move to the new pods as they settle, and each one
  migrates when it first starts there. Migrated data cannot be read by the
  older binary, which refuses a newer schema. So once instances have moved,
  rolling back means restoring the dump (see Backups). Release notes say
  when a release migrates.
- **SQLite.** The StatefulSet restarts one shard at a time. Each shard's
  instances are down while its pod restarts, and migrate when they start.
  `clax serve`'s copy of a database before a migration is kept in each
  home.

## Backups and restore

- **PostgreSQL.** Everything is in the `clax_fleet_*` tables. Back up with
  `pg_dump -t 'clax_fleet_*'`. To restore, scale the Deployment to 0 first
  (`kubectl scale deploy clax --replicas=0`, with the autoscaler's
  `minReplicas` raised from 0 back after). Then restore, and empty
  `clax_fleet_members` and `clax_fleet_leases`, so that no stale holder or
  lower epoch meets a live fence. Then scale up.
- **SQLite.** A volume snapshot of a live home is only crash-consistent.
  For a consistent copy of one instance, pause it (`paused = true`), copy
  its home out (`kubectl exec … tar`, as in Moving an instance), and
  unpause it. To restore, pause it, copy the home back, and unpause.

## Operating it

Each pod serves the instances on port 7480, and `/healthz` (alive),
`/readyz` (taking requests), `/status` and `/metrics` on 7481. `/status` is
JSON: the pod, its version and commit, the backend, its shard, the catalog
in effect and its reload state, and the instances running here. `/metrics`
is Prometheus text: requests answered here, forwarded and refused, forward
errors, instance starts and failures, lease losses and renewal failures,
handovers, catalog reload failures, and gauges of running instances and
readiness. An instance's own `/healthz` answers on 7480 at its origin, with
the version.

**Which pod runs which instance:** `kubectl exec deploy/clax --
clax-fleet status --backend postgres` lists every lease, its pod and epoch.
On SQLite, `kubectl exec clax-0 -- clax-fleet status --backend sqlite`
lists each shard's running instances.

**Logs** are text by default; set `CLAX_FLEET_LOG_FORMAT=json` for one JSON
object per line, with each request's `instance` in its span. `RUST_LOG`
sets the filter (`info`).

| Variable | Flag | Meaning |
|---|---|---|
| `CLAX_FLEET_BACKEND` | `--backend` | `sqlite` or `postgres` |
| `CLAX_FLEET_CATALOG` | `--catalog` | the catalog (`/etc/clax/fleet.toml`) |
| `CLAX_FLEET_DATA_DIR` | `--data-dir` | the homes go under `<dir>/instances` (`/var/lib/clax`) |
| `CLAX_FLEET_LISTEN` | `--listen` | the instances' address (`0.0.0.0:7480`) |
| `CLAX_FLEET_ADMIN_LISTEN` | `--admin-listen` | health, status and metrics (`0.0.0.0:7481`) |
| `POD_NAME` | `--pod` | this pod's name in the fleet |
| `POD_IP`, `CLAX_FLEET_ADVERTISE` | `--pod-ip`, `--advertise` | where other pods reach this one (`$POD_IP:<listen port>`) |
| `CLAX_FLEET_ORDINAL` | `--ordinal` | SQLite: this pod's shard (by default, its name's suffix) |
| `CLAX_FLEET_PEER` | `--peer` | SQLite: where shard `{shard}` is reached |
| `CLAX_FLEET_PEER_KEY` | `--peer-key` | the key forwards carry (on PostgreSQL, kept in the database when unset) |
| `CLAX_POSTGRES_URL` | `--postgres-url` | PostgreSQL: the connection string |
| `CLAX_POSTGRES_TLS`, `CLAX_POSTGRES_CA` | `--postgres-tls`, `--postgres-ca` | PostgreSQL: TLS (required when on), and extra CA certificates |
| `CLAX_POSTGRES_POOL` | `--postgres-pool` | PostgreSQL: data connections per pod (8) |
| `CLAX_FLEET_DB_CACHE_MIB` | `--db-cache-mib` | PostgreSQL: SQLite's page cache per instance (32) |
| `CLAX_FLEET_DB_MAX_TRANSACTION_MIB` | `--db-max-transaction-mib` | PostgreSQL: the most one transaction writes (256) |
| `CLAX_KEK` | | the key secrets are sealed under (64 hex digits; required on PostgreSQL) |
| `CLAX_FLEET_DRAIN_DELAY` | `--drain-delay-secs` | seconds to keep serving after SIGTERM (5) |
| `CLAX_FLEET_LOG_FORMAT` | `--log-format` | `text` or `json` |
| `RUST_LOG` | | the log filter (`info`) |

## Limits

- One instance's work is one pod's. The fleet scales the number of
  instances, not the throughput of one.
- An instance starts on its first request, so its first request after a
  move waits for its database to open (and to migrate, after an upgrade).
- On PostgreSQL an instance's database runs on one connection with jobs run
  inline (`Store::open_on`), not the reader pool of a home on disk. Reads
  hit SQLite's page cache, and a page cache miss costs a round trip.
- An instance that moves cuts its open event streams and WebSockets, and
  clients reconnect.
- Forwarded requests reach the owner from another pod. The owner sees that
  pod as the peer, so it counts by `CF-Connecting-IP` only behind
  Cloudflare (Security, above).

## Testing

`just test-affected` runs `clax-fleet`'s tests:

- the catalog, placement, and leases on an in-memory coordinator (with the
  coordinators' shared contract, and failing renewals);
- two-pod fleets in one process on both placements, including drains,
  handovers, paused instances, forged forwarding headers and shards
  without a home;
- the proxy (WebSockets, streaming, hop limits, the peer key);
- a check that the manifests set only variables the binary reads.

`clax-pagevfs`'s tests run the Clax store on the page VFS.

The PostgreSQL tests need a server. They cover the schema, the
coordinator's contract, the store on pages in PostgreSQL, fencing, the
content store contract, and an instance moving between two processes with
its data:

```
CLAX_TEST_PG_URL=postgresql://postgres@127.0.0.1:5432/clax_test just fleet-pg-test
```
