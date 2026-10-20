//! The queue owner of each job client (spec 2026-10-19-in-process-core-design
//! §7.5, §8.1): the one caller that may write that client's jobs, chosen by
//! the core with no clock. A claim is tied to the claimant's live event
//! stream (`GET /api/stream`) and released when that stream's connection
//! ends, by its token, or when the core closes: it lives in memory only, so
//! a crash, a close or a reopen of the core forgets every claim.
//!
//! Each job write is admitted under the claim ([`JobsOwners::enter`]) and
//! holds a [`WriteGuard`] across its store call. A release or a
//! replacement admits no write under the old token from then on, and a new
//! claim answers only once every write admitted under an older claim of
//! its client has ended, so no old owner's write lands after the new owner
//! holds the queue. Nothing here reads a clock.

use clax_core::{Event, EventBus};
use clax_realtime::stream::Hub;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// The header a write of a client's jobs carries its queue owner's token in.
pub const OWNER_HEADER: &str = "x-clax-jobs-owner";

/// A client's claim: its token and the stream it is tied to.
struct Claim {
    token: String,
    stream: String,
}

/// The writes admitted under one claim's token that have not ended.
struct Writes {
    client: String,
    count: usize,
}

#[derive(Default)]
struct Inner {
    /// Each client's claim.
    claims: HashMap<String, Claim>,
    /// By token: the writes in flight, of a claim held or released.
    writes: HashMap<String, Writes>,
}

/// The queue owner of each job client.
#[derive(Default)]
pub struct JobsOwners {
    inner: Mutex<Inner>,
    /// Notified whenever a write ends.
    ended: Notify,
}

/// A job write admitted under a claim ([`JobsOwners::enter`]): held across
/// the write's store call, it keeps a new claim of the client waiting.
pub struct WriteGuard {
    owners: Arc<JobsOwners>,
    token: String,
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        {
            let mut g = self.owners.lock();
            if let Some(w) = g.writes.get_mut(&self.token) {
                w.count -= 1;
                if w.count == 0 {
                    g.writes.remove(&self.token);
                }
            }
        }
        self.owners.ended.notify_waiters();
    }
}

/// What a claim answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claimed {
    /// The claimant owns the client's queue, with this token.
    Owner(String),
    /// Another claimant's stream holds it.
    Held,
    /// The stream named is not one the hub holds a connection of.
    NoSuchStream,
}

/// A claim's token: 32 hex characters from the operating system's random
/// source.
fn new_token() -> String {
    clax_core::secret::random_bytes::<16>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl JobsOwners {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A claim is only ever replaced whole and a count changed in one
        // step: a poisoned lock still holds them.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Admits one write of `client`'s jobs under `token`: a guard to hold
    /// across the write, or `None` when `token` does not hold the client's
    /// queue.
    pub fn enter(self: &Arc<Self>, client: &str, token: &str) -> Option<WriteGuard> {
        let mut g = self.lock();
        let holder = g.claims.get(client)?;
        if !clax_core::secret::constant_time_eq(holder.token.as_bytes(), token.as_bytes()) {
            return None;
        }
        let token = holder.token.clone();
        g.writes
            .entry(token.clone())
            .or_insert_with(|| Writes {
                client: client.to_string(),
                count: 0,
            })
            .count += 1;
        Some(WriteGuard {
            owners: self.clone(),
            token,
        })
    }

    /// Whether a write of `client`'s jobs admitted under a token other than
    /// `token` is in flight.
    fn writes_of_others(&self, client: &str, token: &str) -> bool {
        self.lock()
            .writes
            .iter()
            .any(|(t, w)| w.client == client && t != token)
    }

    /// Resolves once no write of `client`'s jobs admitted under a token
    /// other than `token` is in flight.
    async fn drained(&self, client: &str, token: &str) {
        loop {
            let ended = self.ended.notified();
            tokio::pin!(ended);
            ended.as_mut().enable();
            if !self.writes_of_others(client, token) {
                return;
            }
            ended.await;
        }
    }

    /// Claims `client`'s queue for the holder of event stream `stream`.
    /// [`Claimed::Held`] while another claim's stream has a connection
    /// open, and that connection is nudged ([`Hub::nudge`]) so a holder
    /// whose reader is gone is noticed at once; a claim by the stream that
    /// holds it answers its token again. A new claim replaces the one
    /// before at once (no write is admitted under the old token from then
    /// on) and answers once the writes admitted under older claims of
    /// `client` have ended. On [`Claimed::Owner`] it publishes
    /// `Event::JobsOwner { held: true }`, after spawning a task that waits
    /// for the stream's connection to end ([`Hub::closed`]) and then
    /// releases the claim, publishing `Event::JobsOwner { held: false }`;
    /// a claim released so while it waited answers
    /// [`Claimed::NoSuchStream`].
    pub async fn claim(
        self: &Arc<Self>,
        hub: &Hub,
        events: &EventBus,
        client: &str,
        stream: &str,
    ) -> Claimed {
        if !hub.is_attached(stream) {
            return Claimed::NoSuchStream;
        }
        let token = {
            let mut g = self.lock();
            let claims = &mut g.claims;
            if let Some(c) = claims.get(client) {
                if c.stream == stream {
                    return Claimed::Owner(c.token.clone());
                }
                if hub.is_attached(&c.stream) {
                    let held = c.stream.clone();
                    drop(g);
                    hub.nudge(&held);
                    return Claimed::Held;
                }
            }
            let token = new_token();
            claims.insert(
                client.to_string(),
                Claim {
                    token: token.clone(),
                    stream: stream.to_string(),
                },
            );
            token
        };
        let closed = hub.closed(stream);
        let owners = self.clone();
        let bus = events.clone();
        let (gone, held) = (client.to_string(), token.clone());
        clax_rt::spawn(async move {
            closed.await;
            if owners.release(&gone, &held) {
                bus.publish(Event::JobsOwner {
                    client: gone,
                    held: false,
                });
            }
        });
        self.drained(client, &token).await;
        if !self.holds(client, &token) {
            return Claimed::NoSuchStream;
        }
        events.publish(Event::JobsOwner {
            client: client.to_string(),
            held: true,
        });
        Claimed::Owner(token)
    }

    /// Releases `client`'s claim when `token` holds it; whether it did.
    /// The caller publishes `Event::JobsOwner { held: false }`.
    pub fn release(&self, client: &str, token: &str) -> bool {
        let claims = &mut self.lock().claims;
        let holds = claims.get(client).is_some_and(|c| {
            clax_core::secret::constant_time_eq(c.token.as_bytes(), token.as_bytes())
        });
        if holds {
            claims.remove(client);
        }
        holds
    }

    /// Releases the claim `token` holds, whichever client's it is; that
    /// client when there was one.
    pub fn release_token(&self, token: &str) -> Option<String> {
        let claims = &mut self.lock().claims;
        let client = claims
            .iter()
            .find(|(_, c)| {
                clax_core::secret::constant_time_eq(c.token.as_bytes(), token.as_bytes())
            })
            .map(|(k, _)| k.clone())?;
        claims.remove(&client);
        Some(client)
    }

    /// Whether `token` holds `client`'s queue.
    pub fn holds(&self, client: &str, token: &str) -> bool {
        self.lock().claims.get(client).is_some_and(|c| {
            clax_core::secret::constant_time_eq(c.token.as_bytes(), token.as_bytes())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clax_core::db::{Caller, Level};
    use clax_realtime::stream::Conn;
    use std::future::Future;
    use std::time::Duration;

    /// A stream with a connection open on `hub`; dropping the connection
    /// detaches it.
    fn attached(hub: &Arc<Hub>) -> (String, Conn) {
        let o = hub.open_for(Caller::new(Level::Owner, None), true, None, None, None);
        let id = o.id.clone();
        let (_tx, rx) = tokio::sync::watch::channel(false);
        (id, Conn::new(hub.clone(), o, Duration::from_secs(3600), rx))
    }

    fn token(c: Claimed) -> String {
        match c {
            Claimed::Owner(t) => t,
            c => panic!("{c:?}"),
        }
    }

    #[tokio::test]
    async fn a_new_claim_answers_once_the_old_claims_writes_end() {
        let hub = Hub::new(Default::default());
        let events = EventBus::new();
        let owners = Arc::new(JobsOwners::default());
        let (a, _conn_a) = attached(&hub);
        let old = token(owners.claim(&hub, &events, "app", &a).await);
        // A write admitted under the old claim, still running.
        let write = owners.enter("app", &old).expect("the holder is admitted");
        assert!(owners.enter("app", "not-the-token").is_none());
        // The claim is released (`DELETE /api/jobs/owner`): no new write is
        // admitted under its token, while the one in flight goes on.
        assert_eq!(owners.release_token(&old).as_deref(), Some("app"));
        assert!(owners.enter("app", &old).is_none(), "released at once");
        let (b, _conn_b) = attached(&hub);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut claim = std::pin::pin!(owners.claim(&hub, &events, "app", &b));
        assert!(
            claim.as_mut().poll(&mut cx).is_pending(),
            "the old claim's write is still running"
        );
        assert!(claim.as_mut().poll(&mut cx).is_pending());
        drop(write);
        let new = match claim.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(c) => token(c),
            std::task::Poll::Pending => panic!("the old claim's writes ended"),
        };
        assert_ne!(new, old);
        assert!(owners.enter("app", &new).is_some());
        assert!(owners.enter("app", &old).is_none());
    }

    #[tokio::test]
    async fn a_claim_replacing_a_detached_holder_waits_for_its_writes_and_admits_none_of_them() {
        let hub = Hub::new(Default::default());
        let events = EventBus::new();
        let owners = Arc::new(JobsOwners::default());
        let (a, conn_a) = attached(&hub);
        let old = token(owners.claim(&hub, &events, "app", &a).await);
        let write = owners.enter("app", &old).unwrap();
        // The holder's connection is gone; the release task has not run.
        drop(conn_a);
        let (b, _conn_b) = attached(&hub);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut claim = std::pin::pin!(owners.claim(&hub, &events, "app", &b));
        assert!(claim.as_mut().poll(&mut cx).is_pending());
        assert!(owners.enter("app", &old).is_none(), "replaced at once");
        // Writes of another client's claim do not hold it.
        let (c, _conn_c) = attached(&hub);
        let other = token(owners.claim(&hub, &events, "cli", &c).await);
        let _theirs = owners.enter("cli", &other).unwrap();
        drop(write);
        assert!(matches!(
            claim.as_mut().poll(&mut cx),
            std::task::Poll::Ready(Claimed::Owner(_))
        ));
    }
}
