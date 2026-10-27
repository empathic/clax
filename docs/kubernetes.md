# Clax on Kubernetes

`clax-fleet` serves hosted Clax instances from a set of pods, backed by
SQLite or by PostgreSQL, and scales by adding pods. `deploy/k8s` holds the
manifests (kustomize), `deploy/Dockerfile` builds the image, and
`crates/clax-fleet` is the server (its crate docs cover the design in
depth).

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
| Needs | A ReadWriteOnce volume per pod | A PostgreSQL 13 or newer, and `CLAX_KEK` |

SQLite is the simpler choice and keeps each instance's I/O local. With
PostgreSQL, pods are interchangeable and nothing on them has to survive.

## The image

```
docker build -f deploy/Dockerfile -t <registry>/clax-fleet:<tag> .
```

This builds the web UI with the pinned Node and pnpm, then the release
`clax-fleet` with the pinned Rust, then a slim Debian runtime that runs as
UID 10001. Before applying, point the manifests' `image:` at your tag (with
`kustomize edit set image ghcr.io/empathic/clax-fleet=<registry>/clax-fleet:<tag>`
in an overlay of your own).

The image runs `clax-fleet serve`. `clax-fleet check-catalog <file>
[--shards N]` validates a catalog the way `serve` reads it.

## The catalog

`deploy/k8s/base/fleet.toml` becomes the `clax-fleet-catalog` ConfigMap,
mounted at `/etc/clax/fleet.toml`:

```toml
[[instance]]
name = "acme"            # lowercase letters, digits and inner dashes
shard = 0                # SQLite: the pod (clax-0) whose volume keeps it
default = true           # answers hosts no instance names
config = '''
[hosted]
public_origin = "https://acme.clax.example.com"
site_domain = "acme-sites.example.com"
setup_secret_env = "CLAX_SETUP_SECRET_ACME"
'''
```

`config` is the instance's `config.toml`. It must have a `[hosted]` table:
a fleet only serves hosted instances, which sign people in through the
browser. No two instances may share an origin or a site domain.

Every pod reloads the catalog every 5 s. When an instance is added it
starts on its first request. When one is removed, or its `config` or shard
changes, it stops, and its next request starts it again with the new
entry. A catalog that does not parse leaves the previous one in effect,
with a warning in the log. Kubernetes takes up to a minute to update a
mounted ConfigMap.

## Deploying on SQLite

```
kubectl create secret generic clax-secrets \
  --from-literal=CLAX_SETUP_SECRET_EXAMPLE="$(openssl rand -hex 16)"
kubectl apply -k deploy/k8s/sqlite
```

The StatefulSet `clax` runs pods `clax-0`, `clax-1`, …, each with a 10 Gi
volume at `/var/lib/clax`. The headless Service `clax-peers` names each
one, and `CLAX_FLEET_PEER` (`clax-{shard}.clax-peers:7480`) tells pods
where to forward requests for each shard. `CLAX_FLEET_SHARDS` must equal
the replica count: a pod whose ordinal is past it refuses to start, and
the catalog may name only shards below it.

**Scaling out:** raise `replicas` and `CLAX_FLEET_SHARDS` together, then
give new instances the new shards.

**Moving an instance** to another shard means moving its home with it:

1. Remove the instance from the catalog and wait for its pod to log
   `instance stopped`.
2. Copy its home between the pods:
   `kubectl exec clax-0 -- tar -C /var/lib/clax/instances -cf - acme | kubectl exec -i clax-1 -- tar -C /var/lib/clax/instances -xf -`
3. Add it back with its new `shard`.

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
pod with a fixed password, so it is not for production.

`CLAX_KEK` is required here. Instances move between pods, so every pod must
be able to open the secrets (ClaxBot's model keys) that the others sealed.
Keep it safe: without it, those secrets cannot be read. For TLS to the
database, set `CLAX_POSTGRES_TLS=on`, and `CLAX_POSTGRES_CA` to a PEM file
of the database's CA if the public roots don't cover it (mount it from a
Secret).

On first start, `clax-fleet` creates its tables (`clax_fleet_*`) under an
advisory lock. Every pod then heartbeats into `clax_fleet_members` every
5 s, and a pod not seen for 20 s is out. A request for an instance goes to
the pod that holds its lease. When no live lease names the instance, it
goes to the live pod that rendezvous hashing prefers, and that pod takes
the lease (raising its epoch) and starts the instance. A lease lasts 30 s
and its holder renews it every 5 s.

**Fencing.** Every write an instance makes is fenced by its lease. Each
page commit, content write and app data write starts by locking the lease
row and checking that it still names this pod at the epoch it acquired. A
pod that lost an instance (paused past its lease, or cut off from the
database) therefore can never write to it again, even before it notices.
It stops the instance at its next renewal.

**Scaling.** The autoscaler (2 to 10 pods, at 70% CPU) adds pods. Once a
new pod has been up 30 s, each instance that rendezvous hashing now
prefers for it is handed over: its old pod stops it and releases the
lease, and its next request starts it on the new pod. Scale-down is slowed
to one pod every two minutes, because each move cuts that instance's open
streams, which reconnect.

**Draining.** On SIGTERM a pod answers `/readyz` with 503 at once, and
keeps serving for `CLAX_FLEET_DRAIN_DELAY` (5 s) while the Service drops
it. It then stops every instance and releases their leases (the other pods
take them on their next request), and ends the open connections within
10 s. `terminationGracePeriodSeconds` is 45.

**Backups.** Everything is in the `clax_fleet_*` tables:
`pg_dump -t 'clax_fleet_*'` backs up the whole fleet. `clax serve` copies a
home's database before a migration; on PostgreSQL there is no such copy,
so take a dump before upgrading to a release whose instances migrate.

## Ingress, DNS and TLS

`deploy/k8s/examples/ingress.yaml` routes an instance's origin and the
wildcard for its site domain to the `clax` Service. Each site domain needs
a wildcard DNS record and a wildcard certificate (cert-manager with a
DNS-01 issuer). Raise the controller's read and send timeouts for event
streams and WebSockets (the example's are ingress-nginx's), and its body
limit to above the 16 MB that a publish may send.

Hosted mode's abuse limits count requests by client address. Behind
Cloudflare, set `[hosted] behind_cloudflare = true`, and the address in
`CF-Connecting-IP` counts. Otherwise the peer address counts, and behind an
Ingress that is the controller's address for every client.

## Operating it

Each pod serves the instances on port 7480, and `/healthz` (alive),
`/readyz` (taking requests) and `/status` on 7481. `/status` is JSON
giving the pod, the catalog's instances and the ones running here. An
instance's own `/healthz` answers on 7480 at its origin.

| Variable | Flag | Meaning |
|---|---|---|
| `CLAX_FLEET_BACKEND` | `--backend` | `sqlite` or `postgres` |
| `CLAX_FLEET_CATALOG` | `--catalog` | the catalog (`/etc/clax/fleet.toml`) |
| `CLAX_FLEET_DATA_DIR` | `--data-dir` | the homes go under `<dir>/instances` (`/var/lib/clax`) |
| `CLAX_FLEET_LISTEN` | `--listen` | the instances' address (`0.0.0.0:7480`) |
| `CLAX_FLEET_ADMIN_LISTEN` | `--admin-listen` | health and status (`0.0.0.0:7481`) |
| `POD_NAME` | `--pod` | this pod's name in the fleet |
| `POD_IP`, `CLAX_FLEET_ADVERTISE` | `--pod-ip`, `--advertise` | where other pods reach this one (`$POD_IP:<listen port>`) |
| `CLAX_FLEET_SHARDS` | `--shards` | SQLite: the number of shards |
| `CLAX_FLEET_ORDINAL` | `--ordinal` | SQLite: this pod's shard (by default, its name's suffix) |
| `CLAX_FLEET_PEER` | `--peer` | SQLite: where shard `{shard}` is reached |
| `CLAX_POSTGRES_URL` | `--postgres-url` | PostgreSQL: the connection string |
| `CLAX_POSTGRES_TLS`, `CLAX_POSTGRES_CA` | `--postgres-tls`, `--postgres-ca` | PostgreSQL: TLS, and extra CA certificates |
| `CLAX_POSTGRES_POOL` | `--postgres-pool` | PostgreSQL: connections per pod (16) |
| `CLAX_KEK` | | the key secrets are sealed under (64 hex digits; required on PostgreSQL) |
| `CLAX_FLEET_DRAIN_DELAY` | `--drain-delay-secs` | seconds to keep serving after SIGTERM (5) |

## Limits

- One instance's work is one pod's. The fleet scales the number of
  instances, not the throughput of one.
- An instance starts on its first request: its first request after a move
  waits for its database to open (and to migrate, after an upgrade).
- On PostgreSQL an instance's database runs on one connection with jobs run
  inline (`Store::open_on`), not the reader pool of a home on disk. Reads
  hit SQLite's 64 MiB page cache, and a page cache miss costs a round trip
  to the database.
- An instance that moves cuts its open event streams and WebSockets, and
  clients reconnect.
- Forwarded requests reach the owner from another pod. The owner sees that
  pod as the peer, so it counts by `CF-Connecting-IP` only behind
  Cloudflare (above).

## Testing

`just test-affected` runs `clax-fleet`'s tests: the catalog, placement,
leases on an in-memory coordinator, two-pod fleets in one process on both
placements, the proxy (WebSockets, streaming, hop limits), and a check that
the manifests set only variables the binary reads. `clax-pagevfs`'s tests
run the Clax store on the page VFS.

The PostgreSQL tests (the schema, the coordinator, the store on pages in
PostgreSQL, fencing, and the content store contract) need a server:

```
CLAX_TEST_PG_URL=postgresql://postgres@127.0.0.1:5432/clax_test just fleet-pg-test
```
