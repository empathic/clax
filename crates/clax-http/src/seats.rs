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
//!
//! The seats themselves, and the live connections that hold them, are
//! [`LiveSeats`] (`clax_realtime::seats`).

use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use clax_core::ArtifactId;
use clax_core::rights::Location;
use clax_core::seats::{Claim, Released, SeatLimits};
use clax_store::Store;
use std::sync::Arc;

pub use clax_realtime::seats::{Counts, IdleListener, LiveSeats, SNAPSHOT_SHARE_MS};

/// The seat limits of a core: none ([`SeatLimits::OFF`]) for the app's
/// in-process core (`embedded`), where everything runs in the app and its
/// person is the only one there; a daemon's from `get` (an environment
/// lookup, [`SeatLimits::from_env`]).
pub fn limits_for(embedded: bool, get: impl Fn(&str) -> Option<String>) -> SeatLimits {
    if embedded {
        SeatLimits::OFF
    } else {
        SeatLimits::from_env(get)
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

#[cfg(test)]
mod tests {
    use super::limits_for;
    use clax_core::seats::{Claim, ENV_PEOPLE, SeatLimits, Seats};

    #[test]
    fn only_the_apps_core_has_no_seats() {
        let one = |k: &str| (k == ENV_PEOPLE).then(|| "1".to_string());
        assert_eq!(limits_for(true, one), SeatLimits::OFF);
        assert_eq!(limits_for(false, |_| None), SeatLimits::default());
        assert_eq!(limits_for(false, one).people, 1);
    }

    #[test]
    fn without_seats_nobody_idles_out_or_is_a_spectator() {
        let mut s = Seats::new(limits_for(true, |_| None));
        s.take("a1", &Claim::owner(), "c0", 0);
        for i in 0..200 {
            assert!(s.take("a1", &Claim::person(&format!("p{i}"), false), &format!("c{i}"), 0));
        }
        assert_eq!(s.next_idle_at(), None);
        assert!(s.release_idle(i64::MAX).is_empty());
        assert!(s.seated("a1", clax_core::seats::OWNER_KEY));
        assert!(s.may_hold("a1", &Claim::guest("g1")));
    }
}
