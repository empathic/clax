//! Live seats on the daemon (spec 2026-10-11-clax-dev-hosted-design §16.1):
//! [`clax_core::seats::Seats`] behind a lock, on the daemon's clock, with
//! the rules that map a [`Caller`] to a seat and refuse a spectator's writes.
//!
//! Who holds a seat ([`claim_for`]):
//! - the owner, reserved on every artifact;
//! - a connected person (`Caller::Person`), reserved where the artifact
//!   names them: they created it, or their rights name it by ID;
//! - a guest: a named viewer who is not a connected person;
//! - nobody else: a viewer with no name and an anonymous request take no
//!   seat (they cannot write either), and agents are never spectators.
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

use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use clax_core::rights::Location;
use clax_core::seats::{Claim, Pool, Released, SeatLimits, Seats};
use clax_core::working::Clock;
use clax_core::{ArtifactId, Store};
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

/// The seat key of `caller`, without a store read: `owner`, a connected
/// person's, a named guest's; `None` for anyone who takes no seat.
pub fn key_for(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Owner { .. } => Some(clax_core::seats::OWNER_KEY.into()),
        Caller::Person { viewer } => Some(clax_core::seats::person_key(&viewer.public_id)),
        Caller::Viewer { viewer } if viewer.display_name.is_some() => {
            Some(clax_core::seats::guest_key(&viewer.public_id))
        }
        _ => None,
    }
}

/// The seat `caller` asks for on `aid` (see the module docs); `None` for
/// agents and for viewers who take no seat.
///
/// # Errors
/// The store's.
pub fn claim_for(st: &Store, caller: &Caller, aid: &str) -> clax_core::Result<Option<Claim>> {
    Ok(match caller {
        Caller::Owner { .. } => Some(Claim::owner()),
        Caller::Person { viewer } => {
            let pid = viewer.public_id.as_str();
            let named = st.artifact_created_by(aid, Some(pid))?
                || st.person_rights(pid).is_ok_and(|r| {
                    r.iter().any(|a| {
                        a.locations().iter().any(|l| {
                            matches!(l, Location::Artifact(x) | Location::OwnArtifact(x) if x == aid)
                        })
                    })
                });
            Some(Claim::person(pid, named))
        }
        Caller::Viewer { viewer } if viewer.display_name.is_some() => {
            Some(Claim::guest(&viewer.public_id))
        }
        _ => None,
    })
}

/// 403 `spectator`: a write refused to a person beyond the live seats.
pub fn spectator_refusal() -> ApiError {
    ApiError::forbidden(
        clax_core::seats::SPECTATOR_CODE,
        clax_core::seats::SPECTATOR_MESSAGE,
    )
}

/// Refuses `caller` a write on `aid` while they are a spectator there
/// (spec §7): they hold no seat, none is reserved for them, and none is
/// free. Agents, the owner and callers who take no seat pass. The write is
/// input on the writer's seat.
///
/// # Errors
/// 403 `spectator` ([`spectator_refusal`]); the store's.
pub async fn require_seat(s: &AppState, caller: &Caller, aid: &ArtifactId) -> Result<(), ApiError> {
    match write_claim(s, caller, aid).await? {
        Some(c) if !s.seats.may_write(aid.as_str(), &c) => Err(spectator_refusal()),
        _ => Ok(()),
    }
}

/// The seat `caller`'s writes on `aid` are checked against
/// ([`LiveSeats::may_write`]), for a connection that checks each write
/// itself (a room socket); `None` for callers never refused: the owner,
/// agents, and callers who take no seat.
///
/// # Errors
/// The store's.
pub async fn write_claim(
    s: &AppState,
    caller: &Caller,
    aid: &ArtifactId,
) -> Result<Option<Claim>, ApiError> {
    Ok(match caller {
        Caller::Person { .. } | Caller::Viewer { .. } => {
            let (c, a) = (caller.clone(), aid.as_str().to_string());
            s.store_call(move |st| claim_for(st, &c, &a)).await?
        }
        _ => None,
    })
}

/// Whether `claim` (from [`write_claim`]) is a spectator's on `aid` now:
/// its writes are refused. `None` never is.
pub fn is_spectator(seats: &LiveSeats, aid: &str, claim: Option<&Claim>) -> bool {
    claim.is_some_and(|c| !seats.may_write(aid, c))
}

/// Why [`subscribe`] refused a subscription change.
#[derive(Debug, PartialEq, Eq)]
pub enum Subscribe {
    /// No live seat was free for the caller on one of the artifacts.
    NoSeat,
    /// The stream hub refused the change.
    Hub(clax_realtime::stream::SubError),
}

/// Subscribes stream `id` to `add` and unsubscribes it from `remove`
/// ([`clax_realtime::stream::Hub::update_via`]), holding the live seat of each
/// `(artifact ID, claim)` in `claims` ([`claim_for`] of the caller, for each
/// artifact whose live topics `add` names) under the stream's connection key
/// ([`clax_realtime::stream::seat_conn`]). `POST /api/stream/<id>` calls it, as
/// does any outlet that receives subscription changes another way (a
/// Durable Object's socket messages).
///
/// Each seat is taken before the topics are added: a change refused a seat
/// on any artifact is refused whole, and keeps no seat it took for it; a
/// change the hub refuses releases them too. The hub releases the seat of
/// an artifact whose last live topic the change removes.
///
/// Takes seats, so it must not be called with the hub's lock held (see the
/// module docs' lock order).
///
/// # Errors
/// [`Subscribe`].
pub fn subscribe(
    s: &AppState,
    id: &str,
    caller: &clax_core::db::Caller,
    credential: Option<&str>,
    claims: &[(String, Claim)],
    add: &[clax_realtime::stream::Topic],
    remove: &[clax_realtime::stream::Topic],
) -> Result<(u64, Vec<String>), Subscribe> {
    let conn = clax_realtime::stream::seat_conn(id);
    let mut taken: Vec<&str> = Vec::new();
    for (aid, c) in claims {
        let held = s.stream.holds_live(id, aid);
        if !s.seats.take(aid, c, &conn) {
            for a in &taken {
                s.seats.leave(a, &conn);
            }
            return Err(Subscribe::NoSeat);
        }
        if !held {
            taken.push(aid);
        }
    }
    s.stream
        .update_via(id, caller, credential, add, remove)
        .map_err(|e| {
            for a in &taken {
                s.seats.leave(a, &conn);
            }
            Subscribe::Hub(e)
        })
}

/// Links the daemon's seats to its stream hub and its live layers: the hub
/// holds and releases its streams' seats, and unsubscribes a stream whose
/// seat went idle; a live socket whose seat went idle is closed.
pub fn install(state: &AppState) {
    state.stream.use_seats(state.seats.clone());
    let hub = Arc::downgrade(&state.stream);
    let live = Arc::downgrade(&state.live_layers);
    state.seats.on_idle(Arc::new(move |r: &Released| {
        for c in &r.conns {
            if let Some(id) = c.strip_prefix(clax_realtime::stream::SEAT_CONN_PREFIX) {
                if let Some(hub) = hub.upgrade() {
                    hub.unseat(id, &r.artifact, clax_core::seats::IDLE_CODE);
                }
            } else if c.starts_with(clax_realtime::live_layers::SEAT_CONN_PREFIX)
                && let Some(live) = live.upgrade()
            {
                live.unseat(c);
            }
        }
    }));
}
