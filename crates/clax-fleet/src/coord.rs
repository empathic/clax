//! Who owns which instance on the PostgreSQL backend: the processes alive
//! (members) and each instance's lease, behind [`Coordinator`], kept in
//! PostgreSQL ([`PgCoordinator`]) or, for tests, in memory
//! ([`MemCoordinator`]).
//!
//! A member is seen alive while it has heartbeated within [`MEMBER_TTL`].
//! Its age runs from when it joined: its first heartbeat, one after a
//! lapse, or one from a new run of the process under the same name (a
//! container restarted in place keeps its pod's name, never its run ID).
//! A lease is live until it expires, [`LEASE_TTL`] after it was acquired or
//! last renewed; only its holder renews it, and each acquisition after the
//! lease has expired (or was released) raises its epoch. The epoch is the
//! fence every write of the instance checks ([`crate::pg::Fence`]). Times
//! are the database's (`now()`), never a process's clock.
//!
//! Where an instance goes when no live lease names it is decided by
//! rendezvous hashing over the live members ([`preferred`]): each member
//! scores the instance by a hash of both names, and the highest score
//! wins. Every process computes the same answer from the same members, and
//! when a member joins or leaves, only the instances it wins or held
//! move. Only members whose build can open the instance's data take part:
//! each lease row keeps the highest store schema (`user_version`) a holder
//! opened the instance at, and each member says which schema its build
//! migrates to. An instance a newer build migrated never goes to an older
//! one, which would refuse it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::pg::{PgError, Pool, Type, err};

/// How long a member is seen alive after its last heartbeat.
pub const MEMBER_TTL: Duration = Duration::from_secs(20);
/// How often a process heartbeats and renews its leases.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// How long a lease is live after it was acquired or last renewed.
pub const LEASE_TTL: Duration = Duration::from_secs(30);
/// How long a member must have been alive before instances move to it from
/// a live owner ([`crate::fleet`]'s rebalance), so a process that starts
/// and stops at once moves nothing.
pub const SETTLE: Duration = Duration::from_secs(30);

/// The store schema (`user_version`) this build migrates an instance's
/// data to, and the highest it opens.
pub const STORE_SCHEMA: u32 = clax_store::migrations::MIGRATIONS.len() as u32;

/// A process of the fleet, as the others see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Its name (the pod's).
    pub pod: String,
    /// Where the others reach it (`host:port`).
    pub addr: String,
    /// How long it has been alive.
    pub age: Duration,
    /// The store schema its build migrates an instance's data to.
    pub store_schema: u32,
}

/// An instance's live lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub holder: String,
    pub addr: String,
    pub epoch: i64,
}

/// The membership and the leases (see the [module docs](self)).
#[async_trait::async_trait]
pub trait Coordinator: Send + Sync {
    /// Records `pod` alive at `addr`, in its run `run`, with a build that
    /// migrates to `store_schema`: it joins (its age restarts) when it is
    /// not a member, or was one in another run.
    async fn heartbeat(
        &self,
        pod: &str,
        addr: &str,
        run: &str,
        store_schema: u32,
    ) -> Result<(), PgError>;
    /// Removes `pod` from the members.
    async fn leave(&self, pod: &str) -> Result<(), PgError>;
    /// The live members.
    async fn members(&self) -> Result<Vec<Member>, PgError>;
    /// `instance`'s lease, when a live one names it.
    async fn lease(&self, instance: &str) -> Result<Option<Lease>, PgError>;
    /// Takes `instance`'s lease for `pod` at `addr`, whose build opens the
    /// instance at `store_schema`, when no live lease names another
    /// process: the new epoch, or `None` when another holds it. The
    /// instance's store schema becomes `store_schema` when that is higher.
    async fn acquire(
        &self,
        instance: &str,
        pod: &str,
        addr: &str,
        store_schema: u32,
    ) -> Result<Option<i64>, PgError>;
    /// The highest store schema a holder of `instance`'s lease opened it at
    /// (0 when none ever held it): a build that migrates to less cannot
    /// open it.
    async fn store_schema(&self, instance: &str) -> Result<u32, PgError>;
    /// Extends each of `pod`'s leases in `holds` (instance and epoch) that
    /// it still holds and that has not expired (nor been released), in one
    /// call: the instances renewed.
    async fn renew_all(&self, pod: &str, holds: &[(String, i64)]) -> Result<Vec<String>, PgError>;
    /// Gives up `pod`'s lease of `instance` at `epoch`, when it still holds
    /// it.
    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError>;
    /// Gives up every lease that names `pod`: what a process does as it
    /// starts, so leases its previous run under the same name held are
    /// free at once rather than when they expire.
    async fn release_all(&self, pod: &str) -> Result<(), PgError>;

    /// The PostgreSQL pool the coordinator runs on, for its metrics.
    fn pool(&self) -> Option<&Arc<Pool>> {
        None
    }

    /// Extends `pod`'s lease of `instance` at `epoch`: false when it no
    /// longer holds it.
    async fn renew(&self, instance: &str, pod: &str, epoch: i64) -> Result<bool, PgError> {
        let renewed = self
            .renew_all(pod, &[(instance.to_string(), epoch)])
            .await?;
        Ok(!renewed.is_empty())
    }
}

fn score(instance: &str, pod: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(instance.as_bytes());
    h.update([0]);
    h.update(pod.as_bytes());
    h.finalize().into()
}

/// The member `instance`, whose data is at `store_schema`, goes to when no
/// live lease names it: the highest rendezvous score among the `members`
/// whose build migrates to at least `store_schema` (ties, which need equal
/// hashes, go to the lesser name). `None` when no member can open it.
pub fn preferred<'a>(
    instance: &str,
    store_schema: u32,
    members: &'a [Member],
) -> Option<&'a Member> {
    members
        .iter()
        .filter(|m| m.store_schema >= store_schema)
        .max_by(|a, b| {
            score(instance, &a.pod)
                .cmp(&score(instance, &b.pod))
                .then_with(|| b.pod.cmp(&a.pod))
        })
}

/// [`Coordinator`] over the fleet's tables (`crate::pg`), on a pool of its
/// own so its calls never wait behind an instance's page and content I/O.
pub struct PgCoordinator {
    pool: Arc<Pool>,
}

impl PgCoordinator {
    pub fn new(pool: Arc<Pool>) -> PgCoordinator {
        PgCoordinator { pool }
    }
}

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

/// A store schema as PostgreSQL's `integer`.
fn int4(schema: u32) -> i32 {
    i32::try_from(schema).unwrap_or(i32::MAX)
}

#[async_trait::async_trait]
impl Coordinator for PgCoordinator {
    async fn heartbeat(
        &self,
        pod: &str,
        addr: &str,
        run: &str,
        store_schema: u32,
    ) -> Result<(), PgError> {
        let (pod, addr, run) = (pod.to_string(), addr.to_string(), run.to_string());
        let store_schema = int4(store_schema);
        self.pool
            .call(move |c| {
                // A member that was seen dead, or heartbeats from another
                // run, joins again: its age restarts.
                c.execute_typed(
                    "INSERT INTO clax_fleet_members (pod, addr, run, store_schema, joined, seen)
                     VALUES ($1, $2, $3, $4, now(), now())
                     ON CONFLICT (pod) DO UPDATE SET
                       addr = EXCLUDED.addr,
                       store_schema = EXCLUDED.store_schema,
                       seen = now(),
                       joined = CASE
                         WHEN clax_fleet_members.seen < now() - make_interval(secs => $5)
                           OR clax_fleet_members.run <> EXCLUDED.run
                         THEN now() ELSE clax_fleet_members.joined END,
                       run = EXCLUDED.run",
                    &[
                        (&pod, Type::TEXT),
                        (&addr, Type::TEXT),
                        (&run, Type::TEXT),
                        (&store_schema, Type::INT4),
                        (&secs(MEMBER_TTL), Type::FLOAT8),
                    ],
                )
                .map_err(err)?;
                Ok(())
            })
            .await
    }

    async fn leave(&self, pod: &str) -> Result<(), PgError> {
        let pod = pod.to_string();
        self.pool
            .call(move |c| {
                c.execute_typed(
                    "DELETE FROM clax_fleet_members WHERE pod = $1",
                    &[(&pod, Type::TEXT)],
                )
                .map_err(err)?;
                Ok(())
            })
            .await
    }

    async fn members(&self) -> Result<Vec<Member>, PgError> {
        self.pool
            .call(|c| {
                let rows = c
                    .query_typed(
                        "SELECT pod, addr, extract(epoch FROM now() - joined)::float8, store_schema
                         FROM clax_fleet_members
                         WHERE seen >= now() - make_interval(secs => $1)
                         ORDER BY pod",
                        &[(&secs(MEMBER_TTL), Type::FLOAT8)],
                    )
                    .map_err(err)?;
                Ok(rows
                    .into_iter()
                    .map(|r| Member {
                        pod: r.get(0),
                        addr: r.get(1),
                        age: Duration::from_secs_f64(r.get::<_, f64>(2).max(0.0)),
                        store_schema: u32::try_from(r.get::<_, i32>(3)).unwrap_or(0),
                    })
                    .collect())
            })
            .await
    }

    async fn lease(&self, instance: &str) -> Result<Option<Lease>, PgError> {
        let instance = instance.to_string();
        self.pool
            .call(move |c| {
                let row = c
                    .query_typed_opt(
                        "SELECT holder, addr, epoch FROM clax_fleet_leases
                         WHERE instance = $1 AND expires > now()",
                        &[(&instance, Type::TEXT)],
                    )
                    .map_err(err)?;
                Ok(row.map(|r| Lease {
                    holder: r.get(0),
                    addr: r.get(1),
                    epoch: r.get(2),
                }))
            })
            .await
    }

    async fn acquire(
        &self,
        instance: &str,
        pod: &str,
        addr: &str,
        store_schema: u32,
    ) -> Result<Option<i64>, PgError> {
        let (instance, pod, addr) = (instance.to_string(), pod.to_string(), addr.to_string());
        let store_schema = int4(store_schema);
        self.pool
            .call(move |c| {
                // Taken when free or expired; a live lease of this pod's is
                // taken again at a new epoch too (a restart that kept its
                // name), so nothing of the earlier hold writes on. The
                // fence row moves with it, in the same statement: that
                // waits for every write the earlier holder had begun
                // (`crate::pg::Fence::check`).
                let row = c
                    .query_typed_opt(
                        "WITH lease AS (
                           INSERT INTO clax_fleet_leases
                             (instance, holder, addr, epoch, expires, store_schema)
                           VALUES ($1, $2, $3, 1, now() + make_interval(secs => $4), $5)
                           ON CONFLICT (instance) DO UPDATE SET
                             holder = EXCLUDED.holder,
                             addr = EXCLUDED.addr,
                             epoch = clax_fleet_leases.epoch + 1,
                             expires = EXCLUDED.expires,
                             store_schema = GREATEST(clax_fleet_leases.store_schema, EXCLUDED.store_schema)
                           WHERE clax_fleet_leases.expires <= now()
                              OR clax_fleet_leases.holder = EXCLUDED.holder
                           RETURNING instance, holder, epoch
                         )
                         INSERT INTO clax_fleet_fences (instance, holder, epoch)
                         SELECT instance, holder, epoch FROM lease
                         ON CONFLICT (instance) DO UPDATE SET
                           holder = EXCLUDED.holder,
                           epoch = EXCLUDED.epoch
                         RETURNING epoch",
                        &[
                            (&instance, Type::TEXT),
                            (&pod, Type::TEXT),
                            (&addr, Type::TEXT),
                            (&secs(LEASE_TTL), Type::FLOAT8),
                            (&store_schema, Type::INT4),
                        ],
                    )
                    .map_err(err)?;
                Ok(row.map(|r| r.get(0)))
            })
            .await
    }

    async fn store_schema(&self, instance: &str) -> Result<u32, PgError> {
        let instance = instance.to_string();
        self.pool
            .call(move |c| {
                let row = c
                    .query_typed_opt(
                        "SELECT store_schema FROM clax_fleet_leases WHERE instance = $1",
                        &[(&instance, Type::TEXT)],
                    )
                    .map_err(err)?;
                Ok(row.map_or(0, |r| u32::try_from(r.get::<_, i32>(0)).unwrap_or(0)))
            })
            .await
    }

    async fn renew_all(&self, pod: &str, holds: &[(String, i64)]) -> Result<Vec<String>, PgError> {
        if holds.is_empty() {
            return Ok(Vec::new());
        }
        let pod = pod.to_string();
        let (instances, epochs): (Vec<String>, Vec<i64>) = holds.iter().cloned().unzip();
        self.pool
            .call(move |c| {
                let rows = c
                    .query_typed(
                        "UPDATE clax_fleet_leases l
                         SET expires = now() + make_interval(secs => $4)
                         FROM unnest($2::text[], $3::bigint[]) AS h(instance, epoch)
                         WHERE l.instance = h.instance AND l.epoch = h.epoch AND l.holder = $1
                           AND l.expires > now()
                         RETURNING l.instance",
                        &[
                            (&pod, Type::TEXT),
                            (&instances, Type::TEXT_ARRAY),
                            (&epochs, Type::INT8_ARRAY),
                            (&secs(LEASE_TTL), Type::FLOAT8),
                        ],
                    )
                    .map_err(err)?;
                Ok(rows.into_iter().map(|r| r.get(0)).collect())
            })
            .await
    }

    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError> {
        let (instance, pod) = (instance.to_string(), pod.to_string());
        self.pool
            .call(move |c| {
                // Expired, not deleted: the epoch keeps rising.
                c.execute_typed(
                    "UPDATE clax_fleet_leases SET expires = now()
                     WHERE instance = $1 AND holder = $2 AND epoch = $3",
                    &[
                        (&instance, Type::TEXT),
                        (&pod, Type::TEXT),
                        (&epoch, Type::INT8),
                    ],
                )
                .map_err(err)?;
                Ok(())
            })
            .await
    }

    async fn release_all(&self, pod: &str) -> Result<(), PgError> {
        let pod = pod.to_string();
        self.pool
            .call(move |c| {
                c.execute_typed(
                    "UPDATE clax_fleet_leases SET expires = now()
                     WHERE holder = $1 AND expires > now()",
                    &[(&pod, Type::TEXT)],
                )
                .map_err(err)?;
                Ok(())
            })
            .await
    }

    fn pool(&self) -> Option<&Arc<Pool>> {
        Some(&self.pool)
    }
}

/// [`Coordinator`] in memory, on a clock the test moves: what several
/// processes of a test share in place of PostgreSQL. [`MemCoordinator::fail`]
/// makes every call fail, as a database out of reach does.
#[derive(Default)]
pub struct MemCoordinator {
    inner: Mutex<MemState>,
    failing: AtomicBool,
}

#[derive(Default)]
struct MemState {
    now: Duration,
    members: HashMap<String, MemMember>,
    /// Instance → (lease, expires).
    leases: HashMap<String, (Lease, Duration)>,
    /// Instance → its store schema.
    schemas: HashMap<String, u32>,
}

struct MemMember {
    addr: String,
    run: String,
    store_schema: u32,
    joined: Duration,
    seen: Duration,
}

impl MemCoordinator {
    pub fn new() -> Arc<MemCoordinator> {
        Arc::new(MemCoordinator::default())
    }

    /// Moves the clock on by `d`.
    pub fn advance(&self, d: Duration) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).now += d;
    }

    /// Makes every call fail (`true`) or answer again (`false`).
    pub fn fail(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, MemState>, PgError> {
        if self.failing.load(Ordering::SeqCst) {
            return Err("the coordinator is out of reach (MemCoordinator::fail)".into());
        }
        Ok(self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[async_trait::async_trait]
impl Coordinator for MemCoordinator {
    async fn heartbeat(
        &self,
        pod: &str,
        addr: &str,
        run: &str,
        store_schema: u32,
    ) -> Result<(), PgError> {
        let mut s = self.state()?;
        let now = s.now;
        let joined = match s.members.get(pod) {
            Some(m) if now.saturating_sub(m.seen) <= MEMBER_TTL && m.run == run => m.joined,
            _ => now,
        };
        s.members.insert(
            pod.to_string(),
            MemMember {
                addr: addr.to_string(),
                run: run.to_string(),
                store_schema,
                joined,
                seen: now,
            },
        );
        Ok(())
    }

    async fn leave(&self, pod: &str) -> Result<(), PgError> {
        self.state()?.members.remove(pod);
        Ok(())
    }

    async fn members(&self) -> Result<Vec<Member>, PgError> {
        let s = self.state()?;
        let mut m: Vec<Member> = s
            .members
            .iter()
            .filter(|(_, m)| s.now.saturating_sub(m.seen) <= MEMBER_TTL)
            .map(|(pod, m)| Member {
                pod: pod.clone(),
                addr: m.addr.clone(),
                age: s.now.saturating_sub(m.joined),
                store_schema: m.store_schema,
            })
            .collect();
        m.sort_by(|a, b| a.pod.cmp(&b.pod));
        Ok(m)
    }

    async fn lease(&self, instance: &str) -> Result<Option<Lease>, PgError> {
        let s = self.state()?;
        Ok(s.leases
            .get(instance)
            .filter(|(_, expires)| *expires > s.now)
            .map(|(l, _)| l.clone()))
    }

    async fn acquire(
        &self,
        instance: &str,
        pod: &str,
        addr: &str,
        store_schema: u32,
    ) -> Result<Option<i64>, PgError> {
        let mut s = self.state()?;
        let now = s.now;
        let epoch = match s.leases.get(instance) {
            Some((l, expires)) if *expires > now && l.holder != pod => return Ok(None),
            Some((l, _)) => l.epoch + 1,
            None => 1,
        };
        s.leases.insert(
            instance.to_string(),
            (
                Lease {
                    holder: pod.to_string(),
                    addr: addr.to_string(),
                    epoch,
                },
                now + LEASE_TTL,
            ),
        );
        let schema = s.schemas.entry(instance.to_string()).or_default();
        *schema = (*schema).max(store_schema);
        Ok(Some(epoch))
    }

    async fn store_schema(&self, instance: &str) -> Result<u32, PgError> {
        Ok(self.state()?.schemas.get(instance).copied().unwrap_or(0))
    }

    async fn renew_all(&self, pod: &str, holds: &[(String, i64)]) -> Result<Vec<String>, PgError> {
        let mut s = self.state()?;
        let now = s.now;
        let mut renewed = Vec::new();
        for (instance, epoch) in holds {
            if let Some((l, expires)) = s.leases.get_mut(instance)
                && l.holder == pod
                && l.epoch == *epoch
                && *expires > now
            {
                *expires = now + LEASE_TTL;
                renewed.push(instance.clone());
            }
        }
        Ok(renewed)
    }

    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError> {
        let mut s = self.state()?;
        let now = s.now;
        if let Some((l, expires)) = s.leases.get_mut(instance)
            && l.holder == pod
            && l.epoch == epoch
            && *expires > now
        {
            *expires = now;
        }
        Ok(())
    }

    async fn release_all(&self, pod: &str) -> Result<(), PgError> {
        let mut s = self.state()?;
        let now = s.now;
        for (l, expires) in s.leases.values_mut() {
            if l.holder == pod && *expires > now {
                *expires = now;
            }
        }
        Ok(())
    }
}

/// [`MemCoordinator`]'s clock moves for everything at once.
#[async_trait::async_trait]
impl contract::Time for MemCoordinator {
    async fn pass(&self, d: Duration, _instances: &[&str], _pods: &[&str]) {
        self.advance(d);
    }
}

/// The rules every [`Coordinator`] keeps: [`run`](contract::run) checks the
/// ones that need no clock, and [`expiry`](contract::expiry) the ones about
/// time, which it makes pass through [`Time`](contract::Time). Each panics
/// on the first rule it finds broken. Each check names its instances and
/// pods with `unique`, so runs against one database never meet.
pub mod contract {
    use std::time::Duration;

    use super::{Coordinator, LEASE_TTL, MEMBER_TTL, Member, SETTLE};

    /// Time passing for a coordinator under test.
    #[async_trait::async_trait]
    pub trait Time: Send + Sync {
        /// Makes `d` pass for the leases of `instances` and the members
        /// `pods` (a clock that moves for everything moves for them too).
        async fn pass(&self, d: Duration, instances: &[&str], pods: &[&str]);
    }

    async fn member(c: &dyn Coordinator, pod: &str) -> Option<Member> {
        c.members()
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.pod == pod)
    }

    pub async fn run(c: &dyn Coordinator, unique: &dyn Fn(&str) -> String) {
        let inst = unique("inst");
        let (a, b) = (unique("pod-a"), unique("pod-b"));
        // One holder at a time; each new hold raises the epoch.
        assert_eq!(c.acquire(&inst, &a, "a:1", 1).await.unwrap(), Some(1));
        assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), None);
        let l = c.lease(&inst).await.unwrap().unwrap();
        assert_eq!(
            (l.holder.as_str(), l.addr.as_str(), l.epoch),
            (a.as_str(), "a:1", 1)
        );
        // Only the holder renews, at its own epoch.
        assert!(c.renew(&inst, &a, 1).await.unwrap());
        assert!(!c.renew(&inst, &b, 1).await.unwrap());
        assert!(!c.renew(&inst, &a, 2).await.unwrap());
        // Taking it again while holding it is a new hold.
        assert_eq!(c.acquire(&inst, &a, "a:1", 1).await.unwrap(), Some(2));
        assert!(!c.renew(&inst, &a, 1).await.unwrap());
        // Released, it is anyone's, at a higher epoch, and its holder's
        // renewal (one that raced the release) does not bring it back.
        c.release(&inst, &a, 2).await.unwrap();
        assert!(c.lease(&inst).await.unwrap().is_none());
        assert!(!c.renew(&inst, &a, 2).await.unwrap());
        assert!(c.lease(&inst).await.unwrap().is_none());
        assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), Some(3));
        // A release by a past holder does nothing.
        c.release(&inst, &a, 2).await.unwrap();
        assert_eq!(c.lease(&inst).await.unwrap().unwrap().holder, b);
        // Renewing several answers the ones still held.
        let other = unique("inst");
        assert_eq!(c.acquire(&other, &b, "b:1", 1).await.unwrap(), Some(1));
        let mut renewed = c
            .renew_all(
                &b,
                &[(inst.clone(), 3), (other.clone(), 1), (unique("none"), 1)],
            )
            .await
            .unwrap();
        renewed.sort();
        let mut want = vec![inst.clone(), other.clone()];
        want.sort();
        assert_eq!(renewed, want);
        // Releasing all of a pod's frees each, and leaves the others'; none
        // of the released renews again.
        let kept = unique("inst");
        assert_eq!(c.acquire(&kept, &a, "a:1", 1).await.unwrap(), Some(1));
        c.release_all(&b).await.unwrap();
        assert!(c.lease(&inst).await.unwrap().is_none());
        assert!(c.lease(&other).await.unwrap().is_none());
        assert_eq!(c.lease(&kept).await.unwrap().unwrap().holder, a);
        let renewed = c
            .renew_all(&b, &[(inst.clone(), 3), (other.clone(), 1)])
            .await
            .unwrap();
        assert!(renewed.is_empty(), "{renewed:?}");
        // An instance's store schema is the highest its holders opened it
        // at, and 0 for one never held.
        assert_eq!(c.store_schema(&unique("none")).await.unwrap(), 0);
        let migrated = unique("inst");
        assert_eq!(c.acquire(&migrated, &a, "a:1", 7).await.unwrap(), Some(1));
        assert_eq!(c.store_schema(&migrated).await.unwrap(), 7);
        c.release(&migrated, &a, 1).await.unwrap();
        assert_eq!(c.acquire(&migrated, &b, "b:1", 6).await.unwrap(), Some(2));
        assert_eq!(c.store_schema(&migrated).await.unwrap(), 7);
        // Members: a heartbeat joins and moves the address and the store
        // schema; leave removes.
        c.heartbeat(&a, "a:1", "run-1", 6).await.unwrap();
        c.heartbeat(&a, "a:2", "run-1", 7).await.unwrap();
        let m = member(c, &a).await.unwrap();
        assert_eq!((m.addr.as_str(), m.store_schema), ("a:2", 7));
        c.leave(&a).await.unwrap();
        assert!(member(c, &a).await.is_none());
    }

    pub async fn expiry(c: &dyn Coordinator, time: &dyn Time, unique: &dyn Fn(&str) -> String) {
        let inst = unique("inst");
        let (a, b) = (unique("pod-a"), unique("pod-b"));
        // A lease lives LEASE_TTL from its last renewal.
        assert_eq!(c.acquire(&inst, &a, "a:1", 1).await.unwrap(), Some(1));
        time.pass(LEASE_TTL / 2, &[&inst], &[]).await;
        assert!(c.renew(&inst, &a, 1).await.unwrap());
        time.pass(LEASE_TTL / 2 + Duration::from_secs(1), &[&inst], &[])
            .await;
        assert_eq!(c.lease(&inst).await.unwrap().unwrap().holder, a);
        assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), None);
        // Expired, it is no one's: its holder cannot renew it, and anyone
        // takes it at a higher epoch.
        time.pass(LEASE_TTL, &[&inst], &[]).await;
        assert!(c.lease(&inst).await.unwrap().is_none());
        assert!(
            !c.renew(&inst, &a, 1).await.unwrap(),
            "an expired lease renewed"
        );
        assert_eq!(c.acquire(&inst, &b, "b:1", 1).await.unwrap(), Some(2));
        assert!(
            !c.renew(&inst, &a, 1).await.unwrap(),
            "a's old hold renewed"
        );
        // A past holder's release_all leaves the lease its instance has
        // since: one that expired and another took.
        time.pass(LEASE_TTL + Duration::from_secs(1), &[&inst], &[])
            .await;
        assert_eq!(c.acquire(&inst, &a, "a:1", 1).await.unwrap(), Some(3));
        c.release_all(&b).await.unwrap();
        assert_eq!(c.lease(&inst).await.unwrap().unwrap().holder, a);

        // A member's age runs from when it joined, while it heartbeats
        // within MEMBER_TTL.
        let step = MEMBER_TTL - Duration::from_secs(5);
        c.heartbeat(&a, "a:1", "run-1", 1).await.unwrap();
        for _ in 0..4 {
            time.pass(step, &[], &[&a]).await;
            c.heartbeat(&a, "a:1", "run-1", 1).await.unwrap();
        }
        let settled = member(c, &a).await.unwrap().age;
        assert!(settled >= step * 4, "{settled:?}");
        // A new run under the same name (a container restarted in place)
        // joins again: it has not settled.
        c.heartbeat(&a, "a:1", "run-2", 1).await.unwrap();
        let age = member(c, &a).await.unwrap().age;
        assert!(age < SETTLE, "a new run kept the old one's age: {age:?}");
        for _ in 0..4 {
            time.pass(step, &[], &[&a]).await;
            c.heartbeat(&a, "a:1", "run-2", 1).await.unwrap();
        }
        // Not seen for MEMBER_TTL, it is out; its next heartbeat joins it
        // again.
        time.pass(MEMBER_TTL + Duration::from_secs(1), &[], &[&a])
            .await;
        assert!(member(c, &a).await.is_none(), "a lapsed member is live");
        c.heartbeat(&a, "a:1", "run-2", 1).await.unwrap();
        let age = member(c, &a).await.unwrap().age;
        assert!(age < SETTLE, "a lapsed member kept its age: {age:?}");
        c.leave(&a).await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(pod: &str, store_schema: u32) -> Member {
        Member {
            pod: pod.into(),
            addr: format!("{pod}:7480"),
            age: Duration::ZERO,
            store_schema,
        }
    }

    /// Adding a member moves only the instances it now wins, and removing
    /// one moves only the instances it had.
    #[test]
    fn rendezvous_hashing_moves_only_what_must_move() {
        let three: Vec<Member> = ["a", "b", "c"].map(|p| member(p, 1)).to_vec();
        let four: Vec<Member> = ["a", "b", "c", "d"].map(|p| member(p, 1)).to_vec();
        let names: Vec<String> = (0..400).map(|i| format!("i{i}")).collect();
        let mut moved = 0;
        let mut per = HashMap::<String, usize>::new();
        for n in &names {
            let before = preferred(n, 1, &three).unwrap();
            let after = preferred(n, 1, &four).unwrap();
            *per.entry(after.pod.clone()).or_default() += 1;
            if before != after {
                assert_eq!(after.pod, "d", "{n} moved between old members");
                moved += 1;
            }
        }
        assert!((50..150).contains(&moved), "{moved} of 400 moved to d");
        for pod in ["a", "b", "c", "d"] {
            assert!(per[pod] > 50, "{pod} has {} of 400", per[pod]);
        }
        assert!(preferred("x", 0, &[]).is_none());
    }

    /// An instance goes only to a member whose build can open its data:
    /// among those, hashing decides as among all.
    #[test]
    fn an_instance_goes_only_to_a_member_that_can_open_its_data() {
        let mixed = vec![member("a", 6), member("b", 7), member("c", 7)];
        let new: Vec<Member> = mixed[1..].to_vec();
        for i in 0..200 {
            let n = format!("i{i}");
            assert_eq!(preferred(&n, 7, &mixed), preferred(&n, 7, &new), "{n}");
            assert!(preferred(&n, 0, &mixed).is_some());
        }
        assert!(preferred("x", 8, &mixed).is_none());
    }

    #[tokio::test]
    async fn the_memory_coordinator_keeps_the_contract() {
        let n = std::sync::atomic::AtomicU32::new(0);
        let unique = |p: &str| format!("{p}-{}", n.fetch_add(1, Ordering::SeqCst));
        let c = MemCoordinator::new();
        contract::run(&*c, &unique).await;
        contract::expiry(&*c, &*c, &unique).await;
    }

    #[tokio::test]
    async fn a_failing_coordinator_fails_every_call_until_it_answers_again() {
        let c = MemCoordinator::new();
        c.fail(true);
        assert!(c.heartbeat("a", "a:1", "r", 1).await.is_err());
        assert!(c.acquire("x", "a", "a:1", 1).await.is_err());
        c.fail(false);
        assert_eq!(c.acquire("x", "a", "a:1", 1).await.unwrap(), Some(1));
    }
}
