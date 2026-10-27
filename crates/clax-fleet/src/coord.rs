//! Who owns which instance on the PostgreSQL backend: the processes alive
//! (members) and each instance's lease, behind [`Coordinator`], kept in
//! PostgreSQL ([`PgCoordinator`]) or, for tests, in memory
//! ([`MemCoordinator`]).
//!
//! A member is seen alive while it has heartbeated within [`MEMBER_TTL`].
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
//! move.

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

/// A process of the fleet, as the others see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Its name (the pod's).
    pub pod: String,
    /// Where the others reach it (`host:port`).
    pub addr: String,
    /// How long it has been alive.
    pub age: Duration,
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
    /// Records `pod` alive at `addr`, joining when it is not a member.
    async fn heartbeat(&self, pod: &str, addr: &str) -> Result<(), PgError>;
    /// Removes `pod` from the members.
    async fn leave(&self, pod: &str) -> Result<(), PgError>;
    /// The live members.
    async fn members(&self) -> Result<Vec<Member>, PgError>;
    /// `instance`'s lease, when a live one names it.
    async fn lease(&self, instance: &str) -> Result<Option<Lease>, PgError>;
    /// Takes `instance`'s lease for `pod` at `addr` when no live lease
    /// names another process: the new epoch, or `None` when another holds
    /// it.
    async fn acquire(&self, instance: &str, pod: &str, addr: &str) -> Result<Option<i64>, PgError>;
    /// Extends each of `pod`'s leases in `holds` (instance and epoch) that
    /// it still holds, in one call: the instances renewed.
    async fn renew_all(&self, pod: &str, holds: &[(String, i64)]) -> Result<Vec<String>, PgError>;
    /// Gives up `pod`'s lease of `instance` at `epoch`, when it still holds
    /// it.
    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError>;
    /// Gives up every lease that names `pod`: what a process does as it
    /// starts, so leases its previous run under the same name held are
    /// free at once rather than when they expire.
    async fn release_all(&self, pod: &str) -> Result<(), PgError>;

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

/// The member `instance` goes to when no live lease names it: the highest
/// rendezvous score among `members` (ties, which need equal hashes, go to
/// the lesser name).
pub fn preferred<'a>(instance: &str, members: &'a [Member]) -> Option<&'a Member> {
    members.iter().max_by(|a, b| {
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

#[async_trait::async_trait]
impl Coordinator for PgCoordinator {
    async fn heartbeat(&self, pod: &str, addr: &str) -> Result<(), PgError> {
        let (pod, addr) = (pod.to_string(), addr.to_string());
        self.pool
            .call(move |c| {
                // A member that was seen dead joins again: its age restarts.
                c.execute_typed(
                    "INSERT INTO clax_fleet_members (pod, addr, joined, seen)
                     VALUES ($1, $2, now(), now())
                     ON CONFLICT (pod) DO UPDATE SET
                       addr = EXCLUDED.addr,
                       seen = now(),
                       joined = CASE
                         WHEN clax_fleet_members.seen < now() - make_interval(secs => $3)
                         THEN now() ELSE clax_fleet_members.joined END",
                    &[
                        (&pod, Type::TEXT),
                        (&addr, Type::TEXT),
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
                        "SELECT pod, addr, extract(epoch FROM now() - joined)::float8
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

    async fn acquire(&self, instance: &str, pod: &str, addr: &str) -> Result<Option<i64>, PgError> {
        let (instance, pod, addr) = (instance.to_string(), pod.to_string(), addr.to_string());
        self.pool
            .call(move |c| {
                // Taken when free or expired; a live lease of this pod's is
                // taken again at a new epoch too (a restart that kept its
                // name), so nothing of the earlier hold writes on.
                let row = c
                    .query_typed_opt(
                        "INSERT INTO clax_fleet_leases (instance, holder, addr, epoch, expires)
                         VALUES ($1, $2, $3, 1, now() + make_interval(secs => $4))
                         ON CONFLICT (instance) DO UPDATE SET
                           holder = EXCLUDED.holder,
                           addr = EXCLUDED.addr,
                           epoch = clax_fleet_leases.epoch + 1,
                           expires = EXCLUDED.expires
                         WHERE clax_fleet_leases.expires <= now()
                            OR clax_fleet_leases.holder = EXCLUDED.holder
                         RETURNING epoch",
                        &[
                            (&instance, Type::TEXT),
                            (&pod, Type::TEXT),
                            (&addr, Type::TEXT),
                            (&secs(LEASE_TTL), Type::FLOAT8),
                        ],
                    )
                    .map_err(err)?;
                Ok(row.map(|r| r.get(0)))
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
    /// Pod → (address, joined, seen).
    members: HashMap<String, (String, Duration, Duration)>,
    /// Instance → (lease, expires).
    leases: HashMap<String, (Lease, Duration)>,
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
    async fn heartbeat(&self, pod: &str, addr: &str) -> Result<(), PgError> {
        let mut s = self.state()?;
        let now = s.now;
        let joined = match s.members.get(pod) {
            Some((_, joined, seen)) if now.saturating_sub(*seen) <= MEMBER_TTL => *joined,
            _ => now,
        };
        s.members
            .insert(pod.to_string(), (addr.to_string(), joined, now));
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
            .filter(|(_, (_, _, seen))| s.now.saturating_sub(*seen) <= MEMBER_TTL)
            .map(|(pod, (addr, joined, _))| Member {
                pod: pod.clone(),
                addr: addr.clone(),
                age: s.now.saturating_sub(*joined),
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

    async fn acquire(&self, instance: &str, pod: &str, addr: &str) -> Result<Option<i64>, PgError> {
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
        Ok(Some(epoch))
    }

    async fn renew_all(&self, pod: &str, holds: &[(String, i64)]) -> Result<Vec<String>, PgError> {
        let mut s = self.state()?;
        let now = s.now;
        let mut renewed = Vec::new();
        for (instance, epoch) in holds {
            if let Some((l, expires)) = s.leases.get_mut(instance)
                && l.holder == pod
                && l.epoch == *epoch
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

/// The rules every [`Coordinator`] keeps that need no clock: `run` panics on
/// the first it breaks. Each check names its instances and pods with
/// `unique`, so runs against one database never meet. The rules about
/// expiry are checked on [`MemCoordinator`], whose clock a test moves
/// (this module's tests).
pub mod contract {
    use super::Coordinator;

    pub async fn run(c: &dyn Coordinator, unique: &dyn Fn(&str) -> String) {
        let inst = unique("inst");
        let (a, b) = (unique("pod-a"), unique("pod-b"));
        // One holder at a time; each new hold raises the epoch.
        assert_eq!(c.acquire(&inst, &a, "a:1").await.unwrap(), Some(1));
        assert_eq!(c.acquire(&inst, &b, "b:1").await.unwrap(), None);
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
        assert_eq!(c.acquire(&inst, &a, "a:1").await.unwrap(), Some(2));
        assert!(!c.renew(&inst, &a, 1).await.unwrap());
        // Released, it is anyone's, at a higher epoch.
        c.release(&inst, &a, 2).await.unwrap();
        assert!(c.lease(&inst).await.unwrap().is_none());
        assert_eq!(c.acquire(&inst, &b, "b:1").await.unwrap(), Some(3));
        // A release by a past holder does nothing.
        c.release(&inst, &a, 2).await.unwrap();
        assert_eq!(c.lease(&inst).await.unwrap().unwrap().holder, b);
        // Renewing several answers the ones still held.
        let other = unique("inst");
        assert_eq!(c.acquire(&other, &b, "b:1").await.unwrap(), Some(1));
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
        // Releasing all of a pod's frees each, and leaves the others'.
        let kept = unique("inst");
        assert_eq!(c.acquire(&kept, &a, "a:1").await.unwrap(), Some(1));
        c.release_all(&b).await.unwrap();
        assert!(c.lease(&inst).await.unwrap().is_none());
        assert!(c.lease(&other).await.unwrap().is_none());
        assert_eq!(c.lease(&kept).await.unwrap().unwrap().holder, a);
        // Members: a heartbeat joins and moves the address; leave removes.
        c.heartbeat(&a, "a:1").await.unwrap();
        c.heartbeat(&a, "a:2").await.unwrap();
        let m = c.members().await.unwrap();
        assert_eq!(m.iter().find(|m| m.pod == a).unwrap().addr, "a:2");
        c.leave(&a).await.unwrap();
        assert!(!c.members().await.unwrap().iter().any(|m| m.pod == a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(pod: &str) -> Member {
        Member {
            pod: pod.into(),
            addr: format!("{pod}:7480"),
            age: Duration::ZERO,
        }
    }

    /// Adding a member moves only the instances it now wins, and removing
    /// one moves only the instances it had.
    #[test]
    fn rendezvous_hashing_moves_only_what_must_move() {
        let three: Vec<Member> = ["a", "b", "c"].map(member).to_vec();
        let four: Vec<Member> = ["a", "b", "c", "d"].map(member).to_vec();
        let names: Vec<String> = (0..400).map(|i| format!("i{i}")).collect();
        let mut moved = 0;
        let mut per = HashMap::<String, usize>::new();
        for n in &names {
            let before = preferred(n, &three).unwrap();
            let after = preferred(n, &four).unwrap();
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
        assert!(preferred("x", &[]).is_none());
    }

    #[tokio::test]
    async fn a_lease_is_held_until_it_expires_and_each_new_hold_raises_the_epoch() {
        let c = MemCoordinator::new();
        assert_eq!(c.acquire("acme", "a", "a:1").await.unwrap(), Some(1));
        assert_eq!(c.acquire("acme", "b", "b:1").await.unwrap(), None);
        c.advance(LEASE_TTL / 2);
        assert!(c.renew("acme", "a", 1).await.unwrap());
        c.advance(LEASE_TTL / 2 + Duration::from_secs(1));
        // Renewed: still a's.
        assert_eq!(c.lease("acme").await.unwrap().unwrap().holder, "a");
        c.advance(LEASE_TTL);
        assert!(c.lease("acme").await.unwrap().is_none());
        assert_eq!(c.acquire("acme", "b", "b:1").await.unwrap(), Some(2));
        assert!(
            !c.renew("acme", "a", 1).await.unwrap(),
            "a's old hold renewed"
        );
        c.release("acme", "b", 2).await.unwrap();
        assert!(c.lease("acme").await.unwrap().is_none());
        assert_eq!(c.acquire("acme", "a", "a:1").await.unwrap(), Some(3));
    }

    #[tokio::test]
    async fn a_member_is_alive_while_it_heartbeats_and_its_age_restarts_after_a_lapse() {
        let c = MemCoordinator::new();
        c.heartbeat("a", "a:1").await.unwrap();
        c.advance(Duration::from_secs(10));
        c.heartbeat("a", "a:1").await.unwrap();
        c.heartbeat("b", "b:1").await.unwrap();
        let m = c.members().await.unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].age, Duration::from_secs(10));
        c.advance(MEMBER_TTL + Duration::from_secs(1));
        c.heartbeat("b", "b:1").await.unwrap();
        assert_eq!(c.members().await.unwrap().len(), 1, "a lapsed");
        c.heartbeat("a", "a:1").await.unwrap();
        let a = c
            .members()
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.pod == "a")
            .unwrap();
        assert_eq!(a.age, Duration::ZERO, "a joined again");
        c.leave("b").await.unwrap();
        assert_eq!(c.members().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_memory_coordinator_keeps_the_contract() {
        let n = std::sync::atomic::AtomicU32::new(0);
        let unique = |p: &str| format!("{p}-{}", n.fetch_add(1, Ordering::SeqCst));
        contract::run(&*MemCoordinator::new(), &unique).await;
    }

    #[tokio::test]
    async fn a_failing_coordinator_fails_every_call_until_it_answers_again() {
        let c = MemCoordinator::new();
        c.fail(true);
        assert!(c.heartbeat("a", "a:1").await.is_err());
        assert!(c.acquire("x", "a", "a:1").await.is_err());
        c.fail(false);
        assert_eq!(c.acquire("x", "a", "a:1").await.unwrap(), Some(1));
    }
}
