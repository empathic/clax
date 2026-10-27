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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::pg::{PgError, Pool, err};

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
    /// Extends `pod`'s lease of `instance` at `epoch`: false when it no
    /// longer holds it.
    async fn renew(&self, instance: &str, pod: &str, epoch: i64) -> Result<bool, PgError>;
    /// Gives up `pod`'s lease of `instance` at `epoch`, when it still holds
    /// it.
    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError>;
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

/// [`Coordinator`] over the fleet's tables (`crate::pg`).
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
                c.execute(
                    "INSERT INTO clax_fleet_members (pod, addr, joined, seen)
                     VALUES ($1, $2, now(), now())
                     ON CONFLICT (pod) DO UPDATE SET
                       addr = EXCLUDED.addr,
                       seen = now(),
                       joined = CASE
                         WHEN clax_fleet_members.seen < now() - make_interval(secs => $3)
                         THEN now() ELSE clax_fleet_members.joined END",
                    &[&pod, &addr, &secs(MEMBER_TTL)],
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
                c.execute("DELETE FROM clax_fleet_members WHERE pod = $1", &[&pod])
                    .map_err(err)?;
                Ok(())
            })
            .await
    }

    async fn members(&self) -> Result<Vec<Member>, PgError> {
        self.pool
            .call(|c| {
                let rows = c
                    .query(
                        "SELECT pod, addr, extract(epoch FROM now() - joined)::float8
                         FROM clax_fleet_members
                         WHERE seen >= now() - make_interval(secs => $1)
                         ORDER BY pod",
                        &[&secs(MEMBER_TTL)],
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
                    .query_opt(
                        "SELECT holder, addr, epoch FROM clax_fleet_leases
                         WHERE instance = $1 AND expires > now()",
                        &[&instance],
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
                    .query_opt(
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
                        &[&instance, &pod, &addr, &secs(LEASE_TTL)],
                    )
                    .map_err(err)?;
                Ok(row.map(|r| r.get(0)))
            })
            .await
    }

    async fn renew(&self, instance: &str, pod: &str, epoch: i64) -> Result<bool, PgError> {
        let (instance, pod) = (instance.to_string(), pod.to_string());
        self.pool
            .call(move |c| {
                let n = c
                    .execute(
                        "UPDATE clax_fleet_leases
                         SET expires = now() + make_interval(secs => $4)
                         WHERE instance = $1 AND holder = $2 AND epoch = $3",
                        &[&instance, &pod, &epoch, &secs(LEASE_TTL)],
                    )
                    .map_err(err)?;
                Ok(n == 1)
            })
            .await
    }

    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError> {
        let (instance, pod) = (instance.to_string(), pod.to_string());
        self.pool
            .call(move |c| {
                // Expired, not deleted: the epoch keeps rising.
                c.execute(
                    "UPDATE clax_fleet_leases SET expires = now()
                     WHERE instance = $1 AND holder = $2 AND epoch = $3",
                    &[&instance, &pod, &epoch],
                )
                .map_err(err)?;
                Ok(())
            })
            .await
    }
}

/// [`Coordinator`] in memory, on a clock the test moves: what several
/// processes of a test share in place of PostgreSQL.
#[derive(Default)]
pub struct MemCoordinator {
    inner: Mutex<MemState>,
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
        self.state().now += d;
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait::async_trait]
impl Coordinator for MemCoordinator {
    async fn heartbeat(&self, pod: &str, addr: &str) -> Result<(), PgError> {
        let mut s = self.state();
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
        self.state().members.remove(pod);
        Ok(())
    }

    async fn members(&self) -> Result<Vec<Member>, PgError> {
        let s = self.state();
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
        let s = self.state();
        Ok(s.leases
            .get(instance)
            .filter(|(_, expires)| *expires > s.now)
            .map(|(l, _)| l.clone()))
    }

    async fn acquire(&self, instance: &str, pod: &str, addr: &str) -> Result<Option<i64>, PgError> {
        let mut s = self.state();
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

    async fn renew(&self, instance: &str, pod: &str, epoch: i64) -> Result<bool, PgError> {
        let mut s = self.state();
        let now = s.now;
        match s.leases.get_mut(instance) {
            Some((l, expires)) if l.holder == pod && l.epoch == epoch => {
                *expires = now + LEASE_TTL;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn release(&self, instance: &str, pod: &str, epoch: i64) -> Result<(), PgError> {
        let mut s = self.state();
        let now = s.now;
        if let Some((l, expires)) = s.leases.get_mut(instance)
            && l.holder == pod
            && l.epoch == epoch
        {
            *expires = now;
        }
        Ok(())
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
}
