//! Live seats on the daemon (spec 2026-10-11-clax-dev-hosted-design §16.1):
//! [`clax_core::seats::Seats`] behind a lock, on the daemon's clock. Who
//! holds a seat, and the rules that map a caller to one and refuse a
//! spectator's writes, are the HTTP layer's `seats`.
//!
//! A live connection holds the seat while it is open: on the native daemon,
//! a stream subscribed to an artifact's `artifact`, `presence` or `working`
//! topic ([`clax_realtime::stream::Hub`], which keys its seats `stream:<id>`), and a
//! live layers socket (`routes/live_layers.rs`, keyed `live:<n>`). Any such
//! other live connection takes a seat with
//! [`LiveSeats::take`] under a connection key of its own, leaves with
//! [`LiveSeats::leave`], asks [`LiveSeats::is_seated`] whether a person is
//! seated, and hears seats released for want of input through
//! [`LiveSeats::on_idle`].
//!
//! Input keeps a people seat: a `here` presence report
//! (`PUT /api/viewers/me/presence`) and every write. Seats idle for the idle
//! time are released by [`LiveSeats::sweep`] (the daemon's periodic sweep,
//! and before every seat decision).
//!
//! Lock order: the stream hub's lock may be held while this one is taken;
//! [`LiveSeats::sweep`] and the calls that sweep (`take`, `may_write`,
//! `is_seated`, `counts`) tell the idle listeners after releasing this lock,
//! and must not be called with the hub's lock held.

use clax_core::seats::{Claim, Pool, Released, SeatLimits, Seats};
use clax_core::working::Clock;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// Hears a seat released for want of input.
pub type IdleListener = Arc<dyn Fn(&Released) + Send + Sync>;

/// How long a built spectator snapshot is shared for one head version, in ms
/// (spec §16.2).
pub const SNAPSHOT_SHARE_MS: i64 = 5000;

/// A snapshot shared for a while: its head version, when it was built
/// (ms), and its body.
type Shared = (u32, i64, Arc<Vec<u8>>);

/// The seats of every artifact (see the module docs).
pub struct LiveSeats {
    inner: Mutex<Seats>,
    clock: Arc<dyn Clock>,
    listeners: Mutex<Vec<IdleListener>>,
    /// The latest spectator snapshot built per artifact: its head version,
    /// when it was built (ms), and its body.
    snapshots: Mutex<HashMap<String, Shared>>,
    /// One snapshot build at a time per artifact, so requests that miss
    /// together share one build.
    builds: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// What the spectator snapshot says about seats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub viewing: u32,
    pub seats_free: u32,
    pub guest_seats_free: u32,
}

impl LiveSeats {
    pub fn new(limits: SeatLimits, clock: Arc<dyn Clock>) -> Arc<LiveSeats> {
        Arc::new(LiveSeats {
            inner: Mutex::new(Seats::new(limits)),
            clock,
            listeners: Mutex::new(Vec::new()),
            snapshots: Mutex::new(HashMap::new()),
            builds: Mutex::new(HashMap::new()),
        })
    }

    /// Seats with the limits the environment sets ([`SeatLimits::from_env`]).
    pub fn from_env(clock: Arc<dyn Clock>) -> Arc<LiveSeats> {
        LiveSeats::new(SeatLimits::from_env(|k| std::env::var(k).ok()), clock)
    }

    fn lock(&self) -> MutexGuard<'_, Seats> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The daemon's time in ms.
    pub fn now_ms(&self) -> i64 {
        self.clock.now().timestamp_millis()
    }

    pub fn limits(&self) -> SeatLimits {
        self.lock().limits()
    }

    /// Changes the limits (tests, and a demo with one people seat).
    pub fn set_limits(&self, limits: SeatLimits) {
        self.lock().set_limits(limits);
    }

    /// Adds a listener for seats released for want of input.
    pub fn on_idle(&self, f: IdleListener) {
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(f);
    }

    /// Runs `f` on the seats after releasing the idle ones, then tells the
    /// listeners, outside the lock.
    fn swept<T>(&self, f: impl FnOnce(&mut Seats, i64) -> T) -> T {
        let now = self.now_ms();
        let (out, gone) = {
            let mut g = self.lock();
            let gone = g.release_idle(now);
            (f(&mut g, now), gone)
        };
        self.tell(&gone);
        out
    }

    fn tell(&self, gone: &[Released]) {
        if gone.is_empty() {
            return;
        }
        let ls: Vec<IdleListener> = self
            .listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for r in gone {
            for l in &ls {
                l(r);
            }
        }
    }

    /// Releases the seats gone idle and tells the listeners.
    pub fn sweep(&self) {
        self.swept(|_, _| ());
    }

    /// How long until [`LiveSeats::sweep`] releases the next people seat
    /// gone idle (zero once it is due); `None` with none held.
    pub fn next_idle_in(&self) -> Option<std::time::Duration> {
        let at = self.lock().next_idle_at()?;
        let ms = at.saturating_sub(self.now_ms()).max(0);
        Some(std::time::Duration::from_millis(ms as u64))
    }

    /// Connection `conn` takes `claim`'s seat on `aid`: whether it holds one
    /// now. A refused person is noted as a spectator (viewing).
    pub fn take(&self, aid: &str, claim: &Claim, conn: &str) -> bool {
        self.swept(|s, now| {
            let ok = s.take(aid, claim, conn, now);
            if !ok && claim.pool == Pool::People {
                s.see_spectator(aid, &claim.key, now);
            }
            ok
        })
    }

    /// Every seat held now ([`Seats::kept`]).
    pub fn kept(&self) -> Vec<clax_core::seats::KeptSeat> {
        self.lock().kept()
    }

    /// Holds `kept` again until their holders' connections take them
    /// ([`Seats::restore`]).
    pub fn restore(&self, kept: &[clax_core::seats::KeptSeat]) {
        self.lock().restore(kept);
    }

    /// Connection `conn` leaves its seat on `aid`.
    pub fn leave(&self, aid: &str, conn: &str) {
        self.lock().leave(aid, conn);
    }

    /// Connection `conn` dropped and may come back ([`Seats::park`]).
    pub fn park(&self, conn: &str) {
        self.lock().park(conn);
    }

    /// Connection `conn` came back: the artifacts it got no seat on again.
    /// Does not sweep, so it may be called under the stream hub's lock.
    pub fn unpark(&self, conn: &str) -> Vec<String> {
        let now = self.now_ms();
        self.lock().unpark(conn, now)
    }

    /// Connection `conn` is gone for good.
    pub fn forget(&self, conn: &str) {
        self.lock().forget(conn);
    }

    /// Input on seat `key` of `aid`.
    pub fn touch(&self, aid: &str, key: &str) {
        let now = self.now_ms();
        self.lock().touch(aid, key, now);
    }

    /// The seat decision: whether seat `key` is held on `aid` now (seats
    /// gone idle released first). A live relay asks it before it sends a
    /// person live layers.
    pub fn is_seated(&self, aid: &str, key: &str) -> bool {
        self.swept(|s, _| s.seated(aid, key))
    }

    /// Whether `claim` may write on `aid`: it holds a seat, one is reserved
    /// for it, or one is free (a person without a live connection writes
    /// while any seat is free, spec §16.1). A write is input on the seat.
    pub fn may_write(&self, aid: &str, claim: &Claim) -> bool {
        self.swept(|s, now| {
            let ok = s.may_hold(aid, claim);
            if ok {
                s.touch(aid, &claim.key, now);
            }
            ok
        })
    }

    /// Whether `claim` holds or may take a seat on `aid` now (seats gone
    /// idle released first): `false` for a spectator. Not input.
    pub fn may_hold(&self, aid: &str, claim: &Claim) -> bool {
        self.swept(|s, _| s.may_hold(aid, claim))
    }

    /// Notes spectator `key` of `aid` as viewing now, unless it is seated.
    pub fn see_spectator(&self, aid: &str, key: &str) {
        let now = self.now_ms();
        self.lock().see_spectator(aid, key, now);
    }

    /// Who is viewing `aid` and how many people seats are free.
    pub fn counts(&self, aid: &str) -> Counts {
        self.swept(|s, now| Counts {
            viewing: s.viewing(aid, now),
            seats_free: s.free(aid, Pool::People, false),
            guest_seats_free: s.free(aid, Pool::People, true),
        })
    }

    /// The snapshot of `aid` at head version `head` built in the last
    /// [`SNAPSHOT_SHARE_MS`], if any.
    pub fn shared_snapshot(&self, aid: &str, head: u32) -> Option<Arc<Vec<u8>>> {
        let now = self.now_ms();
        let g = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        g.get(aid)
            .filter(|(h, at, _)| *h == head && now >= *at && now - *at < SNAPSHOT_SHARE_MS)
            .map(|(_, _, b)| b.clone())
    }

    /// The lock a build of `aid`'s snapshot holds: whoever takes it after
    /// another build looks for that build's snapshot first.
    pub fn build_lock(&self, aid: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut g = self.builds.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|_, l| Arc::strong_count(l) > 1);
        g.entry(aid.to_string()).or_default().clone()
    }

    /// Keeps `body` as `aid`'s snapshot at head version `head`, built now.
    pub fn keep_snapshot(&self, aid: &str, head: u32, body: Arc<Vec<u8>>) {
        let now = self.now_ms();
        let mut g = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|_, (_, at, _)| now - *at < SNAPSHOT_SHARE_MS);
        g.insert(aid.to_string(), (head, now, body));
    }
}
